//! Env-gated GBI/RDP opcode census.
//!
//! Counts every command byte `decode_stream_impl` dispatches on, split by
//! lane: the RSP-side GBI display-list lane (`raw_rdp == false`) and the
//! raw-RDP passthrough lane (`raw_rdp == true`). The same byte means
//! different things in the two lanes -- `0x05` is `G_TRI1` in GBI and an RDP
//! No Operation in raw-RDP -- so a single flat histogram would be wrong, and
//! the two are kept separate for that reason, not for presentation.
//!
//! Off unless `FN64_GBI_CENSUS` is set (see `crate::debug_flag`). When off,
//! [`note`] loads one relaxed atomic and returns; nothing else runs. Set
//! `FN64_GBI_CENSUS_OUT` to a path to also write the TSV report there on
//! [`report`]; otherwise the report goes to stderr.
//!
//! Names come from this crate's own `state::opcode_name`, the table the
//! decoder's unsupported-command diagnostic already prints from, so a census
//! row and a decode panic name the same command identically. Bytes that
//! table does not recognize are reported as `UNNAMED_<byte>` rather than
//! guessed at or merged under its placeholder string.

#[cfg(not(test))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};

/// One counter per lane per possible command byte. Flat arrays, not maps:
/// the hook runs once per display-list command in the hottest decode loop in
/// the crate, and a relaxed `fetch_add` into a fixed slot is the cheapest
/// thing that is still correct under the decoder's threading.
static GBI_COUNTS: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
static RDP_COUNTS: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
/// Display-list decode entries observed, i.e. how many top-level task walks
/// the counts below are summed over. Not a frame count: one task can decode
/// several nested lists, and a frame can span more than one task.
static DECODE_ENTRIES: AtomicU64 = AtomicU64::new(0);

#[cfg(not(test))]
static ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static INIT: AtomicBool = AtomicBool::new(false);

/// Whether the census is armed. Reads the env var exactly once.
///
/// Always off under `cfg(test)`: `crate::debug_flag` is itself
/// `cfg(not(test))` (unit tests must not observe ambient `FN64_*` knobs), and
/// a census armed by a stray env var would let one test's counts leak into
/// another's. The naming tables below stay testable regardless -- they are
/// pure functions and do not consult this.
pub fn on() -> bool {
    #[cfg(test)]
    {
        false
    }
    #[cfg(not(test))]
    {
        if crate::speculative_observations_suppressed() {
            return false;
        }
        if !INIT.swap(true, Ordering::Relaxed) {
            ENABLED.store(crate::debug_flag("FN64_GBI_CENSUS"), Ordering::Relaxed);
        }
        ENABLED.load(Ordering::Relaxed)
    }
}

/// Count one dispatched command byte.
///
/// `raw_rdp` selects the lane, and must be the same flag
/// `decode_stream_impl` used to canonicalize the opcode -- the count is of
/// what the decoder dispatched on, not of the wire byte before
/// normalization, because the dispatch byte is what a backend would have to
/// admit.
pub fn note(opcode: u8, raw_rdp: bool) {
    if !on() {
        return;
    }
    let table = if raw_rdp { &RDP_COUNTS } else { &GBI_COUNTS };
    table[opcode as usize].fetch_add(1, Ordering::Relaxed);
}

/// Per-decode-entry command totals, appended in decode order when
/// `FN64_GBI_CENSUS_PER_TASK` is set. Answers a question the cumulative
/// histogram cannot: whether one frame's worth of commands is a small set or
/// the whole set, i.e. how much a backend must admit to draw ONE frame rather
/// than to survive a whole boot.
static PER_ENTRY: std::sync::Mutex<Vec<Vec<(u8, u64)>>> = std::sync::Mutex::new(Vec::new());

#[cfg(not(test))]
static PER_ENTRY_ON: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static PER_ENTRY_INIT: AtomicBool = AtomicBool::new(false);

/// Whether per-decode-entry snapshots are being kept. Separate knob from
/// [`on`] because the snapshot vector grows without bound, which is the wrong
/// default for a harness expected to run for millions of steps.
pub fn per_entry_on() -> bool {
    #[cfg(test)]
    {
        false
    }
    #[cfg(not(test))]
    {
        if !on() {
            return false;
        }
        if !PER_ENTRY_INIT.swap(true, Ordering::Relaxed) {
            PER_ENTRY_ON.store(
                crate::debug_flag("FN64_GBI_CENSUS_PER_TASK"),
                Ordering::Relaxed,
            );
        }
        PER_ENTRY_ON.load(Ordering::Relaxed)
    }
}

/// Snapshot the raw-RDP lane's current totals as one per-entry row. Called at
/// each top-level decode entry, so consecutive rows differ by exactly one
/// decode's worth of commands.
fn snapshot_entry() {
    if !per_entry_on() {
        return;
    }
    let row: Vec<(u8, u64)> = (0u16..256)
        .filter_map(|b| {
            let opcode = b as u8;
            let count = RDP_COUNTS[opcode as usize].load(Ordering::Relaxed);
            (count > 0).then_some((opcode, count))
        })
        .collect();
    if let Ok(mut guard) = PER_ENTRY.lock() {
        guard.push(row);
    }
}

