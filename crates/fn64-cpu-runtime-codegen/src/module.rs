//! The ELF/symbol-table **front-end**: turn a whole [`RecompConfig`]
//! (sections + functions) into a single linkable Rust module whose emitted
//! functions call each other **directly** by name — the thing that makes
//! `fn64-cpu-runtime` a *whole-program* recompiler and not a bag of
//! per-function `lookup()` stubs.
//!
//! # What it does (mirrors N64Recomp's `resolve_jal`)
//!
//! N64Recomp resolves a `JAL 0xNNN` by looking the target vram up in its
//! `functions_by_vram` map (`recompilation.cpp::resolve_jal`): a unique known
//! function symbol becomes a **direct call** to that named C function; an
//! ambiguous/unknown target falls back to a runtime `LOOKUP_FUNC` dispatch.
//! We reproduce that decision in pure typed Rust:
//!
//! 1. [`SymbolTable::from_config`] builds a `vram -> function name` map from
//!    every function in every section of the config. A vram claimed by more
//!    than one distinct name is *ambiguous* and deliberately left unresolved
//!    (emitted indirect), exactly like N64Recomp's multi-match `Ambiguous`
//!    case — we never silently pick one. Its claimants are retained with
//!    their section indices so the emitted dispatcher can still reach them
//!    (see `BANKED_LOOKUP_TABLE` in [`emit_lookup_dispatcher`]): overlay banks
//!    genuinely share a VRAM window, and dropping those bodies entirely would
//!    make them undispatchable rather than merely un-direct-callable.
//! 2. As a [`CallResolver`], it turns a `JAL`/`J` target vram into either a
//!    [`CallTarget::Direct`] (unique symbol) or [`CallTarget::Indirect`]
//!    (unknown or ambiguous), which the emitter renders as a direct
//!    `name(ctx, mem)` call or a `lookup(addr)(ctx, mem)` dispatch.
//! 3. [`emit_module`] runs the resolver over every function and concatenates
//!    the bodies into one module — a linkable unit with real cross-function
//!    calls.
//!
//! # Clean-room note
//!
//! Only the *structure* of the vram->symbol resolution is taken from the MIT
//! N64Recomp source (public algorithm). No C is copied; the emitted output is
//! typed Rust with no `unsafe` and no pointer casts, upholding the crate's
//! `#![forbid(unsafe_code)]` guarantee.

use std::collections::HashMap;

use crate::emit::{emit_function_resolved, CallResolver, CallTarget, FuncInput};

/// A `vram -> function name` resolver built from a whole config's symbol table.
///
/// Implements [`CallResolver`] so the emitter can turn `JAL`/`J` targets into
/// direct named calls. Ambiguous vrams (claimed by two different names) resolve
/// to [`CallTarget::Indirect`] — the same conservative choice N64Recomp's
/// `resolve_jal` makes for its multi-match case.
#[derive(Clone, Debug, Default)]
pub struct SymbolTable {
    /// Unique vram -> name. A vram present here has exactly one owner.
    by_vram: HashMap<u32, String>,
    /// Vrams seen more than once with conflicting names; kept so a direct call
    /// is never emitted for them (they fall through to indirect lookup).
    ambiguous: std::collections::HashSet<u32>,
    /// Every claimant of an ambiguous vram, keyed by vram: `(section, name)`
    /// for each distinct body that links at that address.
    ///
    /// Bank-switched overlays genuinely share a VRAM window (WM2000's
    /// `bank1_text` and `bank4_text` both link at `0x800E1B90`), so dropping
    /// the collided vrams from `by_vram` is right for *direct* calls but
    /// would make the bodies undispatchable if the information were also
    /// discarded here. Retaining the claimants is what lets
    /// [`emit_lookup_dispatcher`] resolve them at runtime against the bank
    /// the guest actually has resident.
    claimants: HashMap<u32, Vec<(usize, String)>>,
}

