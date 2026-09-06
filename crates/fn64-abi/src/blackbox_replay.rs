#![cfg(test)]

//! Black-box shim replay (cleanup plan task 5.6).
//!
//! `AGENTS.md`'s clean-room protocol allows exactly one way to learn how the
//! GPL reference runtime behaves: a differential experiment against it as a
//! black box. This module is fn64's half of that experiment.
//!
//! A GPL driver outside this repo (`tools/shim-probe/shim-probe.cpp` in the
//! aki-recomp checkout, never copied here) links the reference runtime,
//! executes a scenario script, and prints one observation tuple per call.
//! `tests/blackbox/*.json` holds those scripts; `tests/blackbox/*.observed.json`
//! holds the recorded reference tuples with their provenance header. This test
//! replays the same scripts through fn64's own shims and classifies each tuple:
//!
//! - `match` — a value was compared and agreed.
//! - `deliberate-divergence` — fn64 differs on purpose, and the entry in
//!   [`DELIBERATE_DIVERGENCES`] carries the public libultra manual citation
//!   that justifies fn64's behavior, plus fn64's exact value.
//! - `not-observed` — the driver could not drive the call as a black box; the
//!   recording carries the measured reason.
//! - `not-compared` — the recording and the script both name nothing, so
//!   replaying the tuple verified nothing.
//! - `unexplained` — anything else. Only this fails the test.
//!
//! `not-compared` and `not-observed` are counted apart from `match` because a
//! tuple that compares nothing is a check that cannot fail, and folding it into
//! the match count overstates coverage. For the same reason a recorded key the
//! script does not observe is `unexplained`, not skipped: a recorded value
//! nothing is compared against verifies nothing, and letting it pass green is
//! how a corrupted recording would go unnoticed.
//!
//! The recorded files are facts about a black-box run. They carry no runtime
//! code and describe no runtime internals, which is what makes them citable
//! here at all.

use super::*;
use crate::test_support::{ctx_zeroed, run_to_idle_with_yielder_plumbing, spawn_test_thread};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------
// A deliberately tiny JSON reader. The scenario schema is fixed and this
// crate has no JSON dependency; adding one for four test fixtures would be
// a heavier change than the reader it replaces.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_array(&self) -> &[Json] {
        match self {
            Json::Arr(items) => items,
            other => panic!("blackbox fixture: expected a JSON array, found {other:?}"),
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            other => panic!("blackbox fixture: expected a JSON string, found {other:?}"),
        }
    }

    fn as_num(&self) -> u64 {
        match self {
            Json::Num(n) => *n,
            other => panic!("blackbox fixture: expected a JSON number, found {other:?}"),
        }
    }

    /// Object entries in source order. Scenario `regs`/`poke_words` are
    /// applied in the order written, which the seeding semantics depend on.
    fn entries(&self) -> &[(String, Json)] {
        match self {
            Json::Obj(entries) => entries,
            other => panic!("blackbox fixture: expected a JSON object, found {other:?}"),
        }
    }
}