/// Per-decode-entry deltas: for each entry after the first, the commands that
/// entry alone issued. Empty unless `FN64_GBI_CENSUS_PER_TASK` is set.
pub fn per_entry_deltas() -> Vec<Vec<(u8, u64)>> {
    let Ok(guard) = PER_ENTRY.lock() else {
        return Vec::new();
    };
    guard
        .windows(2)
        .map(|pair| {
            let before: std::collections::HashMap<u8, u64> = pair[0].iter().copied().collect();
            pair[1]
                .iter()
                .filter_map(|&(opcode, after)| {
                    let delta = after - before.get(&opcode).copied().unwrap_or(0);
                    (delta > 0).then_some((opcode, delta))
                })
                .collect()
        })
        .collect()
}

/// Count one entry into a top-level display-list decode.
pub fn note_decode_entry() {
    if !on() {
        return;
    }
    DECODE_ENTRIES.fetch_add(1, Ordering::Relaxed);
    snapshot_entry();
}

/// Name for a dispatched command byte in the RSP-side GBI lane.
///
/// Delegates to this crate's own `state::opcode_name`, the table the
/// decoder's own unsupported-command diagnostic prints from, so a census row
/// and a decode panic name the same command identically. Bytes that table
/// does not recognize surface as `UNNAMED_<byte>` from [`rows`] rather than
/// as its `G_<unrecognized>` placeholder, which would merge distinct bytes.
fn gbi_name(opcode: u8) -> Option<&'static str> {
    match super::state::opcode_name(opcode) {
        "G_<unrecognized>" => None,
        name => Some(name),
    }
}

/// Name for a dispatched command byte in the raw-RDP passthrough lane.
///
/// `decode_stream_impl` canonicalizes raw-RDP command ids into the GBI byte
/// space before dispatch (`canonical_raw_rdp_opcode`: `0x00..=0x0f` stay
/// bare, everything else becomes `0xc0 | command`). The bare low block is
/// therefore NOT the GBI command of the same byte -- `0x05` is an RDP No
/// Operation here, not `G_TRI1` -- and is named by its RDP meaning. Above
/// `0x0f` the two spellings coincide and [`gbi_name`] is correct.
fn raw_rdp_lane_name(opcode: u8) -> Option<&'static str> {
    Some(match opcode {
        0x00..=0x07 => "RDP_NOOP",
        0x08 => "RDP_TRI_FILL",
        0x09 => "RDP_TRI_FILL_Z",
        0x0a => "RDP_TRI_TEX",
        0x0b => "RDP_TRI_TEX_Z",
        0x0c => "RDP_TRI_SHADE",
        0x0d => "RDP_TRI_SHADE_Z",
        0x0e => "RDP_TRI_SHADE_TEX",
        0x0f => "RDP_TRI_SHADE_TEX_Z",
        _ => return gbi_name(opcode),
    })
}

/// One census row: the lane, the dispatch byte, the name, and the count.
pub struct Row {
    pub lane: &'static str,
    pub opcode: u8,
    pub name: String,
    pub count: u64,
}

/// Snapshot every non-zero counter, GBI lane first, each lane sorted by
/// descending count.
pub fn rows() -> Vec<Row> {
    let mut out = Vec::new();
    for (lane, table, namer) in [
        (
            "GBI",
            &GBI_COUNTS,
            gbi_name as fn(u8) -> Option<&'static str>,
        ),
        (
            "RDP",
            &RDP_COUNTS,
            raw_rdp_lane_name as fn(u8) -> Option<&'static str>,
        ),
    ] {
        let mut lane_rows: Vec<Row> = (0u16..256)
            .filter_map(|byte| {
                let opcode = byte as u8;
                let count = table[opcode as usize].load(Ordering::Relaxed);
                (count > 0).then(|| Row {
                    lane,
                    opcode,
                    name: namer(opcode)
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("UNNAMED_{opcode:#04x}")),
                    count,
                })
            })
            .collect();
        lane_rows.sort_by(|a, b| b.count.cmp(&a.count).then(a.opcode.cmp(&b.opcode)));
        out.extend(lane_rows);
    }
    out
}

/// How many top-level decode entries the counts cover.
pub fn decode_entries() -> u64 {
    DECODE_ENTRIES.load(Ordering::Relaxed)
}

/// Render the census as TSV and emit it: to `FN64_GBI_CENSUS_OUT` when that
/// names a path, otherwise to stderr. A no-op when the census is off.
pub fn report() {
    report_inner(true);
}

/// [`report`] without the confirmation line, for callers flushing on a tight
/// cadence so a mid-run abort cannot lose the numbers. Writing the file is
/// the point; announcing each write would bury the run's real output.
pub fn report_quiet() {
    report_inner(false);
}

