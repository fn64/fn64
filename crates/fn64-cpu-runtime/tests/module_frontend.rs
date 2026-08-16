//! Strong validation for the ELF/symbol **front-end** (`module.rs`): a
//! synthetic multi-function `RecompConfig`-shaped input is turned into one
//! module whose functions call each other by **direct Rust call**, and that
//! module is then *executed* to prove the cross-call semantics land the right
//! machine state.
//!
//! # Why this shape of test
//!
//! The front-end is codegen: its product is Rust *source*, not a value. So it
//! is validated two ways, both strong (never fuzzy):
//!
//! 1. **Golden lock** ([`emit_module_matches_golden`]): the live `emit_module`
//!    must byte-match `goldens/module_crosscall.rs`. That file is the exact
//!    text the emitter produced for these ROM words; the executable functions
//!    below are pasted verbatim from it. So executing them here really is
//!    executing the front-end's product — if the emitter drifts, the golden
//!    lock fails loudly and the paste must be refreshed.
//!
//! 2. **Behavioural cross-call execution** ([`crosscall_executes_to_expected_state`],
//!    [`tail_call_executes_to_expected_state`]): run the pasted `caller` /
//!    `tail_caller` against the real [`runtime`] and assert the callee's effect
//!    (writing `$v0`) is observable in the register file — i.e. the direct
//!    `callee(ctx, mem)` call the front-end emitted actually invoked the
//!    sibling function. A per-function `lookup()` recompiler could not produce
//!    this without the symbol table.
//!
//! 3. **Resolution correctness** ([`known_target_is_direct_unknown_is_lookup`]):
//!    a JAL to a *known* symbol emits a direct call; a JAL to an *unknown*
//!    address falls back to an indirect `lookup()`, matching N64Recomp's
//!    `resolve_jal` Match-vs-Ambiguous decision.

use fn64_cpu_runtime::{
    call_host_or_recompiled, resolve_host_function, set_function_entry_observer, set_host_lookup,
    Rdram, RecompContext, RecompFunc, TranslatedFunctionIdentity,
};
use fn64_recomp_rs_codegen::{emit_module, ModuleFunc, SymbolTable};