struct JsonParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn parse(&mut self) -> Json {
        self.skip_ws();
        let byte = *self
            .src
            .get(self.pos)
            .unwrap_or_else(|| panic!("blackbox fixture: unexpected end of input"));
        match byte {
            b'{' => self.parse_obj(),
            b'[' => self.parse_arr(),
            b'"' => Json::Str(self.parse_str()),
            b't' => {
                self.pos += 4;
                Json::Bool(true)
            }
            b'f' => {
                self.pos += 5;
                Json::Bool(false)
            }
            b'n' => {
                self.pos += 4;
                Json::Null
            }
            _ => self.parse_num(),
        }
    }

    fn parse_obj(&mut self) -> Json {
        self.pos += 1; // '{'
        let mut entries = Vec::new();
        loop {
            self.skip_ws();
            if self.src.get(self.pos) == Some(&b'}') {
                self.pos += 1;
                return Json::Obj(entries);
            }
            let key = self.parse_str();
            self.skip_ws();
            assert_eq!(
                self.src.get(self.pos),
                Some(&b':'),
                "blackbox fixture: expected ':' after object key {key:?}"
            );
            self.pos += 1;
            entries.push((key, self.parse()));
            self.skip_ws();
            match self.src.get(self.pos) {
                Some(&b',') => self.pos += 1,
                Some(&b'}') => {}
                other => panic!("blackbox fixture: expected ',' or '}}', found {other:?}"),
            }
        }
    }

    fn parse_arr(&mut self) -> Json {
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.src.get(self.pos) == Some(&b']') {
                self.pos += 1;
                return Json::Arr(items);
            }
            items.push(self.parse());
            self.skip_ws();
            match self.src.get(self.pos) {
                Some(&b',') => self.pos += 1,
                Some(&b']') => {}
                other => panic!("blackbox fixture: expected ',' or ']', found {other:?}"),
            }
        }
    }

    fn parse_str(&mut self) -> String {
        assert_eq!(
            self.src.get(self.pos),
            Some(&b'"'),
            "blackbox fixture: expected a string"
        );
        self.pos += 1;
        let mut out = String::new();
        while let Some(&byte) = self.src.get(self.pos) {
            match byte {
                b'"' => {
                    self.pos += 1;
                    return out;
                }
                b'\\' => {
                    self.pos += 1;
                    let escaped = self.src[self.pos];
                    out.push(match escaped {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        other => other as char,
                    });
                    self.pos += 1;
                }
                other => {
                    out.push(other as char);
                    self.pos += 1;
                }
            }
        }
        panic!("blackbox fixture: unterminated string");
    }

    fn parse_num(&mut self) -> Json {
        let start = self.pos;
        if self.src.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        let negative = self.pos != start;
        let digits_start = self.pos;
        let value = if self.src[self.pos..].starts_with(b"0x") {
            self.pos += 2;
            let hex_start = self.pos;
            while self
                .src
                .get(self.pos)
                .is_some_and(u8::is_ascii_hexdigit)
            {
                self.pos += 1;
            }
            u64::from_str_radix(
                std::str::from_utf8(&self.src[hex_start..self.pos]).expect("ascii hex digits"),
                16,
            )
            .expect("blackbox fixture: hex literal fits u64")
        } else {
            while self.src.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            std::str::from_utf8(&self.src[digits_start..self.pos])
                .expect("ascii digits")
                .parse::<u64>()
                .expect("blackbox fixture: decimal literal fits u64")
        };
        Json::Num(if negative {
            (value as i64).wrapping_neg() as u64
        } else {
            value
        })
    }
}

fn parse_json(src: &str) -> Json {
    JsonParser::new(src).parse()
}

// ---------------------------------------------------------------------
// Deliberate divergences.
//
// Each entry says: for this scenario and this call index, fn64's tuple
// differs from the reference's on purpose, and here is the public libultra
// manual section that justifies fn64's choice. An entry is only honored when
// fn64 actually produces `fn64_regs`; if fn64 later changes, the tuple stops
// matching the recorded divergence and the test reports it unexplained.
// ---------------------------------------------------------------------

struct DeliberateDivergence {
    scenario: &'static str,
    call_index: usize,
    /// The fn64 register values that this divergence licenses, as
    /// `(register name, value)`. Only these registers are exempted.
    fn64_regs: &'static [(&'static str, u64)],
    citation: &'static str,
}