impl SymbolTable {
    /// Build a symbol table from an iterator of `(name, vram)` entries.
    ///
    /// A vram that appears twice with the *same* name is fine (idempotent). A
    /// vram that appears with two *different* names is marked ambiguous and
    /// removed from the direct-call map.
    pub fn from_entries<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = (S, u32)>,
        S: Into<String>,
    {
        // Section 0 for every entry: a caller with no section information
        // cannot describe a bank collision, so every claimant is attributed to
        // one nominal section and bank-aware dispatch is unavailable. Callers
        // that know their sections use `from_section_entries`.
        SymbolTable::from_section_entries(
            entries
                .into_iter()
                .map(|(name, vram)| (0usize, name.into(), vram)),
        )
    }

    /// Build a symbol table from `(section_index, name, vram)` entries.
    ///
    /// Identical to [`SymbolTable::from_entries`] for the unique and
    /// same-name-twice cases. The difference is what happens on a genuine
    /// collision: the vram still leaves `by_vram` (no direct call is ever
    /// emitted for it), but every distinct claimant is retained with the
    /// section that owns it, so the runtime dispatcher can resolve the
    /// address against the bank the guest currently has PI-swapped in.
    ///
    /// `section_index` must be the section's registration index -- the same
    /// numbering `RECOMPILED_SECTION_GEOMETRY` is emitted in and that the
    /// host's `SectionRegistry` assigns -- because that index is the only
    /// thing tying an emitted claimant back to a residency bit.
    pub fn from_section_entries<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = (usize, S, u32)>,
        S: Into<String>,
    {
        let mut by_vram: HashMap<u32, String> = HashMap::new();
        let mut ambiguous = std::collections::HashSet::new();
        let mut claimants: HashMap<u32, Vec<(usize, String)>> = HashMap::new();
        for (section, name, vram) in entries {
            let name = name.into();
            let seen = claimants.entry(vram).or_default();
            if !seen.iter().any(|(s, n)| *s == section && *n == name) {
                seen.push((section, name.clone()));
            }
            match by_vram.get(&vram) {
                Some(existing) if *existing == name => {}
                Some(_) => {
                    ambiguous.insert(vram);
                    by_vram.remove(&vram);
                }
                None => {
                    if !ambiguous.contains(&vram) {
                        by_vram.insert(vram, name);
                    }
                }
            }
        }
        // Only collisions need claimant records; uniquely-owned vrams resolve
        // through the flat table and would only bloat the emitted source.
        claimants.retain(|vram, _| ambiguous.contains(vram));
        for entries in claimants.values_mut() {
            entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        }
        SymbolTable {
            by_vram,
            ambiguous,
            claimants,
        }
    }