fn report_inner(announce: bool) {
    if !on() {
        return;
    }
    let rows = rows();
    let mut text = String::new();
    text.push_str(&format!(
        "# fn64 GBI/RDP opcode census\n# decode_entries\t{}\n",
        decode_entries()
    ));
    let gbi_total: u64 = rows
        .iter()
        .filter(|r| r.lane == "GBI")
        .map(|r| r.count)
        .sum();
    let rdp_total: u64 = rows
        .iter()
        .filter(|r| r.lane == "RDP")
        .map(|r| r.count)
        .sum();
    text.push_str(&format!(
        "# gbi_lane_commands\t{gbi_total}\n# rdp_lane_commands\t{rdp_total}\n"
    ));
    text.push_str("lane\topcode\tname\tcount\n");
    for row in &rows {
        text.push_str(&format!(
            "{}\t{:#04x}\t{}\t{}\n",
            row.lane, row.opcode, row.name, row.count
        ));
    }
    let deltas = per_entry_deltas();
    if !deltas.is_empty() {
        text.push_str("\n# per-decode-entry deltas (raw-RDP lane)\nentry\topcode\tname\tcount\n");
        for (index, delta) in deltas.iter().enumerate() {
            for &(opcode, count) in delta {
                let name = raw_rdp_lane_name(opcode)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("UNNAMED_{opcode:#04x}"));
                text.push_str(&format!("{index}\t{opcode:#04x}\t{name}\t{count}\n"));
            }
        }
    }
    match std::env::var("FN64_GBI_CENSUS_OUT") {
        Ok(path) if !path.is_empty() => {
            if let Err(e) = std::fs::write(&path, &text) {
                eprintln!("[FN64_GBI_CENSUS] failed to write {path}: {e}");
                eprint!("{text}");
            } else if announce {
                eprintln!("[FN64_GBI_CENSUS] wrote {} rows to {path}", rows.len());
            }
        }
        // No path configured: stderr is the only sink, so a quiet flush has
        // nowhere quiet to go and is skipped rather than repeated.
        _ if announce => eprint!("{text}"),
        _ => {}
    }
}

/// Per-`G_TEXRECT` cycle-mode and combiner census.
///
/// The opcode histogram above proves co-occurrence but carries no operand
/// data, so it cannot answer the one question that sizes the remaining
/// texture-rectangle work: which RDP cycle mode is latched when the game
/// issues a texture rectangle. A Copy-cycle rectangle is a raw texel blit; a
/// one- or two-cycle rectangle runs the color combiner per fragment, which is
/// a categorically larger implementation.
///
/// The cycle type is read through [`super::types::OtherMode::cycle_type`],
/// the same accessor the decoder itself uses at the `G_TEXRECT` site, so a
/// census row and a decode cannot disagree about `G_MDSFT_CYCLETYPE`.
///
/// Armed by `FN64_GBI_TEXRECT_CENSUS`, independently of [`on`]: the row
/// vector grows once per rectangle, so it is the wrong default for a harness
/// running for millions of steps. `FN64_GBI_TEXRECT_CENSUS_OUT` names the TSV
/// sink; otherwise the rows go to stderr with the main report.
pub mod texrect {
    use super::{AtomicU64, Ordering};
    use crate::gbi::types::{AlphaSource, ColorSource, CombinerCycle, CombinerMode, CycleType};

    #[cfg(not(test))]
    use std::sync::atomic::AtomicBool;

    #[cfg(not(test))]
    static ENABLED: AtomicBool = AtomicBool::new(false);
    #[cfg(not(test))]
    static INIT: AtomicBool = AtomicBool::new(false);

    /// Whether the per-rectangle probe is armed. Always off under
    /// `cfg(test)`, for the same reason [`super::on`] is: a unit test must
    /// not observe an ambient `FN64_*` knob, and rows from one test leaking
    /// into another's report would be worse than no report.
    pub fn on() -> bool {
        #[cfg(test)]
        {
            false
        }
        #[cfg(not(test))]
        {
            if crate::speculative_observations_suppressed() {
                return false;
            }
            if !INIT.swap(true, Ordering::Relaxed) {
                ENABLED.store(
                    crate::debug_flag("FN64_GBI_TEXRECT_CENSUS"),
                    Ordering::Relaxed,
                );
            }
            ENABLED.load(Ordering::Relaxed)
        }
    }

    /// How this rectangle's combiner would have to be evaluated.
    ///
    /// The distinction the sizing question turns on. In Copy and Fill cycle
    /// the RDP does not run the combiner at all, so its programmed state is
    /// irrelevant to the rectangle; in one- and two-cycle it runs per
    /// fragment, and the only cheap case is a combiner whose active cycles
    /// reduce to "emit texel 0 unchanged".
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum CombinerClass {
        /// Cycle mode bypasses the combiner entirely.
        NotEvaluated,
        /// Every active cycle reduces to texel 0, RGB and alpha alike.
        TexelPassthrough,
        /// At least one active cycle mixes something other than texel 0.
        RealWork,
    }