const DELIBERATE_DIVERGENCES: &[DeliberateDivergence] = &[
    // -----------------------------------------------------------------
    // osSendMesg on a full queue, OS_MESG_NOBLOCK.
    // -----------------------------------------------------------------
    DeliberateDivergence {
        scenario: "mesgqueue-full-and-jam",
        call_index: 2,
        fn64_regs: &[("r2", u64::MAX)],
        citation: "Public libultra Function Reference, Message Manager, \
                   `osSendMesg` \"Explanation\": a non-blocking send whose queue \
                   is full does not enqueue the message and returns -1. The \
                   reference returned 0 for the same call. fn64 follows the \
                   manual because the return value is the only signal a guest \
                   has that its message was dropped; returning 0 for a dropped \
                   message reports success for work that did not happen, which \
                   `AGENTS.md`'s \"loud traps, no silent shrugs\" rule forbids. \
                   Pinned independently by `mesgqueue.rs`'s own \
                   `noblock_send_return_value_in_v0_is_minus_one_on_full_zero_on_enqueue`.",
    },
    DeliberateDivergence {
        scenario: "mesgqueue-full-and-jam",
        call_index: 3,
        fn64_regs: &[("r2", u64::MAX)],
        citation: "Same as call 2: the third non-blocking send onto the same \
                   still-full capacity-1 queue. Public libultra Function \
                   Reference, Message Manager, `osSendMesg` \"Explanation\".",
    },
    // -----------------------------------------------------------------
    // osSetIntMask / __osDisableInt return register.
    // -----------------------------------------------------------------
    DeliberateDivergence {
        scenario: "intmask-and-ai",
        call_index: 0,
        fn64_regs: &[("r2", 0), ("status_reg", 1)],
        citation: "Public libultra Function Reference, Interrupt Manager, \
                   `osSetIntMask` \"Return Value\": the previous interrupt mask. \
                   The reference left the caller's seeded $v0 (0x12345678) \
                   untouched and left Status unchanged, so it neither returns \
                   the documented value nor installs the requested mask for \
                   this call. fn64 returns the previous mask (0, from the \
                   zeroed Status this call was seeded with) and sets Status.IE, \
                   which is the mask argument 1 taking effect. A guest that \
                   reads $v0 after this call gets whatever the previous call \
                   left there under the reference, which cannot be what the \
                   manual describes.",
    },
    DeliberateDivergence {
        scenario: "intmask-and-ai",
        call_index: 1,
        fn64_regs: &[("r2", 1)],
        citation: "Same as call 0. fn64 returns 1: the mask installed by call 0 \
                   read back as the previous mask, per `osSetIntMask` \
                   \"Return Value\". Status returns to 0 because this call's \
                   mask argument is 0. The reference again left the sentinel.",
    },
    DeliberateDivergence {
        scenario: "intmask-and-ai",
        call_index: 2,
        fn64_regs: &[("r2", 1), ("status_reg", 0)],
        citation: "Public libultra Function Reference, Interrupt Manager, \
                   `__osDisableInt` \"Return Value\": the previous \
                   interrupt-enable state, which `__osRestoreInt` consumes. The \
                   reference left the seeded sentinel in $v0 and left Status.IE \
                   set, so it did not disable interrupts. fn64 returns the \
                   previous Status.IE bit (1) and clears it, so the documented \
                   disable/restore pairing works; `system.rs`'s own \
                   disable/restore round-trip test pins it.",
    },
    // -----------------------------------------------------------------
    // osAiSetFrequency.
    // -----------------------------------------------------------------
    DeliberateDivergence {
        scenario: "intmask-and-ai",
        call_index: 4,
        fn64_regs: &[("r2", 22047)],
        citation: "Public libultra Function Reference, Audio Interface, \
                   `osAiSetFrequency` \"Return Value\": the frequency actually \
                   set, which is the VI clock divided by the integer DAC \
                   divisor, not the frequency requested. The reference echoed \
                   the request (22050). fn64 returns the achievable rate \
                   because the guest uses it to size its audio buffers; echoing \
                   the request hides the rounding the hardware performs. \
                   Arithmetic, not asserted: NTSC VI clock 48681812, rounded \
                   divisor (48681812 + 22050/2) / 22050 = 2208, achievable rate \
                   48681812 / 2208 = 22047.",
    },
    DeliberateDivergence {
        scenario: "intmask-and-ai",
        call_index: 5,
        fn64_regs: &[("r2", 32006)],
        citation: "Same as call 4, for a non-hardware rate (32001 requested). \
                   Public libultra Function Reference, Audio Interface, \
                   `osAiSetFrequency` \"Return Value\". Arithmetic, not \
                   asserted: NTSC VI clock 48681812, rounded divisor \
                   (48681812 + 32001/2) / 32001 = 1521, achievable rate \
                   48681812 / 1521 = 32006. The reference echoed 32001, which \
                   no integer divisor of the VI clock produces.",
    },
    DeliberateDivergence {
        scenario: "intmask-and-ai",
        call_index: 6,
        fn64_regs: &[("r2", u32::MAX as u64)],
        citation: "Public libultra Function Reference, Audio Interface, \
                   `osAiSetFrequency` \"Return Value\": -1 when the requested \
                   frequency cannot be set. The required divisor for 1 Hz is \
                   48681812, which is computed without difficulty but falls \
                   outside the AI DAC rate register's range \
                   (`AI_MIN_DAC_RATE..=AI_MAX_DAC_RATE`, 132..=16384 in \
                   `ai.rs`) -- the range check is what rejects it, not an \
                   arithmetic limit. The reference echoed 1, a rate no N64 AI \
                   can produce.",
    },
];