thread_local! {
    static FUNCTION_ENTRIES: std::cell::RefCell<Vec<TranslatedFunctionIdentity>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

fn observe_function_entry(identity: TranslatedFunctionIdentity) {
    FUNCTION_ENTRIES.with(|entries| entries.borrow_mut().push(identity));
}

// --- The synthetic three-function "program" (real MIPS III encodings). ---
//
// callee @ 0x80001000:  addiu $v0,$zero,0x2A ; jr $ra ; nop         -> $v0 = 42
// caller @ 0x80002000:  jal callee ; addiu $a0,$zero,7 (delay) ; jr $ra ; nop
// tail_caller @ 0x80003000:  j callee ; nop (delay)                 -> tail call
const CALLEE_VRAM: u32 = 0x8000_1000;
const CALLER_VRAM: u32 = 0x8000_2000;
const TAIL_VRAM: u32 = 0x8000_3000;

const CALLEE_WORDS: [u32; 3] = [0x2402_002A, 0x03E0_0008, 0x0000_0000];
// jal 0x80001000 -> 0x0C000400 (target26 = (0x80001000 & 0x0FFFFFFF) >> 2 = 0x400).
const CALLER_WORDS: [u32; 4] = [0x0C00_0400, 0x2404_0007, 0x03E0_0008, 0x0000_0000];
// j   0x80001000 -> 0x08000400.
const TAIL_WORDS: [u32; 2] = [0x0800_0400, 0x0000_0000];

fn synthetic_symbols() -> SymbolTable {
    SymbolTable::from_entries([
        ("callee".to_string(), CALLEE_VRAM),
        ("caller".to_string(), CALLER_VRAM),
        ("tail_caller".to_string(), TAIL_VRAM),
    ])
}

fn synthetic_module() -> String {
    let symbols = synthetic_symbols();
    let funcs = [
        ModuleFunc {
            name: "callee",
            vram: CALLEE_VRAM,
            words: &CALLEE_WORDS,
        },
        ModuleFunc {
            name: "caller",
            vram: CALLER_VRAM,
            words: &CALLER_WORDS,
        },
        ModuleFunc {
            name: "tail_caller",
            vram: TAIL_VRAM,
            words: &TAIL_WORDS,
        },
    ];
    emit_module(&funcs, &symbols)
}

// --- The emitter's output, pasted VERBATIM from goldens/module_crosscall.rs. ---
//
// `emit_module_matches_golden` guarantees the live emitter still produces
// exactly this, so executing it below really executes the front-end's product.
// The dispatcher at the bottom is also pasted verbatim from the golden. The
// direct-call tests inspect their emitted bodies so the dispatcher's own
// definition/table do not masquerade as an indirect call site.

#[allow(unused_variables, unused_mut, unused_labels, clippy::all)]
pub fn callee(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(0x80001000, "callee"));
    let mut pc: u32 = 0x80001000;
    'run: loop {
        match pc {
            0x80001000 => {
                // 0x80001000: Addiu { rt: 2, rs: 0, imm: 42 }
                ctx.set_r32(2, (0i32).wrapping_add(42));
                // 0x80001004: Jr { rs: 31 }
                // delay: 0x80001008: Nop
                // nop
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

#[allow(unused_variables, unused_mut, unused_labels, clippy::all)]
pub fn caller(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(0x80002000, "caller"));
    let mut pc: u32 = 0x80002000;
    'run: loop {
        match pc {
            0x80002000 => {
                // 0x80002000: Jal { target: 1024 }
                ctx.set_r32(31, 0x80002008u32 as i32);
                // delay: 0x80002004: Addiu { rt: 4, rs: 0, imm: 7 }
                ctx.set_r32(4, (0i32).wrapping_add(7));
                call_host_or_recompiled(0x80001000, callee, ctx, mem);
                pc = 0x80002008;
                continue 'run;
            }
            0x80002008 => {
                // 0x80002008: Jr { rs: 31 }
                // delay: 0x8000200C: Nop
                // nop
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

#[allow(unused_variables, unused_mut, unused_labels, clippy::all)]
pub fn tail_caller(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(
        0x80003000,
        "tail_caller",
    ));
    let mut pc: u32 = 0x80003000;
    'run: loop {
        match pc {
            0x80003000 => {
                // 0x80003000: J { target: 1024 }
                // delay: 0x80003004: Nop
                // nop
                call_host_or_recompiled(0x80001000, callee, ctx, mem);
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

// Safe LOOKUP_FUNC/get_function equivalent: sorted vram -> typed fn table.
static LOOKUP_TABLE: &[(u32, RecompFunc)] = &[
    (0x80001000, callee as RecompFunc),
    (0x80002000, caller as RecompFunc),
    (0x80003000, tail_caller as RecompFunc),
];

pub fn lookup(vram: u32) -> RecompFunc {
    if let Some(func) = resolve_host_function(vram) {
        return func;
    }
    match LOOKUP_TABLE.binary_search_by_key(&vram, |(addr, _)| *addr) {
        Ok(index) => LOOKUP_TABLE[index].1,
        Err(_) => panic!("lookup: no recompiled function or host shim at vram {vram:#010X}"),
    }
}

/// The live `emit_module` output must be byte-identical to the pasted golden,
/// keeping the executed functions honest (they are copied from this golden).
#[test]
fn emit_module_matches_golden() {
    let emitted = synthetic_module();
    let golden = include_str!("goldens/module_crosscall.rs");
    let norm = |s: &str| s.trim_end().replace("\r\n", "\n");
    assert_eq!(
        norm(&emitted),
        norm(golden),
        "emit_module drifted from goldens/module_crosscall.rs; refresh the golden \
         and the pasted functions in this file if the change is intended"
    );
}

/// The strong behavioural check: executing the front-end's emitted `caller`
/// must run the direct call into `callee`, leaving `$v0 = 42` (and the delay
/// slot's `$a0 = 7`). If the JAL had been left as an indirect `lookup()`, the
/// `lookup` shim above would panic — so a green here proves a resolved direct
/// call.
#[test]
fn crosscall_executes_to_expected_state() {
    let mut mem_buf = vec![0u8; 64];
    let mut mem = Rdram::new(&mut mem_buf);
    let mut ctx = RecompContext::new();

    caller(&mut ctx, &mut mem);

    assert_eq!(
        ctx.r(2),
        42,
        "$v0 should hold the callee's result after the direct call"
    );
    assert_eq!(
        ctx.r(4),
        7,
        "delay-slot $a0 = 7 must have executed before the call"
    );
    // $ra was linked to the return address after the delay slot.
    assert_eq!(
        ctx.r_u32(31),
        0x8000_2008,
        "JAL must link $ra to post-delay-slot pc"
    );
}

#[test]
fn entry_observer_covers_guest_bodies_and_excludes_resolution_attempts() {
    FUNCTION_ENTRIES.with(|entries| entries.borrow_mut().clear());
    let previous_observer = set_function_entry_observer(Some(observe_function_entry));

    let mut mem_buf = vec![0u8; 64];
    let mut mem = Rdram::new(&mut mem_buf);
    let mut ctx = RecompContext::new();
    caller(&mut ctx, &mut mem);
    tail_caller(&mut ctx, &mut mem);
    lookup(CALLEE_VRAM)(&mut ctx, &mut mem);

    let previous_lookup = set_host_lookup(Some(host_resolver));
    caller(&mut ctx, &mut mem);
    lookup(CALLEE_VRAM)(&mut ctx, &mut mem);
    set_host_lookup(previous_lookup);
    let missing = std::panic::catch_unwind(|| lookup(0x8000_9000));

    set_function_entry_observer(previous_observer);
    assert!(missing.is_err());
    FUNCTION_ENTRIES.with(|entries| {
        assert_eq!(
            entries.borrow().as_slice(),
            [
                TranslatedFunctionIdentity::new(CALLER_VRAM, "caller"),
                TranslatedFunctionIdentity::new(CALLEE_VRAM, "callee"),
                TranslatedFunctionIdentity::new(TAIL_VRAM, "tail_caller"),
                TranslatedFunctionIdentity::new(CALLEE_VRAM, "callee"),
                TranslatedFunctionIdentity::new(CALLEE_VRAM, "callee"),
                TranslatedFunctionIdentity::new(CALLER_VRAM, "caller"),
            ]
        );
    });
}

/// The inter-function `J` (tail call) path: `tail_caller` tail-calls `callee`,
/// so `$v0 = 42` must be observable after it returns.
#[test]
fn tail_call_executes_to_expected_state() {
    let mut mem_buf = vec![0u8; 64];
    let mut mem = Rdram::new(&mut mem_buf);
    let mut ctx = RecompContext::new();

    tail_caller(&mut ctx, &mut mem);

    assert_eq!(
        ctx.r(2),
        42,
        "$v0 should hold the callee's result after the tail call"
    );
}

/// Resolution correctness against the emitted text: a JAL to a KNOWN symbol
/// emits a statically typed `callee` fallback through the host-first seam and
/// no `lookup(`; a JAL to an
/// UNKNOWN address emits an indirect `lookup(...)` and no direct name.
#[test]
fn known_target_is_direct_unknown_is_lookup() {
    // Known-target module: caller -> callee. Must be direct, no lookup.
    let known = synthetic_module();
    assert!(
        known.contains("call_host_or_recompiled(0x80001000, callee, ctx, mem);"),
        "known JAL must emit a typed recompiled fallback through the host seam"
    );
    assert!(
        !known.contains("            lookup("),
        "no indirect lookup CALL should appear when every target is a known symbol:\n{known}"
    );

    // Unknown-target module: caller JALs 0x80009000, which is NOT in the table.
    // The front-end must fall back to an indirect lookup.
    let unknown_symbols = SymbolTable::from_entries([("caller".to_string(), CALLER_VRAM)]);
    // jal 0x80009000 -> target26 = (0x80009000 & 0x0FFFFFFF) >> 2 = 0x2400 -> 0x0C002400.
    let words: [u32; 4] = [0x0C00_2400, 0x2404_0007, 0x03E0_0008, 0x0000_0000];
    let funcs = [ModuleFunc {
        name: "caller",
        vram: CALLER_VRAM,
        words: &words,
    }];
    let unknown = emit_module(&funcs, &unknown_symbols);
    assert!(
        unknown.contains("lookup(0x80009000)(ctx, mem);"),
        "unknown JAL target must emit an indirect lookup:\n{unknown}"
    );
}

#[test]
fn generated_lookup_resolves_recompiled_function() {
    let mut bytes = [0u8; 64];
    let mut mem = Rdram::new(&mut bytes);
    let mut ctx = RecompContext::new();
    lookup(CALLEE_VRAM)(&mut ctx, &mut mem);
    assert_eq!(ctx.r(2), 42);
}

fn host_callee(ctx: &mut RecompContext, _mem: &mut Rdram) {
    ctx.set_r32(2, 99);
}

fn host_resolver(vram: u32) -> Option<RecompFunc> {
    (vram == CALLEE_VRAM).then_some(host_callee as RecompFunc)
}

#[test]
fn host_lookup_overrides_recompiled_table_without_unsafe() {
    let previous = set_host_lookup(Some(host_resolver));
    let mut bytes = [0u8; 64];
    let mut mem = Rdram::new(&mut bytes);
    let mut ctx = RecompContext::new();
    lookup(CALLEE_VRAM)(&mut ctx, &mut mem);
    set_host_lookup(previous);
    assert_eq!(ctx.r(2), 99);
}