    impl CombinerClass {
        pub fn label(self) -> &'static str {
            match self {
                CombinerClass::NotEvaluated => "not-evaluated",
                CombinerClass::TexelPassthrough => "texel-passthrough",
                CombinerClass::RealWork => "real-work",
            }
        }
    }

    /// Whether one RGB cycle `(A - B) * C + D` is exactly texel 0.
    ///
    /// Two forms qualify, and only two. `D = Texel0` with the product forced
    /// to zero (`C = Zero`, or `A = B` so the difference is zero), and the
    /// direct product form `(Texel0 - Zero) * One + Zero`. Anything else is
    /// mixing, including cases that happen to be constant: this classifies
    /// what a fragment shader would have to compute, not what it evaluates
    /// to.
    fn rgb_cycle_is_texel_passthrough(rgb: &[ColorSource; 4]) -> bool {
        let [a, b, c, d] = *rgb;
        let product_is_zero = c == ColorSource::Zero || a == b;
        if product_is_zero && d == ColorSource::Texel0 {
            return true;
        }
        a == ColorSource::Texel0
            && b == ColorSource::Zero
            && c == ColorSource::One
            && d == ColorSource::Zero
    }

    /// The alpha half of [`rgb_cycle_is_texel_passthrough`], over the alpha
    /// mux's smaller source set.
    fn alpha_cycle_is_texel_passthrough(alpha: &[AlphaSource; 4]) -> bool {
        let [a, b, c, d] = *alpha;
        let product_is_zero = c == AlphaSource::Zero || a == b;
        if product_is_zero && d == AlphaSource::Texel0 {
            return true;
        }
        a == AlphaSource::Texel0
            && b == AlphaSource::Zero
            && c == AlphaSource::One
            && d == AlphaSource::Zero
    }

    fn cycle_is_texel_passthrough(cycle: &CombinerCycle) -> bool {
        rgb_cycle_is_texel_passthrough(&cycle.rgb) && alpha_cycle_is_texel_passthrough(&cycle.alpha)
    }

    /// Classify a latched combiner against the latched cycle mode.
    ///
    /// Only the cycles the mode actually runs are examined: two-cycle mode
    /// runs both, one-cycle runs the first, and Copy/Fill run neither. A
    /// second-cycle program left over from an earlier two-cycle draw is dead
    /// state in one-cycle mode and must not be counted against it.
    pub fn classify(mode: &CombinerMode, cycle_type: CycleType) -> CombinerClass {
        let active = match cycle_type {
            CycleType::OneCycle => 1,
            CycleType::TwoCycle => 2,
            CycleType::Copy | CycleType::Fill => return CombinerClass::NotEvaluated,
        };
        if mode.cycles[..active].iter().all(cycle_is_texel_passthrough) {
            CombinerClass::TexelPassthrough
        } else {
            CombinerClass::RealWork
        }
    }

    /// Spell one active cycle as `(A-B)*C+D` over both muxes.
    ///
    /// A class alone answers "is this cheap"; the sizing question also needs
    /// "which inputs would a fragment shader have to supply", and that is
    /// only readable from the selectors themselves.
    fn spell_cycle(cycle: &CombinerCycle) -> String {
        let [ar, br, cr, dr] = cycle.rgb;
        let [aa, ba, ca, da] = cycle.alpha;
        format!("rgb=({ar:?}-{br:?})*{cr:?}+{dr:?} a=({aa:?}-{ba:?})*{ca:?}+{da:?}")
    }

    /// The programs the mode's active cycles run, joined by `;`. Empty in
    /// Copy and Fill, where no cycle runs.
    pub fn spell(mode: &CombinerMode, cycle_type: CycleType) -> String {
        let active = match cycle_type {
            CycleType::OneCycle => 1,
            CycleType::TwoCycle => 2,
            CycleType::Copy | CycleType::Fill => 0,
        };
        mode.cycles[..active]
            .iter()
            .map(spell_cycle)
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    /// One recorded rectangle: the decode entry it belongs to, its cycle
    /// mode, the raw other-mode high word it was read from, the combiner
    /// classification, and the program that classification came from.
    #[derive(Clone)]
    pub struct Rect {
        pub entry: u64,
        pub cycle_type: CycleType,
        pub other_mode_high: u32,
        pub combiner: CombinerClass,
        pub program: String,
    }

    static ROWS: std::sync::Mutex<Vec<Rect>> = std::sync::Mutex::new(Vec::new());
    /// Rectangles observed, counted whether or not the row vector took them,
    /// so a poisoned lock cannot silently shrink the denominator.
    static SEEN: AtomicU64 = AtomicU64::new(0);

    /// Record one texture rectangle at its decode site.
    ///
    /// `other_mode_high` and `cycle_type` must both come from the same
    /// latched [`super::super::types::OtherMode`] the decoder is about to
    /// build the rectangle from, so the raw word in the row and the decoded
    /// mode in the row are two views of one value, not two reads.
    pub fn note(cycle_type: CycleType, other_mode_high: u32, mode: &CombinerMode) {
        if !on() {
            return;
        }
        SEEN.fetch_add(1, Ordering::Relaxed);
        let row = Rect {
            entry: super::decode_entries(),
            cycle_type,
            other_mode_high,
            combiner: classify(mode, cycle_type),
            program: spell(mode, cycle_type),
        };
        if let Ok(mut guard) = ROWS.lock() {
            guard.push(row);
        }
    }

    pub fn rows() -> Vec<Rect> {
        ROWS.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn seen() -> u64 {
        SEEN.load(Ordering::Relaxed)
    }

    pub fn cycle_label(cycle_type: CycleType) -> &'static str {
        match cycle_type {
            CycleType::OneCycle => "1cycle",
            CycleType::TwoCycle => "2cycle",
            CycleType::Copy => "copy",
            CycleType::Fill => "fill",
        }
    }

    /// Render the per-rectangle census as TSV: a summary comment block, then
    /// one row per rectangle in decode order. Long format on purpose -- the
    /// per-frame question ("does the mode vary across the window?") needs the
    /// entry index kept alongside every rectangle, not aggregated away.
    pub fn render() -> String {
        let rows = rows();
        let mut text = String::new();
        text.push_str("# fn64 G_TEXRECT cycle-mode census\n");
        text.push_str(&format!("# texrects_seen\t{}\n", seen()));
        text.push_str(&format!("# texrects_recorded\t{}\n", rows.len()));
        for cycle_type in [
            CycleType::Fill,
            CycleType::Copy,
            CycleType::OneCycle,
            CycleType::TwoCycle,
        ] {
            let count = rows.iter().filter(|r| r.cycle_type == cycle_type).count();
            text.push_str(&format!("# cycle\t{}\t{count}\n", cycle_label(cycle_type)));
        }
        for class in [
            CombinerClass::NotEvaluated,
            CombinerClass::TexelPassthrough,
            CombinerClass::RealWork,
        ] {
            let count = rows.iter().filter(|r| r.combiner == class).count();
            text.push_str(&format!("# combiner\t{}\t{count}\n", class.label()));
        }
        // Distinct programs, most frequent first. The count of distinct
        // programs is the quantity that sizes a combiner implementation: one
        // program is a special case, many is the general evaluator.
        let mut programs: Vec<(&str, usize)> = Vec::new();
        for row in &rows {
            match programs.iter_mut().find(|(p, _)| *p == row.program) {
                Some((_, count)) => *count += 1,
                None => programs.push((row.program.as_str(), 1)),
            }
        }
        programs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        text.push_str(&format!("# distinct_programs\t{}\n", programs.len()));
        for (program, count) in &programs {
            text.push_str(&format!("# program\t{count}\t{program}\n"));
        }
        text.push_str("entry\tcycle\tother_mode_high\tcombiner\tprogram\n");
        for row in &rows {
            text.push_str(&format!(
                "{}\t{}\t{:#010x}\t{}\t{}\n",
                row.entry,
                cycle_label(row.cycle_type),
                row.other_mode_high,
                row.combiner.label(),
                row.program
            ));
        }
        text
    }

    /// Emit [`render`] to `FN64_GBI_TEXRECT_CENSUS_OUT`, or to stderr when
    /// that names no path. A no-op when the probe is off.
    pub fn report() {
        if !on() {
            return;
        }
        let text = render();
        match std::env::var("FN64_GBI_TEXRECT_CENSUS_OUT") {
            Ok(path) if !path.is_empty() => {
                if let Err(e) = std::fs::write(&path, &text) {
                    eprintln!("[FN64_GBI_TEXRECT_CENSUS] failed to write {path}: {e}");
                    eprint!("{text}");
                }
            }
            _ => eprint!("{text}"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::gbi::types::OtherMode;

        /// The wire encoding of `G_MDSFT_CYCLETYPE`, asserted here as a
        /// literal against the accessor the probe reports through, so a
        /// census row and a decode cannot drift apart. Public `gbi.h`
        /// `G_MDSFT_CYCLETYPE = 20`, `G_CYC_1CYCLE..G_CYC_FILL = 0..3`.
        #[test]
        fn cycle_label_tracks_the_decoders_own_accessor() {
            for (value, label) in [(0u32, "1cycle"), (1, "2cycle"), (2, "copy"), (3, "fill")] {
                let mode = OtherMode::from_raw(value << 20, 0, 0);
                assert_eq!(cycle_label(mode.cycle_type()), label, "cycletype {value}");
            }
        }

        /// Copy and Fill bypass the combiner, so its programmed contents are
        /// not a cost in those modes however busy they look.
        #[test]
        fn copy_and_fill_never_evaluate_the_combiner() {
            let busy = CombinerMode::default();
            for cycle_type in [CycleType::Copy, CycleType::Fill] {
                assert_eq!(
                    classify(&busy, cycle_type),
                    CombinerClass::NotEvaluated,
                    "{cycle_type:?}"
                );
            }
        }

        /// The crate's default combiner is the modulate program
        /// `(Texel0 - Zero) * Shade + Zero`, which is real work: it needs the
        /// shade input a passthrough blit does not have.
        #[test]
        fn the_default_modulate_combiner_is_real_work() {
            assert_eq!(
                classify(&CombinerMode::default(), CycleType::OneCycle),
                CombinerClass::RealWork
            );
        }

        /// Both passthrough spellings are recognized, and each is checked
        /// against a near-miss that differs in exactly one selector -- a
        /// classifier that accepted the near-miss would undercount the real
        /// work this card exists to size.
        #[test]
        fn both_passthrough_forms_are_recognized_and_neither_is_over_broad() {
            let d_form = CombinerCycle {
                rgb: [
                    ColorSource::Combined,
                    ColorSource::Combined,
                    ColorSource::Zero,
                    ColorSource::Texel0,
                ],
                alpha: [
                    AlphaSource::Combined,
                    AlphaSource::Combined,
                    AlphaSource::Zero,
                    AlphaSource::Texel0,
                ],
            };
            assert!(cycle_is_texel_passthrough(&d_form));

            let product_form = CombinerCycle {
                rgb: [
                    ColorSource::Texel0,
                    ColorSource::Zero,
                    ColorSource::One,
                    ColorSource::Zero,
                ],
                alpha: [
                    AlphaSource::Texel0,
                    AlphaSource::Zero,
                    AlphaSource::One,
                    AlphaSource::Zero,
                ],
            };
            assert!(cycle_is_texel_passthrough(&product_form));

            // One selector off in the D form: D is env colour, not texel 0.
            let mut near = d_form;
            near.rgb[3] = ColorSource::Environment;
            assert!(!cycle_is_texel_passthrough(&near));

            // One selector off in the product form: C is shade, so this is
            // modulation.
            let mut near = product_form;
            near.rgb[2] = ColorSource::Shade;
            assert!(!cycle_is_texel_passthrough(&near));

            // RGB passthrough with a mixing alpha is NOT passthrough: the
            // fragment still has to compute something.
            let mut split = d_form;
            split.alpha[3] = AlphaSource::Environment;
            assert!(!cycle_is_texel_passthrough(&split));
        }

        /// The spelling and the classification must be read from the same
        /// cycles. A spelling that showed a cycle the mode does not run
        /// would invite reading real work into a passthrough row.
        #[test]
        fn the_spelling_covers_exactly_the_cycles_the_classifier_examined() {
            let mode = CombinerMode::default();
            assert_eq!(spell(&mode, CycleType::Copy), "");
            assert_eq!(spell(&mode, CycleType::Fill), "");
            assert_eq!(spell(&mode, CycleType::OneCycle).matches(" ; ").count(), 0);
            assert_eq!(spell(&mode, CycleType::TwoCycle).matches(" ; ").count(), 1);
            // The default is the modulate program, and the spelling says so
            // in the same terms `classify` judged it by.
            assert_eq!(
                spell(&mode, CycleType::OneCycle),
                "rgb=(Texel0-Zero)*Shade+Zero a=(Texel0-Zero)*Shade+Zero"
            );
        }

        /// One-cycle mode must ignore a stale second-cycle program, and
        /// two-cycle mode must not. The same combiner classifies differently
        /// under the two modes, which is the whole reason `classify` takes
        /// the cycle type.
        #[test]
        fn only_the_cycles_the_mode_runs_are_examined() {
            let passthrough = CombinerCycle {
                rgb: [
                    ColorSource::Combined,
                    ColorSource::Combined,
                    ColorSource::Zero,
                    ColorSource::Texel0,
                ],
                alpha: [
                    AlphaSource::Combined,
                    AlphaSource::Combined,
                    AlphaSource::Zero,
                    AlphaSource::Texel0,
                ],
            };
            let mut stale_second = passthrough;
            stale_second.rgb[3] = ColorSource::Primitive;
            let mode = CombinerMode {
                cycles: [passthrough, stale_second],
            };
            assert_eq!(
                classify(&mode, CycleType::OneCycle),
                CombinerClass::TexelPassthrough
            );
            assert_eq!(
                classify(&mode, CycleType::TwoCycle),
                CombinerClass::RealWork
            );
        }
    }
}

/// Raw RDP command-word dump for a chosen decode entry.
///
/// The opcode histogram above records *which* commands a decode entry
/// issued and the [`texrect`] probe records their cycle/combiner metadata,
/// but neither carries the command words themselves. A backend cannot be
/// fed a histogram. This module records the exact `(w0, w1)` pairs
/// `decode_stream_impl` dispatched on, so a real packet the game issued can
/// be replayed through a different backend rather than approximated by a
/// synthetic stand-in.
///
/// The words recorded are the ones the decoder dispatched on, taken at the
/// same site [`note`] counts from and from the same `(w0, w1)` bindings, so
/// a histogram row and a dumped word pair cannot disagree about what was
/// decoded. On the raw-RDP lane those are the wire words unmodified
/// (`decode_stream_impl` passes `(wire_w0, wire_w1)` straight through when
/// `raw_rdp`); on the GBI lane they are post-normalization, which is what
/// dispatch saw.
///
/// Armed by `FN64_GBI_PACKET_DUMP`, independently of [`on`], and bounded by
/// construction: `FN64_GBI_PACKET_DUMP_ENTRIES` names the decode entries to
/// capture (comma-separated, zero-based, matching the per-entry delta
/// indices the opcode census reports). Unset means entry 0 only. Nothing
/// outside that set is stored, so the dump does not grow with the run --
/// the reason the [`texrect`] probe needs its own separate knob does not
/// apply here.
///
/// `FN64_GBI_PACKET_DUMP_OUT` names the sink; otherwise the dump goes to
/// stderr.
pub mod packet {
    use super::{AtomicU64, Ordering};

    #[cfg(not(test))]
    use std::sync::atomic::AtomicBool;

    #[cfg(not(test))]
    static ENABLED: AtomicBool = AtomicBool::new(false);
    #[cfg(not(test))]
    static INIT: AtomicBool = AtomicBool::new(false);

    /// Whether the packet dump is armed. Always off under `cfg(test)`, for
    /// the same reason [`super::on`] is: a unit test must not observe an
    /// ambient `FN64_*` knob.
    pub fn on() -> bool {
        #[cfg(test)]
        {
            false
        }
        #[cfg(not(test))]
        {
            if crate::speculative_observations_suppressed() {
                return false;
            }
            if !INIT.swap(true, Ordering::Relaxed) {
                ENABLED.store(crate::debug_flag("FN64_GBI_PACKET_DUMP"), Ordering::Relaxed);
            }
            ENABLED.load(Ordering::Relaxed)
        }
    }

    /// Parse the entry-selection list. Comma-separated zero-based decode
    /// entry indices; an empty or unset value selects entry 0 alone.
    ///
    /// Parsing is total: a malformed element is a hard error rather than a
    /// silently dropped selection, because a dump that quietly captured a
    /// different entry than the operator asked for would be worse than no
    /// dump -- every downstream claim names the entry.
    pub fn parse_entry_selection(raw: &str) -> Vec<u64> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return vec![0];
        }
        trimmed
            .split(',')
            .map(|field| {
                field.trim().parse::<u64>().unwrap_or_else(|e| {
                    panic!(
                        "FN64_GBI_PACKET_DUMP_ENTRIES element {field:?} is not a decode entry \
                         index: {e}"
                    )
                })
            })
            .collect()
    }

    #[cfg(not(test))]
    static SELECTION: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();

    fn selected_entries() -> &'static [u64] {
        #[cfg(test)]
        {
            &[]
        }
        #[cfg(not(test))]
        {
            SELECTION.get_or_init(|| {
                parse_entry_selection(
                    &std::env::var("FN64_GBI_PACKET_DUMP_ENTRIES").unwrap_or_default(),
                )
            })
        }
    }

    /// One dumped command: the decode entry it belongs to, the lane, the
    /// RDRAM address the word pair was read from, and the pair itself.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Command {
        pub entry: u64,
        pub raw_rdp: bool,
        pub pc: usize,
        pub w0: u32,
        pub w1: u32,
    }

    static ROWS: std::sync::Mutex<Vec<Command>> = std::sync::Mutex::new(Vec::new());
    /// Commands the dump was offered, counted whether or not the row vector
    /// took them, so a poisoned lock cannot silently shrink the denominator.
    static OFFERED: AtomicU64 = AtomicU64::new(0);

    /// Record one dispatched command's word pair, if its decode entry is
    /// selected.
    ///
    /// `w0`/`w1` must be the same bindings `decode_stream_impl` dispatched
    /// on -- not a re-read of RDRAM -- so the dumped bytes and the counted
    /// opcode are two views of one value rather than two reads that could
    /// diverge.
    pub fn note(pc: usize, w0: u32, w1: u32, raw_rdp: bool) {
        if !on() {
            return;
        }
        let entry = super::decode_entries().saturating_sub(1);
        if !selected_entries().contains(&entry) {
            return;
        }
        OFFERED.fetch_add(1, Ordering::Relaxed);
        let row = Command {
            entry,
            raw_rdp,
            pc,
            w0,
            w1,
        };
        if let Ok(mut guard) = ROWS.lock() {
            guard.push(row);
        }
    }

    /// Record a variable-width command's continuation words.
    ///
    /// [`note`] fires at the dispatch site, which sees only the leading
    /// `(w0, w1)` pair -- but `G_TEXRECT` is 16 bytes, and its second pair
    /// carries the S/T origin and the per-pixel gradients. A dump missing
    /// them would replay as a rectangle with fabricated texture
    /// coordinates, which is a synthetic stand-in wearing a real packet's
    /// opcode. Called from the arm that decodes the continuation, with the
    /// same words that arm decoded.
    ///
    /// The continuation is appended as its own row at `pc + 8`, so the
    /// dumped rows reconstruct the wire byte stream in address order with
    /// no gaps -- the property a replay depends on and that the reader can
    /// check from the `pc` column alone.
    pub fn note_continuation(pc: usize, w2: u32, w3: u32, raw_rdp: bool) {
        note(pc, w2, w3, raw_rdp);
    }

    pub fn rows() -> Vec<Command> {
        ROWS.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn offered() -> u64 {
        OFFERED.load(Ordering::Relaxed)
    }

    /// Render the dump as TSV: a summary comment block, then one row per
    /// command in dispatch order.
    ///
    /// The word pair is the payload; `pc` and the entry index are what make
    /// a row auditable against the opcode census, which reports per-entry
    /// deltas over the same entry numbering.
    pub fn render() -> String {
        let rows = rows();
        let mut text = String::new();
        text.push_str("# fn64 raw RDP command-word dump\n");
        text.push_str(&format!("# commands_offered\t{}\n", offered()));
        text.push_str(&format!("# commands_recorded\t{}\n", rows.len()));
        let selection: Vec<String> = selected_entries().iter().map(u64::to_string).collect();
        text.push_str(&format!("# entries_selected\t{}\n", selection.join(",")));
        for entry in selected_entries() {
            let count = rows.iter().filter(|r| r.entry == *entry).count();
            text.push_str(&format!("# entry\t{entry}\t{count}\n"));
        }
        text.push_str("entry\tlane\tpc\tw0\tw1\n");
        for row in &rows {
            text.push_str(&format!(
                "{}\t{}\t{:#010x}\t{:#010x}\t{:#010x}\n",
                row.entry,
                if row.raw_rdp { "RDP" } else { "GBI" },
                row.pc,
                row.w0,
                row.w1
            ));
        }
        text
    }

    /// Emit [`render`] to `FN64_GBI_PACKET_DUMP_OUT`, or to stderr when that
    /// names no path. A no-op when the dump is off.
    pub fn report() {
        if !on() {
            return;
        }
        let text = render();
        match std::env::var("FN64_GBI_PACKET_DUMP_OUT") {
            Ok(path) if !path.is_empty() => {
                if let Err(e) = std::fs::write(&path, &text) {
                    eprintln!("[FN64_GBI_PACKET_DUMP] failed to write {path}: {e}");
                    eprint!("{text}");
                }
            }
            _ => eprint!("{text}"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The default selection is entry 0 alone, and it is reached by an
        /// unset var and by an empty one alike -- an operator who exports
        /// the knob to the empty string gets the documented default rather
        /// than an empty selection that captures nothing.
        #[test]
        fn an_absent_or_empty_selection_means_entry_zero() {
            assert_eq!(parse_entry_selection(""), vec![0]);
            assert_eq!(parse_entry_selection("   "), vec![0]);
        }

        /// A list is parsed in the order written, duplicates and all: the
        /// dump's `# entry` summary lines are emitted per selection element,
        /// so reordering them would reorder the report.
        #[test]
        fn a_selection_list_parses_in_order() {
            assert_eq!(parse_entry_selection("0,1,2"), vec![0, 1, 2]);
            assert_eq!(parse_entry_selection(" 7 , 3 "), vec![7, 3]);
            assert_eq!(parse_entry_selection("5"), vec![5]);
        }

        /// A malformed element aborts rather than being dropped. A dropped
        /// element would silently capture a different entry set than the
        /// operator named, and every claim downstream names the entry.
        #[test]
        #[should_panic(expected = "is not a decode entry index")]
        fn a_malformed_selection_element_is_a_hard_error() {
            parse_entry_selection("0,notanumber");
        }

        /// The probe is inert under `cfg(test)`, so no unit test in this
        /// crate can be perturbed by an ambient `FN64_GBI_PACKET_DUMP` in
        /// the developer's environment.
        #[test]
        fn the_probe_is_off_under_cfg_test() {
            assert!(!on());
            note(0x1000, 0xdead_beef, 0xfeed_face, true);
            assert_eq!(offered(), 0);
            assert!(rows().is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_bytes_are_distinct_within_a_lane() {
        // A duplicated name would silently merge two opcodes in a report,
        // which is the one failure mode a census cannot survive: the reader
        // would act on a count attributed to the wrong command.
        for namer in [
            gbi_name as fn(u8) -> Option<&'static str>,
            raw_rdp_lane_name as fn(u8) -> Option<&'static str>,
        ] {
            let mut seen: Vec<&'static str> = (0u16..256)
                .filter_map(|b| namer(b as u8))
                // The bare RDP No Operation block is eight bytes with one
                // meaning; every other name must be unique.
                .filter(|n| *n != "RDP_NOOP")
                .collect();
            let before = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(before, seen.len(), "duplicate opcode name within a lane");
        }
    }

    #[test]
    fn raw_rdp_lane_names_the_whole_low_triangle_block() {
        for opcode in 0x08u8..=0x0f {
            let name = raw_rdp_lane_name(opcode).expect("low RDP block is fully named");
            assert!(name.starts_with("RDP_TRI"), "{opcode:#04x} named {name}");
        }
    }

    #[test]
    fn the_two_lanes_disagree_where_the_wire_spellings_disagree() {
        // The load-bearing reason the census keeps two tables. `0x05` is
        // `G_TRI1` on the GBI wire and an RDP No Operation after
        // `canonical_raw_rdp_opcode`; a single table would report one of
        // them under the other's name.
        assert_eq!(gbi_name(0x05), Some("G_TRI1"));
        assert_eq!(raw_rdp_lane_name(0x05), Some("RDP_NOOP"));
        // Above the low block they must agree, or a reader comparing lanes
        // would see two names for one command.
        for opcode in 0x10u16..=0xff {
            let opcode = opcode as u8;
            assert_eq!(gbi_name(opcode), raw_rdp_lane_name(opcode), "{opcode:#04x}");
        }
    }

    #[test]
    fn unnamed_bytes_are_reported_as_unnamed_not_guessed() {
        // 0xd0 has no arm in `state::opcode_name`; the census must say so
        // rather than inherit its `G_<unrecognized>` placeholder, which
        // would collapse every unknown byte into one row.
        assert!(gbi_name(0xd0).is_none());
        assert!(raw_rdp_lane_name(0xd0).is_none());
    }
}