fn lookup_divergence(scenario: &str, call_index: usize) -> Option<&'static DeliberateDivergence> {
    DELIBERATE_DIVERGENCES
        .iter()
        .find(|d| d.scenario == scenario && d.call_index == call_index)
}

// ---------------------------------------------------------------------
// Replay.
// ---------------------------------------------------------------------

/// Where a scenario's scratch rdram lives. Scenario addresses are KSEG0
/// (`0x80xxxxxx`), the same convention the driver uses.
const RDRAM_BYTES: usize = 8 * 1024 * 1024;

thread_local! {
    /// The one process RDRAM allocation this test thread registers.
    ///
    /// `register_process_rdram` refuses to replace a live allocation, so every
    /// scenario in a run must present the same pointer and length. Holding one
    /// buffer per test thread satisfies that; it is zeroed between scenarios so
    /// each still starts from a clean guest memory image.
    static SCENARIO_RDRAM: std::cell::RefCell<Box<[u8]>> =
        std::cell::RefCell::new(vec![0u8; RDRAM_BYTES].into_boxed_slice());
}

/// A borrow of this thread's scenario rdram, zeroed for a fresh scenario.
///
/// Held as a raw pointer because the shims under test take `*mut u8` and the
/// executor keeps the same pointer registered for the whole run; the slice
/// accessor exists so the harness's own pokes and peeks stay bounds-checked.
struct ScenarioRdram(*mut u8);

impl ScenarioRdram {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.0
    }

    /// # Safety notes
    /// The pointer comes from a live `RDRAM_BYTES` thread-local buffer that
    /// outlives every use, and nothing else aliases it during a scenario.
    fn bytes(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.0, RDRAM_BYTES) }
    }
}