    /// Every `(vram, claimants)` pair for a vram claimed by more than one
    /// differently-named body, in ascending vram order, each claimant list in
    /// ascending section order.
    ///
    /// These are exactly the bodies a flat `vram -> fn` table cannot express.
    /// Reported by the gap report and emitted as the banked dispatch table.
    pub fn ambiguous_claimants(&self) -> Vec<(u32, Vec<(usize, &str)>)> {
        let mut rows = self
            .claimants
            .iter()
            .map(|(&vram, entries)| {
                (
                    vram,
                    entries
                        .iter()
                        .map(|(section, name)| (*section, name.as_str()))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        rows.sort_unstable_by_key(|(vram, _)| *vram);
        rows
    }

    /// Number of distinct bodies that are only reachable through bank-aware
    /// dispatch (the sum of every ambiguous vram's claimant count).
    pub fn banked_body_count(&self) -> usize {
        self.claimants.values().map(Vec::len).sum()
    }

    /// Build from a [`fn64_recomp::RecompConfig`]: every function in every
    /// section contributes its `(name, vram)` pair.
    pub fn from_config(cfg: &fn64_recomp::RecompConfig) -> Self {
        let entries = cfg.sections.iter().enumerate().flat_map(|(index, s)| {
            s.functions
                .iter()
                .map(move |f| (index, f.name.clone(), f.vram))
        });
        SymbolTable::from_section_entries(entries)
    }

    /// The name owning `vram`, if it is a unique function entry.
    pub fn name_of(&self, vram: u32) -> Option<&str> {
        self.by_vram.get(&vram).map(String::as_str)
    }

    /// Whether `vram` was claimed by two different names (left indirect).
    pub fn is_ambiguous(&self, vram: u32) -> bool {
        self.ambiguous.contains(&vram)
    }

    /// Number of uniquely-resolvable symbols.
    pub fn len(&self) -> usize {
        self.by_vram.len()
    }

    /// Whether there are no uniquely-resolvable symbols.
    pub fn is_empty(&self) -> bool {
        self.by_vram.is_empty()
    }

    /// Unique `(vram, name)` entries in ascending vram order.
    ///
    /// `HashMap` iteration order must never leak into generated source. The
    /// dispatcher table is binary-searched, so sorted order is both part of
    /// its correctness and what makes output byte-for-byte reproducible.
    pub fn entries_by_vram(&self) -> Vec<(u32, &str)> {
        let mut entries = self
            .by_vram
            .iter()
            .map(|(&vram, name)| (vram, name.as_str()))
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(vram, _)| *vram);
        entries
    }
}

impl CallResolver for SymbolTable {
    fn resolve(&self, target_vram: u32) -> CallTarget {
        match self.name_of(target_vram) {
            Some(name) => CallTarget::Direct(name.to_string()),
            None => CallTarget::Indirect,
        }
    }
}

/// One function's identity + its already-read instruction words, ready to
/// recompile as part of a module.
pub struct ModuleFunc<'a> {
    pub name: &'a str,
    pub vram: u32,
    pub words: &'a [u32],
}

/// Emit the safe `vram -> RecompFunc` indirect-call dispatcher.
///
/// N64Recomp's `LOOKUP_FUNC(val)` calls `get_function((int32_t) val)` and
/// receives a `recomp_func_t*` (`refs/N64RecompSource/include/recomp.h:443-451`).
/// The typed-Rust equivalent is a sorted static slice of ordinary Rust
/// function pointers plus `binary_search_by_key`. Host-owned functions are
/// resolved first through [`crate::resolve_host_function`]; callers exclude
/// those vrams from `symbols`, which makes both direct `JAL`s and computed
/// `JALR`s converge on the same host seam instead of a recompiled panic body.
///
/// A vram claimed by two or more overlay banks cannot live in the flat table,
/// so it is emitted into a second sorted table, `BANKED_LOOKUP_TABLE`, that
/// keeps every claimant with the section index owning it.
/// `fn64_cpu_runtime::resolve_banked_function` resolves it against the bank
/// the guest actually has resident; zero or multiple resident claimants are
/// both named traps, never a first-claimant default.
///
/// Unknown vrams trap with the exact address. There is no default function,
/// pointer cast, `transmute`, or `unsafe` block.
pub fn emit_lookup_dispatcher(symbols: &SymbolTable) -> String {
    let mut out = String::new();
    out.push_str("// Safe LOOKUP_FUNC/get_function equivalent: sorted vram -> typed fn table.\n");
    out.push_str("static LOOKUP_TABLE: &[(u32, RecompFunc)] = &[\n");
    for (vram, name) in symbols.entries_by_vram() {
        out.push_str(&format!("    ({vram:#010X}, {name} as RecompFunc),\n"));
    }
    out.push_str("];\n\n");

    // Bank-switched overlays share a VRAM window, so these vrams cannot live
    // in the flat table: two differently-named bodies claim one address and
    // only the resident bank's is correct. Each row carries every claimant
    // with the section index that owns it; `resolve_banked_function` picks the
    // resident one and traps when residency does not name exactly one.
    let banked = symbols.ambiguous_claimants();
    out.push_str(
        "// Vrams claimed by more than one overlay bank: resolved against guest residency.\n",
    );
    out.push_str(
        "static BANKED_LOOKUP_TABLE: &[(u32, &[(usize, &'static str, RecompFunc)])] = &[\n",
    );
    for (vram, claimants) in &banked {
        out.push_str(&format!("    ({vram:#010X}, &["));
        for (section, name) in claimants {
            out.push_str(&format!("({section}, \"{name}\", {name} as RecompFunc), "));
        }
        out.push_str("]),\n");
    }
    out.push_str("];\n\n");

    out.push_str("pub fn lookup(vram: u32) -> RecompFunc {\n");
    out.push_str("    if let Some(func) = resolve_host_function(vram) {\n");
    out.push_str("        return func;\n");
    out.push_str("    }\n");
    out.push_str("    match LOOKUP_TABLE.binary_search_by_key(&vram, |(addr, _)| *addr) {\n");
    out.push_str("        Ok(index) => LOOKUP_TABLE[index].1,\n");
    out.push_str("        Err(_) => match BANKED_LOOKUP_TABLE.binary_search_by_key(&vram, |(addr, _)| *addr) {\n");
    out.push_str("            Ok(index) => fn64_cpu_runtime::resolve_banked_function(vram, BANKED_LOOKUP_TABLE[index].1),\n");
    out.push_str("            Err(_) => fn64_cpu_runtime::trap_unsupported(format!(\"lookup: no recompiled function or host shim at vram {vram:#010X}\")),\n");
    out.push_str("        },\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    out
}

/// Emit a whole linkable module from a set of functions plus the symbol table
/// that resolves their inter-function calls.
///
/// Each function is recompiled with [`emit_function_resolved`] against
/// `symbols`, so a `JAL`/`J` to any function in the table becomes a **direct**
/// Rust call. The result is a single `String` containing every function body,
/// prefixed with the module header (imports + a note). It is a linkable unit:
/// compiling it defines every `fn` and every cross-function call is a real
/// Rust call to another `fn` in the same module.
pub fn emit_module(funcs: &[ModuleFunc], symbols: &SymbolTable) -> String {
    let mut out = String::new();
    out.push_str("// Generated by fn64-cpu-runtime (ELF/symbol front-end).\n");
    out.push_str("// Whole-program: inter-function JAL/J resolve to direct Rust calls.\n");
    out.push_str("// Typed Rust, no unsafe, no pointer casts.\n");
    out.push_str("#![allow(clippy::all, unused, non_snake_case)]\n");
    out.push_str("pub const FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA: fn64_cpu_runtime::FunctionEntryObservationSchema = fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA;\n");
    out.push_str(
        "use fn64_cpu_runtime::{call_host_or_recompiled, pause_self, resolve_host_function, RecompContext, RecompFunc, Rdram};\n\n",
    );
    for f in funcs {
        let input = FuncInput {
            name: f.name,
            vram: f.vram,
            words: f.words,
        };
        out.push_str(&emit_function_resolved(&input, symbols));
        out.push('\n');
    }
    out.push_str(&emit_lookup_dispatcher(symbols));
    out
}


/// One statically-known call target that the emitted module can never
/// dispatch: a `JAL`/`J` immediate whose vram carries no function symbol, yet
/// falls strictly **inside** the span of a function that IS emitted.
///
/// Such a target is emitted as `lookup(addr)`, but `addr` reaches neither
/// `LOOKUP_TABLE` nor `BANKED_LOOKUP_TABLE` (both hold function ENTRY vrams
/// only), so the call traps at run time via `trap_unsupported` -- arbitrarily
/// far into the run, whenever the guest first takes that path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndispatchableCallTarget {
    /// The unreachable target vram (the `lookup()` argument that will trap).
    pub target: u32,
    /// Name of the emitted function whose declared span contains `target`.
    pub containing_function: String,
    /// Entry vram of that containing function.
    pub containing_vram: u32,
    /// Every emitted function that contains a static call to `target`.
    pub callers: Vec<String>,
}

/// Audit a module's functions for statically-known call targets that resolve
/// indirect and are undispatchable (see [`UndispatchableCallTarget`]).
///
/// This is a **whole-module** property: neither the per-function emitter nor
/// the dispatcher emitter can see it, because each half is individually
/// correct. The emitter is right that an unknown vram must become `lookup()`;
/// the dispatcher is right that only entry points belong in its tables. What
/// is wrong is the *symbol table*, which failed to declare a function entry
/// that the machine code plainly calls -- typically because the upstream
/// symbol source mislabeled it, so the predecessor's declared size swallowed
/// it.
///
/// Detecting it here converts a nondeterministic mid-run abort into a
/// deterministic, named build-time finding that points at the exact missing
/// symbol boundary.
///
/// Returns the findings ordered by target vram. An empty vector means every
/// static call target in `funcs` is dispatchable.
pub fn audit_undispatchable_call_targets(
    funcs: &[ModuleFunc],
    symbols: &SymbolTable,
) -> Vec<UndispatchableCallTarget> {
    use fn64_cpu_runtime::decode;

    // Spans of every emitted function, from its declared word count.
    let spans: Vec<(u32, u32, &str)> = funcs
        .iter()
        .map(|f| (f.vram, f.vram + (f.words.len() as u32) * 4, f.name))
        .collect();

    // target -> (containing function, callers)
    let mut found: HashMap<u32, (String, u32, Vec<String>)> = HashMap::new();

    for f in funcs {
        for (i, &w) in f.words.iter().enumerate() {
            let vram = f.vram + (i as u32) * 4;
            let instr = decode(w);
            // Only absolute JAL/J immediates carry a statically-known target.
            let target = match instr {
                fn64_cpu_runtime::Instruction::J { target }
                | fn64_cpu_runtime::Instruction::Jal { target } => {
                    (vram.wrapping_add(4) & 0xF000_0000) | (target << 2)
                }
                _ => continue,
            };
            // A target the symbol table resolves is emitted as a direct call.
            if symbols.resolve(target) != CallTarget::Indirect {
                continue;
            }
            // An ambiguous vram is a real entry reachable via the banked
            // table; that is dispatchable, not a gap.
            if symbols.is_ambiguous(target) {
                continue;
            }
            // A target inside a function's span -- but not its entry -- can
            // never be reached by an entry-keyed table.
            let Some(&(cv, _, cname)) = spans
                .iter()
                .find(|&&(start, end, _)| target > start && target < end)
            else {
                continue;
            };
            let row = found
                .entry(target)
                .or_insert_with(|| (cname.to_string(), cv, Vec::new()));
            if !row.2.iter().any(|c| c == f.name) {
                row.2.push(f.name.to_string());
            }
        }
    }

    let mut out: Vec<UndispatchableCallTarget> = found
        .into_iter()
        .map(|(target, (containing_function, containing_vram, mut callers))| {
            callers.sort();
            UndispatchableCallTarget {
                target,
                containing_function,
                containing_vram,
                callers,
            }
        })
        .collect();
    out.sort_unstable_by_key(|f| f.target);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode `JAL <target>` (opcode 3) exactly as the ROM does.
    fn jal(target: u32) -> u32 {
        (3u32 << 26) | ((target & 0x0FFF_FFFF) >> 2)
    }

    /// The WM2000 swap-1901 abort, reduced to its exact shape: a caller makes
    /// a static `JAL` to an address that is a real function entry in the ROM
    /// but carries no symbol, so the predecessor's declared size swallows it.
    /// The emitted call becomes `lookup(target)`, and no table holds a
    /// non-entry vram, so it traps at run time.
    #[test]
    fn interior_static_call_target_is_reported_as_undispatchable() {
        // The real `func_8012079C_bank3_text` is declared 75 words long, so
        // its span runs 0x8012079C..0x801208C8 and swallows 0x80120854.
        let swallower_words = [0u32; 75];
        // The caller does `JAL 0x80120854` -- inside `swallower`, not its entry.
        let caller_words = [jal(0x8012_0854), 0];
        let funcs = [
            ModuleFunc {
                name: "func_8012079C",
                vram: 0x8012_079C,
                words: &swallower_words,
            },
            ModuleFunc {
                name: "func_801206D0",
                vram: 0x8012_06D0,
                words: &caller_words,
            },
        ];
        let symbols = SymbolTable::from_entries([
            ("func_8012079C", 0x8012_079Cu32),
            ("func_801206D0", 0x8012_06D0u32),
        ]);

        // The emitter really does render this as an undispatchable lookup.
        let module = emit_module(&funcs, &symbols);
        assert!(
            module.contains("lookup(0x80120854)"),
            "expected the swallowed target to be emitted as a lookup"
        );
        assert!(
            !module.contains("(0x80120854, func_"),
            "the swallowed target must NOT be in the dispatch table"
        );

        let findings = audit_undispatchable_call_targets(&funcs, &symbols);
        assert_eq!(
            findings,
            vec![UndispatchableCallTarget {
                target: 0x8012_0854,
                containing_function: "func_8012079C".to_string(),
                containing_vram: 0x8012_079C,
                callers: vec!["func_801206D0".to_string()],
            }]
        );
    }

    /// The audit must not fire on the ordinary cases. A call to a declared
    /// entry is direct; a call to an address outside every emitted span is a
    /// host shim or a genuinely absent function, not a swallowed entry; and a
    /// call to a function's own entry is dispatchable by definition.
    #[test]
    fn dispatchable_static_call_targets_are_not_reported() {
        let callee_words = [0u32, 0];
        // Calls: a declared entry, an address beyond every span, and an entry.
        let caller_words = [jal(0x8012_0900), jal(0x8000_2000), jal(0x8012_0900)];
        let funcs = [
            ModuleFunc {
                name: "callee",
                vram: 0x8012_0900,
                words: &callee_words,
            },
            ModuleFunc {
                name: "caller",
                vram: 0x8012_0A00,
                words: &caller_words,
            },
        ];
        let symbols =
            SymbolTable::from_entries([("callee", 0x8012_0900u32), ("caller", 0x8012_0A00u32)]);
        assert_eq!(audit_undispatchable_call_targets(&funcs, &symbols), vec![]);
    }

    /// A vram claimed by two overlay banks IS dispatchable -- through
    /// `BANKED_LOOKUP_TABLE` -- so the audit must not confuse a bank
    /// collision (correctly handled) with a swallowed entry (the defect).
    #[test]
    fn bank_ambiguous_interior_target_is_not_reported() {
        let host_words = [0u32; 8];
        let caller_words = [jal(0x800E_1B90), 0];
        let funcs = [
            ModuleFunc {
                name: "spanning",
                vram: 0x800E_1B88,
                words: &host_words,
            },
            ModuleFunc {
                name: "caller",
                vram: 0x800E_2000,
                words: &caller_words,
            },
        ];
        // 0x800E1B90 is a real entry claimed by two banks, and it sits inside
        // `spanning`'s declared span -- the audit must still let it pass.
        let symbols = SymbolTable::from_section_entries([
            (2usize, "func_800E1B90", 0x800E_1B90u32),
            (5usize, "func_800E1B90_bank4_text", 0x800E_1B90u32),
            (1usize, "spanning", 0x800E_1B88u32),
            (1usize, "caller", 0x800E_2000u32),
        ]);
        assert_eq!(audit_undispatchable_call_targets(&funcs, &symbols), vec![]);
    }

    /// Every caller of one swallowed target is reported, de-duplicated and
    /// sorted, so the finding names the whole repair surface at once.
    #[test]
    fn all_callers_of_one_swallowed_target_are_collected() {
        let swallower_words = [0u32; 75];
        let a_words = [jal(0x8012_0854), 0, jal(0x8012_0854)];
        let b_words = [jal(0x8012_0854), 0];
        let funcs = [
            ModuleFunc {
                name: "swallower",
                vram: 0x8012_079C,
                words: &swallower_words,
            },
            ModuleFunc {
                name: "zeta_caller",
                vram: 0x8012_0900,
                words: &a_words,
            },
            ModuleFunc {
                name: "alpha_caller",
                vram: 0x8012_0A00,
                words: &b_words,
            },
        ];
        let symbols = SymbolTable::from_entries([
            ("swallower", 0x8012_079Cu32),
            ("zeta_caller", 0x8012_0900u32),
            ("alpha_caller", 0x8012_0A00u32),
        ]);
        let findings = audit_undispatchable_call_targets(&funcs, &symbols);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].callers,
            vec!["alpha_caller".to_string(), "zeta_caller".to_string()]
        );
    }

    #[test]
    fn ambiguous_vram_is_left_indirect() {
        // Same vram, two different names -> ambiguous, not directly resolvable.
        let t = SymbolTable::from_entries([("foo", 0x8000_0100u32), ("bar", 0x8000_0100u32)]);
        assert!(t.is_ambiguous(0x8000_0100));
        assert_eq!(t.name_of(0x8000_0100), None);
        assert_eq!(t.resolve(0x8000_0100), CallTarget::Indirect);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn collided_vram_keeps_every_claimant_with_its_section() {
        // Two banks at one vram (WM2000's 0x800E1B90 shape), plus an
        // uncontested address to prove only collisions are retained.
        let t = SymbolTable::from_section_entries([
            (2usize, "func_800E1B90", 0x800E_1B90u32),
            (5usize, "func_800E1B90_bank4_text", 0x800E_1B90u32),
            (1usize, "resident_only", 0x8000_0450u32),
        ]);
        assert!(t.is_ambiguous(0x800E_1B90));
        assert_eq!(t.name_of(0x800E_1B90), None);
        assert_eq!(
            t.ambiguous_claimants(),
            vec![(
                0x800E_1B90,
                vec![(2, "func_800E1B90"), (5, "func_800E1B90_bank4_text"),]
            )]
        );
        assert_eq!(t.banked_body_count(), 2);
        // Uncontested vrams stay in the flat table and out of the claimants.
        assert_eq!(t.name_of(0x8000_0450), Some("resident_only"));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn same_name_in_two_sections_is_not_a_collision() {
        // A name reported twice for one vram is idempotent regardless of the
        // section it came from: that is one body, not two banks.
        let t = SymbolTable::from_section_entries([
            (2usize, "func_800E1B90", 0x800E_1B90u32),
            (5usize, "func_800E1B90", 0x800E_1B90u32),
        ]);
        assert!(!t.is_ambiguous(0x800E_1B90));
        assert_eq!(t.name_of(0x800E_1B90), Some("func_800E1B90"));
        assert!(t.ambiguous_claimants().is_empty());
        assert_eq!(t.banked_body_count(), 0);
    }

    #[test]
    fn banked_table_is_sorted_and_binary_searchable() {
        // The emitted dispatcher binary-searches BANKED_LOOKUP_TABLE, so its
        // rows must be vram-ascending and its claimants section-ascending —
        // both independent of the HashMap iteration order they came from.
        let t = SymbolTable::from_section_entries([
            (5usize, "high_b", 0x8011_C900u32),
            (3usize, "high_a", 0x8011_C900u32),
            (5usize, "low_b", 0x800E_1B90u32),
            (2usize, "low_a", 0x800E_1B90u32),
        ]);
        let rows = t.ambiguous_claimants();
        assert_eq!(
            rows,
            vec![
                (0x800E_1B90, vec![(2, "low_a"), (5, "low_b")]),
                (0x8011_C900, vec![(3, "high_a"), (5, "high_b")]),
            ]
        );
        assert_eq!(t.banked_body_count(), 4);
    }

    #[test]
    fn duplicate_same_name_is_idempotent() {
        let t = SymbolTable::from_entries([("foo", 0x8000_0100u32), ("foo", 0x8000_0100u32)]);
        assert_eq!(t.name_of(0x8000_0100), Some("foo"));
        assert!(!t.is_ambiguous(0x8000_0100));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn unique_symbol_resolves_direct() {
        let t = SymbolTable::from_entries([("callee", 0x8000_0200u32)]);
        assert_eq!(
            t.resolve(0x8000_0200),
            CallTarget::Direct("callee".to_string())
        );
        // An unknown target stays indirect.
        assert_eq!(t.resolve(0x8000_9999), CallTarget::Indirect);
    }

    #[test]
    fn dispatcher_entries_are_sorted_by_vram() {
        let t = SymbolTable::from_entries([
            ("last", 0x8000_0300u32),
            ("first", 0x8000_0100u32),
            ("middle", 0x8000_0200u32),
        ]);
        assert_eq!(
            t.entries_by_vram(),
            vec![
                (0x8000_0100, "first"),
                (0x8000_0200, "middle"),
                (0x8000_0300, "last"),
            ]
        );
    }
}
