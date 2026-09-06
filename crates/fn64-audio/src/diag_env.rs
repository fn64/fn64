//! The one permitted process-environment read site in this crate.
//!
//! Task 2.2 moved every `user`-class knob onto `fn64-shell`'s typed `Knobs`.
//! This crate has none: everything it reads from the environment is a
//! `diagnostic`-class sink -- census counters, dumps, timing probes, trace
//! switches. They are set by a gate harness or by hand for one investigation
//! and are read nowhere in a shipped configuration.
//!
//! Those are not migrated one-by-one -- there are ~150 of them across five
//! crates, and a flag for each is exactly the speculative surface the cleanup
//! plan forbids. Instead every one of them reads through [`diag_env`], so:
//!
//! - `scripts/lint-hot-path-env.py` can enforce a whole-crate rule ("the only
//!   permitted `env::var`/`var_os` call site in this crate is `diag_env`")
//!   rather than the per-function allowlist it had, which silently covered
//!   nothing outside the ten functions someone remembered to register;
//! - a follow-up task has ONE function to change when the sinks move onto a
//!   typed config, instead of ~150 call sites to find first.
//!
//! This is the seam, not the destination. It deliberately does no parsing,
//! caching or validation: each sink already owns its own (a bounded capacity,
//! an absolute-path rule, a cross-field precondition) with its own tests, and
//! re-deriving those here would put one contract in two places to drift.

/// Read one `diagnostic`-class environment variable.
///
/// Returns `None` when the variable is unset **or not valid Unicode** --
/// a diagnostic sink has no useful response to a mojibake value other than
/// staying off, and the previous call sites were split roughly evenly between
/// `var(..).ok()` and `var_os(..)`, which agreed on exactly that.
///
/// `name` is `&'static str` so every knob name is a literal in the source,
/// which is what `scripts/knob-registry.py` scans to build the `FN64_*`
/// denominator. A computed name would go uncatalogued.
pub(crate) fn diag_env(name: &'static str) -> Option<String> {
    std::env::var(name).ok()
}

/// Whether a `diagnostic`-class variable is set to anything at all.
///
/// The `var_os(..).is_some()` idiom, which several sinks use to mean "set,
/// value irrelevant". Kept as its own function rather than
/// `diag_env(name).is_some()` because the two differ for a non-Unicode value:
/// this one still reports it as set.
pub(crate) fn diag_env_present(name: &'static str) -> bool {
    std::env::var_os(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unset name reads as absent through both entry points. Uses a name
    /// no knob uses, so it cannot collide with a harness-set variable.
    #[test]
    fn an_unset_name_is_absent() {
        const NAME: &str = "FN64_DIAG_ENV_SELF_TEST_UNSET";
        assert_eq!(diag_env(NAME), None);
        assert!(!diag_env_present(NAME));
    }
}