fn scenario_rdram() -> ScenarioRdram {
    SCENARIO_RDRAM.with(|cell| {
        let mut buffer = cell.borrow_mut();
        buffer.fill(0);
        ScenarioRdram(buffer.as_mut_ptr())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Match,
    DeliberateDivergence(&'static str),
    Unexplained(String),
    NotObserved,
    /// The reference tuple was recorded, but the script asked for nothing to
    /// compare it against, so replaying it verified nothing.
    ///
    /// This is deliberately not `Match`. A vacuous tuple counted as a match
    /// inflates the coverage number with checks that cannot fail, which is the
    /// green-theater this harness exists to avoid. It is also not a failure:
    /// the recording is honest, the script simply does not interrogate it.
    NotCompared,
}

/// One replayed call's result: the fn64 registers the scenario asked to
/// observe, plus any rdram words it asked to peek.
#[derive(Default, Debug)]
struct Tuple {
    regs: BTreeMap<String, u64>,
    words: BTreeMap<String, u64>,
}

fn write_reg(ctx: &mut RecompContext, name: &str, value: u64) {
    match name {
        "r2" => ctx.r2 = value,
        "r3" => ctx.r3 = value,
        "r4" => ctx.r4 = value,
        "r5" => ctx.r5 = value,
        "r6" => ctx.r6 = value,
        "r7" => ctx.r7 = value,
        "r29" => ctx.r29 = value,
        "status_reg" => ctx.status_reg = value as u32,
        other => panic!("blackbox scenario seeds unsupported register {other}"),
    }
}

fn read_reg(ctx: &RecompContext, name: &str) -> u64 {
    match name {
        "r2" => ctx.r2,
        "r3" => ctx.r3,
        "r4" => ctx.r4,
        "r5" => ctx.r5,
        "r6" => ctx.r6,
        "r7" => ctx.r7,
        "r29" => ctx.r29,
        "status_reg" => u64::from(ctx.status_reg),
        other => panic!("blackbox scenario observes unsupported register {other}"),
    }
}

/// Dispatch one scripted call to fn64's shim of the same name.
///
/// Every shim reached here is called on a live executor thread, because
/// fn64's message-queue shims suspend the active coroutine rather than
/// returning a status inline (`docs/DESIGN.md` § 2's single stackful-coroutine
/// model). That is a structural difference from the reference's inline
/// implementation, not a behavioral one: what the harness compares is the
/// tuple each side leaves behind, which is what a guest can observe.
fn call_fn64_shim(shim: &str, rdram: *mut u8, ctx: &mut RecompContext) {
    // SAFETY: `rdram` points at this scenario's live `RDRAM_BYTES` buffer for
    // the whole replay, and `ctx` is a live stack `RecompContext`. Both
    // outlive the call, which is every fn64 shim's documented contract.
    unsafe {
        match shim {
            "osCreateMesgQueue" => osCreateMesgQueue_recomp(rdram, ctx),
            "osSendMesg" => osSendMesg_recomp(rdram, ctx),
            "osJamMesg" => osJamMesg_recomp(rdram, ctx),
            "osRecvMesg" => osRecvMesg_recomp(rdram, ctx),
            "osSetIntMask" => osSetIntMask_recomp(rdram, ctx),
            "__osDisableInt" => __osDisableInt_recomp(rdram, ctx),
            "__osRestoreInt" => __osRestoreInt_recomp(rdram, ctx),
            "osAiSetFrequency" => osAiSetFrequency_recomp(rdram, ctx),
            other => panic!(
                "blackbox replay reached shim {other}, which this harness does not \
                 dispatch. Add it here rather than skipping the scenario."
            ),
        }
    }
}

/// Replay one scenario against fn64 and classify every recorded tuple.
fn replay(scenario_json: &str, observed_json: &str) -> Vec<(usize, String, Verdict)> {
    let script = parse_json(scenario_json);
    let observed = parse_json(observed_json);

    let scenario_name = script
        .get("scenario")
        .expect("scenario file names its scenario")
        .as_str()
        .to_owned();
    assert_eq!(
        scenario_name,
        observed
            .get("scenario")
            .expect("observation file names its scenario")
            .as_str(),
        "observation file records a different scenario than the script it pairs with"
    );
    assert!(
        observed.get("provenance").is_some(),
        "observation file {scenario_name} has no provenance header; a recorded \
         observation without its runtime commit, driver commit, date and command \
         is not a citable fact"
    );

    let calls = script.get("calls").expect("script has calls").as_array();
    let tuples = observed
        .get("observations")
        .expect("observation file has observations")
        .as_array();
    assert_eq!(
        calls.len(),
        tuples.len(),
        "scenario {scenario_name} scripts {} calls but the recorded run has {} \
         tuples; the recording is stale",
        calls.len(),
        tuples.len()
    );

    // A scenario may declare host preconditions that the reference driver got
    // implicitly from the process it ran in. Declaring them in the script
    // keeps the two sides driven from the same file rather than from setup
    // hidden in this harness.
    if let Some(setup) = script.get("setup") {
        for (key, value) in setup.entries() {
            match (key.as_str(), value.as_str()) {
                ("tv_type", "ntsc") => {
                    crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
                }
                ("tv_type", "pal") => {
                    crate::configure_tv_type(fn64_runtime::TvType::Pal);
                }
                (other, value) => panic!(
                    "blackbox scenario {scenario_name} declares unsupported setup \
                     {other}={value}"
                ),
            }
        }
    }

    let mut rdram = scenario_rdram();
    let rdram_ptr = rdram.as_mut_ptr();
    // fn64's executor mirrors the `OSMesgQueue` struct into guest rdram, but
    // only once the process allocation is registered; unregistered it returns
    // early and writes nothing. Without this the harness would read its own
    // zeroed buffer and score a queue-header comparison against fn64 output
    // that never happened -- which is exactly the vacuous-comparison failure
    // this file guards against, one level down.
    //
    // SAFETY: `rdram` is a live `RDRAM_BYTES` allocation that outlives every
    // shim call below, and the registration is idempotent for the same
    // pointer/length pair, which `scenario_rdram` guarantees by handing back
    // the one per-thread buffer.
    unsafe { crate::register_process_rdram(rdram_ptr, RDRAM_BYTES) };
    let mut ctx = ctx_zeroed();
    let mut verdicts = Vec::new();

    for (index, (call, recorded)) in calls.iter().zip(tuples).enumerate() {
        let shim = call.get("shim").expect("call names its shim").as_str();
        let status = recorded
            .get("status")
            .expect("recorded tuple has a status")
            .as_str();

        if status == "not-observed" {
            // The driver could not drive this call as a black box. The reason
            // is recorded; nothing is invented, and nothing is compared.
            verdicts.push((index, shim.to_owned(), Verdict::NotObserved));
            continue;
        }

        // Seed registers in source order, then rdram words.
        if let Some(regs) = call.get("regs") {
            for (name, value) in regs.entries() {
                write_reg(&mut ctx, name, value.as_num());
            }
        }
        if let Some(pokes) = call.get("poke_words") {
            for (addr, value) in pokes.entries() {
                let addr = parse_kseg0(addr);
                let offset = (addr - 0x8000_0000) as usize;
                let word = value.as_num() as u32;
                rdram.bytes()[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
            }
        }

        call_fn64_shim(shim, rdram_ptr, &mut ctx);

        let mut actual = Tuple::default();
        if let Some(observe) = call.get("observe") {
            for name in observe.as_array() {
                let name = name.as_str();
                actual.regs.insert(name.to_owned(), read_reg(&ctx, name));
            }
        }
        if let Some(peeks) = call.get("peek_words") {
            for addr in peeks.as_array() {
                let addr = addr.as_num();
                let offset = (addr - 0x8000_0000) as usize;
                let word = u32::from_ne_bytes(
                    rdram.bytes()[offset..offset + 4]
                        .try_into()
                        .expect("four rdram bytes"),
                );
                actual
                    .words
                    .insert(format!("{addr:#x}"), u64::from(word));
            }
        }

        verdicts.push((
            index,
            shim.to_owned(),
            classify(&scenario_name, index, recorded, &actual),
        ));
    }

    verdicts
}

fn parse_kseg0(text: &str) -> u64 {
    let trimmed = text.trim_start_matches("0x");
    u64::from_str_radix(trimmed, 16).unwrap_or_else(|_| {
        text.parse::<u64>()
            .unwrap_or_else(|_| panic!("blackbox scenario address {text} is not a number"))
    })
}

/// A recorded key the script does not observe. Reported as its own kind of
/// mismatch, because "the recording and the script disagree about what this
/// call produces" is a different defect from "fn64 produced a different value",
/// and silently skipping it lets a corrupted recording pass as a match.
const UNOBSERVED: u64 = u64::MAX - 0xDEAD;

fn classify(scenario: &str, index: usize, recorded: &Json, actual: &Tuple) -> Verdict {
    let mut mismatches = Vec::new();
    // Counts what was actually put side by side, so a tuple that compared
    // nothing is never reported as a match.
    let mut compared = 0usize;

    if let Some(regs) = recorded.get("regs") {
        for (name, value) in regs.entries() {
            let reference = value.as_num();
            let Some(&mine) = actual.regs.get(name.as_str()) else {
                mismatches.push((
                    format!("{name} (recorded but not observed by the script)"),
                    reference,
                    UNOBSERVED,
                ));
                continue;
            };
            compared += 1;
            if mine != reference {
                mismatches.push((name.clone(), reference, mine));
            }
        }
    }
    if let Some(words) = recorded.get("words") {
        for (addr, value) in words.entries() {
            let reference = value.as_num();
            let Some(&mine) = actual.words.get(addr.as_str()) else {
                mismatches.push((
                    format!("{addr} (recorded but not observed by the script)"),
                    reference,
                    UNOBSERVED,
                ));
                continue;
            };
            compared += 1;
            if mine != reference {
                mismatches.push((addr.clone(), reference, mine));
            }
        }
    }

    if mismatches.is_empty() && compared == 0 {
        return Verdict::NotCompared;
    }

    if mismatches.is_empty() {
        return Verdict::Match;
    }

    // A recorded key the script never observes is never licensable: a citation
    // justifies a value fn64 produced, and here fn64 produced nothing to
    // justify. Checked explicitly rather than left to the fact that the
    // decorated key name cannot match a licensed one.
    if mismatches.iter().any(|(_, _, mine)| *mine == UNOBSERVED) {
        return Verdict::Unexplained(format!(
            "{scenario} call {index}: the recording names a key the script does not \
             observe, so replaying it compares nothing. Mismatches (name, reference, \
             fn64) = {mismatches:?}, where {UNOBSERVED} marks the unobserved key. The \
             recording and the script have drifted apart: either add the key to the \
             script's `observe`/`peek_words`, or re-record the scenario."
        ));
    }

    // A divergence is only deliberate when fn64 produced exactly the value the
    // citation licenses. A divergence that drifted to some third value is
    // unexplained, which is the whole point of pinning the value here.
    if let Some(divergence) = lookup_divergence(scenario, index) {
        let licensed: BTreeMap<&str, u64> = divergence.fn64_regs.iter().copied().collect();
        let all_licensed = mismatches.iter().all(|(name, _, mine)| {
            licensed
                .get(name.as_str())
                .is_some_and(|&expected| expected == *mine)
        });
        if all_licensed {
            return Verdict::DeliberateDivergence(divergence.citation);
        }
        return Verdict::Unexplained(format!(
            "{scenario} call {index}: a deliberate divergence is recorded here, but fn64 \
             produced a value the citation does not license. Recorded divergence expects \
             {licensed:?}; observed mismatches (name, reference, fn64) = {mismatches:?}. \
             Either fn64's behavior changed or the citation is stale -- do not widen the \
             exemption without re-reading the manual section it names."
        ));
    }

    Verdict::Unexplained(format!(
        "{scenario} call {index}: fn64 differs from the recorded reference tuple with no \
         deliberate-divergence entry. Mismatches (name, reference, fn64) = {mismatches:?}. \
         Either fix fn64, or add a DELIBERATE_DIVERGENCES entry with the public libultra \
         manual section that justifies the difference."
    ))
}

// ---------------------------------------------------------------------
// The scenarios, and the one test that runs them all.
// ---------------------------------------------------------------------

macro_rules! scenario {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../tests/blackbox/", $name, ".json")),
            include_str!(concat!("../tests/blackbox/", $name, ".observed.json")),
        )
    };
}

const SCENARIOS: &[(&str, &str, &str)] = &[
    scenario!("mesgqueue-noblock"),
    scenario!("mesgqueue-full-and-jam"),
    scenario!("intmask-and-ai"),
    scenario!("timer-and-dma"),
];

/// Verdict counts for a replayed run.
#[derive(Default)]
struct Tally {
    matched: usize,
    deliberate: usize,
    not_observed: usize,
    not_compared: usize,
    unexplained: Vec<String>,
}

impl Tally {
    fn of<'a>(
        results: impl Iterator<Item = &'a (String, Vec<(usize, String, Verdict)>)>,
    ) -> Self {
        let mut tally = Tally::default();
        for (scenario, verdicts) in results {
            for (index, shim, verdict) in verdicts {
                match verdict {
                    Verdict::Match => tally.matched += 1,
                    Verdict::DeliberateDivergence(_) => tally.deliberate += 1,
                    Verdict::NotObserved => tally.not_observed += 1,
                    Verdict::NotCompared => tally.not_compared += 1,
                    Verdict::Unexplained(detail) => tally
                        .unexplained
                        .push(format!("{scenario} call {index} ({shim}): {detail}")),
                }
            }
        }
        tally
    }
}

/// Replay every recorded black-box observation through fn64's shims.
///
/// Only `unexplained` fails. `match` and `deliberate-divergence` pass;
/// `not-observed` records that the driver could not drive that call as a black
/// box, and `not-compared` that the script asked for nothing to compare. The
/// last two are counted apart from `match` precisely so neither can pad the
/// coverage number with a check that cannot fail.
#[test]
fn recorded_black_box_observations_replay_without_unexplained_divergence() {
    // fn64's message-queue shims suspend the active coroutine, so the replay
    // runs on a real executor thread rather than bare. Every scenario shares
    // one thread: the scripts are written as a single guest thread's call
    // sequence, which is also how the driver executes them.
    let results = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = results.clone();
    spawn_test_thread(0x5B0, 10, move || {
        for (name, script, observed) in SCENARIOS {
            let verdicts = replay(script, observed);
            sink.borrow_mut().push(((*name).to_owned(), verdicts));
        }
    });
    run_to_idle_with_yielder_plumbing();

    let results = results.borrow();
    assert_eq!(
        results.len(),
        SCENARIOS.len(),
        "the replay thread did not finish every scenario; it ran {} of {}",
        results.len(),
        SCENARIOS.len()
    );

    let tally = Tally::of(results.iter());
    let Tally {
        matched,
        deliberate,
        not_observed,
        not_compared,
        ref unexplained,
    } = tally;

    eprintln!(
        "[blackbox] {matched} match, {deliberate} deliberate-divergence, \
         {not_observed} not-observed, {not_compared} not-compared, {} unexplained",
        unexplained.len()
    );

    assert!(
        unexplained.is_empty(),
        "black-box replay found {} unexplained divergence(s) from the recorded \
         reference run:\n{}",
        unexplained.len(),
        unexplained.join("\n")
    );

    // A harness that silently stopped comparing would also report zero
    // unexplained, so pin the exact shape of the run rather than just that it
    // is non-empty. `matched` counts only tuples where a value was actually put
    // side by side; a vacuous tuple lands in `not_compared` and cannot pad it.
    assert_eq!(
        (matched, deliberate, not_observed, not_compared),
        (18, 8, 2, 0),
        "black-box replay verdict counts changed. If this is intended -- a \
         re-recording, a new scenario, a scenario that now observes a value it \
         previously did not -- update this pin, the counts in \
         `crates/fn64-abi/tests/blackbox/README.md`, and the \
         black-box paragraph in `docs/COMPLETENESS.md` in the same commit."
    );
}

/// The reviewer's probe, kept as a test: a recorded reference value that the
/// script does not observe must be reported, not skipped.
///
/// Before this was fixed, `classify` hit `else { continue; }` for such a key
/// and the tuple fell through to `Match`, so planting a contradictory value
/// into a recording left the suite green. That is the exact green-theater shape
/// this harness exists to prevent, so it gets a test rather than a comment.
#[test]
fn a_recorded_key_the_script_does_not_observe_is_unexplained() {
    let script = r#"{
        "scenario": "probe",
        "calls": [ { "shim": "osSetIntMask", "regs": { "r4": 1 } } ]
    }"#;
    // The recording names r2; the script's call observes nothing at all.
    let observed = r#"{
        "provenance": { "note": "synthetic fixture for this test" },
        "scenario": "probe",
        "observations": [
            { "shim": "osSetIntMask", "status": "observed", "regs": { "r2": 424242 } }
        ]
    }"#;

    let verdicts = replay_on_executor(script, observed);
    let (_, _, verdict) = &verdicts[0];
    let Verdict::Unexplained(detail) = verdict else {
        panic!("a recorded key the script does not observe must be unexplained, got {verdict:?}");
    };
    assert!(
        detail.contains("does not observe"),
        "the failure must name the drift between recording and script, got: {detail}"
    );
}

