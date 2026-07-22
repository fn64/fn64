//! The ELF/symbol-table **front-end**: turn a whole [`RecompConfig`]
//! (sections + functions) into a single linkable Rust module whose emitted
//! functions call each other **directly** by name — the thing that makes
//! `fn64-recomp-rs` a *whole-program* recompiler and not a bag of
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
//!    case — we never silently pick one.
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
        let mut by_vram: HashMap<u32, String> = HashMap::new();
        let mut ambiguous = std::collections::HashSet::new();
        for (name, vram) in entries {
            let name = name.into();
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
        SymbolTable { by_vram, ambiguous }
    }

    /// Build from a [`fn64_recomp::RecompConfig`]: every function in every
    /// section contributes its `(name, vram)` pair.
    pub fn from_config(cfg: &fn64_recomp::RecompConfig) -> Self {
        let entries = cfg
            .sections
            .iter()
            .flat_map(|s| s.functions.iter())
            .map(|f| (f.name.clone(), f.vram));
        SymbolTable::from_entries(entries)
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
    out.push_str("pub fn lookup(vram: u32) -> RecompFunc {\n");
    out.push_str("    if let Some(func) = resolve_host_function(vram) {\n");
    out.push_str("        return func;\n");
    out.push_str("    }\n");
    out.push_str("    match LOOKUP_TABLE.binary_search_by_key(&vram, |(addr, _)| *addr) {\n");
    out.push_str("        Ok(index) => LOOKUP_TABLE[index].1,\n");
    out.push_str("        Err(_) => fn64_recomp_rs::trap_unsupported(format!(\"lookup: no recompiled function or host shim at vram {vram:#010X}\")),\n");
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
    out.push_str("// Generated by fn64-recomp-rs (ELF/symbol front-end).\n");
    out.push_str("// Whole-program: inter-function JAL/J resolve to direct Rust calls.\n");
    out.push_str("// Typed Rust, no unsafe, no pointer casts.\n");
    out.push_str("#![allow(clippy::all, unused, non_snake_case)]\n");
    out.push_str("pub const FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA: fn64_recomp_rs::FunctionEntryObservationSchema = fn64_recomp_rs::FUNCTION_ENTRY_OBSERVATION_SCHEMA;\n");
    out.push_str(
        "use fn64_recomp_rs::{call_host_or_recompiled, pause_self, resolve_host_function, RecompContext, RecompFunc, Rdram};\n\n",
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

#[cfg(test)]
mod tests {
    use super::*;

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