/// A tuple whose script observes nothing and whose recording claims nothing is
/// `not-compared`, never `match`: replaying it verified nothing, and counting
/// it as a match would overstate coverage.
#[test]
fn a_tuple_with_nothing_to_compare_is_not_counted_as_a_match() {
    let script = r#"{
        "scenario": "probe",
        "calls": [ { "shim": "osSetIntMask", "regs": { "r4": 1 } } ]
    }"#;
    let observed = r#"{
        "provenance": { "note": "synthetic fixture for this test" },
        "scenario": "probe",
        "observations": [
            { "shim": "osSetIntMask", "status": "observed", "regs": {} }
        ]
    }"#;

    let verdicts = replay_on_executor(script, observed);
    let (_, _, verdict) = &verdicts[0];
    assert_eq!(
        *verdict,
        Verdict::NotCompared,
        "a tuple that compared nothing must be not-compared, not match"
    );

    let results = vec![("probe".to_owned(), verdicts)];
    let tally = Tally::of(results.iter());
    assert_eq!(
        (tally.matched, tally.not_compared),
        (0, 1),
        "a vacuous tuple must not be counted among the matches"
    );
}

/// `replay` calls shims that suspend the active coroutine, so it needs a live
/// executor thread. Shared by the tests above.
fn replay_on_executor(script: &'static str, observed: &'static str) -> Vec<(usize, String, Verdict)> {
    let out = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = out.clone();
    spawn_test_thread(0x5B1, 10, move || {
        *sink.borrow_mut() = replay(script, observed);
    });
    run_to_idle_with_yielder_plumbing();
    let verdicts = out.borrow().clone();
    assert!(!verdicts.is_empty(), "the replay thread produced no verdicts");
    verdicts
}
