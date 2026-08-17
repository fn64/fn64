//! Deterministic shader-source serialization from RT64's ShaderDescription.
//!
//! Literal port of the permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/render/rt64_shader_common.h`/`.cpp`
//! (SHA-256 of the whole files,
//! `c34ec83e2061f1586bfe68a0437ad1f93e49d985217c4319385a8013193b5e6f` /
//! `9606e91c5794b3ac388327d4724d19292cd1d32f6d58d7b5e198fb3481ea68d7`):
//!
//! Only the `ShaderDescription` struct and its `toShader()` method
//! are ported (the `hash()` method calling XXH3_64bits and the empty
//! `maskUnusedParameters()` stub are explicitly not ported — see "Nonclaims").
//!
//! ```text
//! // rt64_shader_common.h
//! struct ShaderDescription {
//!     interop::ColorCombiner colorCombiner;
//!     interop::OtherMode otherMode;
//!     interop::RenderFlags flags;
//!
//!     void maskUnusedParameters();
//!     uint64_t hash() const;
//!     std::string toShader() const;
//! };
//!
//! // rt64_shader_common.cpp
//! std::string ShaderDescription::toShader() const {
//!     std::stringstream ss;
//!     ss << "RenderParams rp;";
//!     ss << "rp.omL = " << std::to_string(otherMode.L) << "U;";
//!     ss << "rp.omH = " << std::to_string(otherMode.H) << "U;";
//!     ss << "rp.ccL = " << std::to_string(colorCombiner.L) << "U;";
//!     ss << "rp.ccH = " << std::to_string(colorCombiner.H) << "U;";
//!     ss << "rp.flags = " << std::to_string(flags.value) << ";";
//!     return ss.str();
//! }
//! ```
//!
//! ## Reuse, not new type
//!
//! `ShaderDescription` carries three plain-POD fields with no behavior:
//! `ColorCombiner` (two u32: L, H), `OtherMode` (two u32: L, H), and
//! `RenderFlags` (one u32: value). No struct wrapper is needed for the
//! port; the module only exports the free function `to_shader()` that
//! takes these three u32 pairs/values as separate parameters and returns
//! the formatted string.
//!
//! ## Admitted domain
//!
//! The `toShader()` method emits a literal C++ shader constant initializer:
//! six concatenated string fragments (no whitespace between them), with
//! a fixed order and specific serialization (decimal unsigned `U` suffix).
//! This port preserves both the emission order and the decimal formatting
//! exactly. The output is deterministic over the same inputs — identical
//! inputs always produce identical output.
//!
//! ## Nonclaims
//!
//! Not ported: `ShaderDescription::hash()`, which calls `XXH3_64bits()` —
//! adding an xxhash dependency is explicitly out of scope for this card.
//!
//! Not ported: `ShaderDescription::maskUnusedParameters()`, which is an
//! empty TODO stub in the source with no body — porting an empty function
//! would constitute a false coverage claim.

/// Emits the deterministic shader-source RenderParams initialization string
/// from a ShaderDescription's three fields (ColorCombiner L/H, OtherMode L/H,
/// RenderFlags value).
///
/// Output is deterministic: identical inputs always produce identical output.
/// The emission order and decimal formatting are preserved exactly from the
/// RT64 source.
///
/// # Arguments
///
/// * `cc_l` - ColorCombiner::L (u32)
/// * `cc_h` - ColorCombiner::H (u32)
/// * `om_l` - OtherMode::L (u32)
/// * `om_h` - OtherMode::H (u32)
/// * `flags_value` - RenderFlags::value (u32)
///
/// # Returns
///
/// A String containing the formatted RenderParams initializer:
/// `"RenderParams rp;rp.omL = <omL>U;rp.omH = <omH>U;rp.ccL = <ccL>U;rp.ccH = <ccH>U;rp.flags = <flags>;"`.
pub fn to_shader(cc_l: u32, cc_h: u32, om_l: u32, om_h: u32, flags_value: u32) -> String {
    let mut result = String::new();
    result.push_str("RenderParams rp;");
    result.push_str(&format!("rp.omL = {}U;", om_l));
    result.push_str(&format!("rp.omH = {}U;", om_h));
    result.push_str(&format!("rp.ccL = {}U;", cc_l));
    result.push_str(&format!("rp.ccH = {}U;", cc_h));
    result.push_str(&format!("rp.flags = {};", flags_value));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Empty and zero inputs ===

    #[test]
    fn empty_description_emits_zeros() {
        let result = to_shader(0, 0, 0, 0, 0);
        assert_eq!(
            result,
            "RenderParams rp;rp.omL = 0U;rp.omH = 0U;rp.ccL = 0U;rp.ccH = 0U;rp.flags = 0;"
        );
    }

    #[test]
    fn each_field_zero_independently() {
        let result_cc_l = to_shader(0, 1, 2, 3, 4);
        assert!(result_cc_l.contains("rp.ccL = 0U;"));

        let result_cc_h = to_shader(1, 0, 2, 3, 4);
        assert!(result_cc_h.contains("rp.ccH = 0U;"));

        let result_om_l = to_shader(1, 2, 0, 3, 4);
        assert!(result_om_l.contains("rp.omL = 0U;"));

        let result_om_h = to_shader(1, 2, 3, 0, 4);
        assert!(result_om_h.contains("rp.omH = 0U;"));

        let result_flags = to_shader(1, 2, 3, 4, 0);
        assert!(result_flags.contains("rp.flags = 0;"));
    }

    // === Full-range values ===

    #[test]
    fn max_u32_values() {
        let max = u32::MAX;
        let result = to_shader(max, max, max, max, max);
        assert_eq!(
            result,
            format!(
                "RenderParams rp;rp.omL = {}U;rp.omH = {}U;rp.ccL = {}U;rp.ccH = {}U;rp.flags = {};",
                max, max, max, max, max
            )
        );
    }

    #[test]
    fn one_each_field() {
        let result = to_shader(1, 1, 1, 1, 1);
        assert_eq!(
            result,
            "RenderParams rp;rp.omL = 1U;rp.omH = 1U;rp.ccL = 1U;rp.ccH = 1U;rp.flags = 1;"
        );
    }

    // === Emission order verification ===

    #[test]
    fn emission_order_is_exact() {
        let result = to_shader(11, 12, 13, 14, 15);
        let parts: Vec<&str> = result.split(';').collect();
        // parts = ["RenderParams rp", "rp.omL = 13U", "rp.omH = 14U", "rp.ccL = 11U", "rp.ccH = 12U", "rp.flags = 15", ""]
        assert_eq!(parts[0], "RenderParams rp");
        assert_eq!(parts[1], "rp.omL = 13U");
        assert_eq!(parts[2], "rp.omH = 14U");
        assert_eq!(parts[3], "rp.ccL = 11U");
        assert_eq!(parts[4], "rp.ccH = 12U");
        assert_eq!(parts[5], "rp.flags = 15");
    }

    #[test]
    fn no_whitespace_between_fragments() {
        let result = to_shader(1, 2, 3, 4, 5);
        assert!(!result.contains("  "));
        assert!(!result.contains(" ;"));
        assert!(!result.contains("; "));
    }

    // === Formatting verification ===

    #[test]
    fn cc_l_field_present() {
        let result = to_shader(12345, 0, 0, 0, 0);
        assert!(result.contains("rp.ccL = 12345U;"));
    }

    #[test]
    fn cc_h_field_present() {
        let result = to_shader(0, 67890, 0, 0, 0);
        assert!(result.contains("rp.ccH = 67890U;"));
    }

    #[test]
    fn om_l_field_present() {
        let result = to_shader(0, 0, 11111, 0, 0);
        assert!(result.contains("rp.omL = 11111U;"));
    }

    #[test]
    fn om_h_field_present() {
        let result = to_shader(0, 0, 0, 22222, 0);
        assert!(result.contains("rp.omH = 22222U;"));
    }

    #[test]
    fn flags_field_present() {
        let result = to_shader(0, 0, 0, 0, 33333);
        assert!(result.contains("rp.flags = 33333;"));
    }

    // === Decimal serialization (no hex, no base change) ===

    #[test]
    fn decimal_serialization_not_hex() {
        let result = to_shader(255, 0, 0, 0, 0);
        assert!(result.contains("255U"));
        assert!(!result.contains("0xffU"));
        assert!(!result.contains("0xFFU"));
    }

    #[test]
    fn large_values_in_decimal() {
        let result = to_shader(1000000, 2000000, 3000000, 4000000, 5000000);
        assert!(result.contains("1000000U"));
        assert!(result.contains("2000000U"));
        assert!(result.contains("3000000U"));
        assert!(result.contains("4000000U"));
        assert!(result.contains("5000000;"));
    }

    // === U suffix for unsigned flag ===

    #[test]
    fn u_suffix_on_all_cc_and_om_fields() {
        let result = to_shader(1, 2, 3, 4, 5);
        assert!(result.contains("rp.ccL = 1U;"));
        assert!(result.contains("rp.ccH = 2U;"));
        assert!(result.contains("rp.omL = 3U;"));
        assert!(result.contains("rp.omH = 4U;"));
    }

    #[test]
    fn no_u_suffix_on_flags_field() {
        let result = to_shader(0, 0, 0, 0, 42);
        assert!(result.contains("rp.flags = 42;"));
        // Ensure it doesn't have a U suffix (only one occurrence)
        assert_eq!(result.matches("42").count(), 1);
        assert!(!result.contains("42U"));
    }

    // === Determinism (same input → same output) ===

    #[test]
    fn deterministic_output() {
        let inputs = [(123, 456, 789, 1011, 1213); 10];
        let outputs: Vec<String> = inputs
            .iter()
            .map(|(cc_l, cc_h, om_l, om_h, flags)| to_shader(*cc_l, *cc_h, *om_l, *om_h, *flags))
            .collect();

        // All outputs must be identical
        for i in 1..outputs.len() {
            assert_eq!(
                outputs[0], outputs[i],
                "Determinism failure at iteration {}",
                i
            );
        }
    }

    #[test]
    fn different_inputs_different_outputs() {
        let result1 = to_shader(1, 2, 3, 4, 5);
        let result2 = to_shader(1, 2, 3, 4, 6);
        assert_ne!(result1, result2);

        let result3 = to_shader(2, 2, 3, 4, 5);
        assert_ne!(result1, result3);
    }

    // === Fully populated description ===

    #[test]
    fn fully_populated_description_real_world_like() {
        // Realistic ColorCombiner/OtherMode/RenderFlags values (not validated against any specific game)
        let cc_l = 0xA7C01012;
        let cc_h = 0x19E50F3F;
        let om_l = 0x00002410;
        let om_h = 0x00000C40;
        let flags = 0x00400025;

        let result = to_shader(cc_l, cc_h, om_l, om_h, flags);

        // Verify all fields are present with correct values
        assert!(result.contains("rp.ccL = 2814382098U;"));
        assert!(result.contains("rp.ccH = 434442047U;"));
        assert!(result.contains("rp.omL = 9232U;"));
        assert!(result.contains("rp.omH = 3136U;"));
        assert!(result.contains("rp.flags = 4194341;"));
    }

    // === Edge cases near power-of-2 boundaries ===

    #[test]
    fn powers_of_two() {
        let result = to_shader(1, 2, 4, 8, 16);
        assert!(result.contains("rp.ccL = 1U;"));
        assert!(result.contains("rp.ccH = 2U;"));
        assert!(result.contains("rp.omL = 4U;"));
        assert!(result.contains("rp.omH = 8U;"));
        assert!(result.contains("rp.flags = 16;"));
    }

    #[test]
    fn near_u32_boundaries() {
        let near_max = u32::MAX - 1;
        let result = to_shader(near_max, near_max, near_max, near_max, near_max);
        assert!(result.contains("4294967294U"));
        assert!(result.contains("4294967294;"));
    }

    // === Ordering consistency ===

    #[test]
    fn field_ordering_identical_regardless_of_input_values() {
        let result_a = to_shader(100, 200, 300, 400, 500);
        let result_b = to_shader(1, 2, 3, 4, 5);

        // Extract positions of each field assignment
        let pos_a_oml = result_a.find("rp.omL").unwrap();
        let pos_a_omh = result_a.find("rp.omH").unwrap();
        let pos_a_ccl = result_a.find("rp.ccL").unwrap();
        let pos_a_cch = result_a.find("rp.ccH").unwrap();
        let pos_a_flags = result_a.find("rp.flags").unwrap();

        let pos_b_oml = result_b.find("rp.omL").unwrap();
        let pos_b_omh = result_b.find("rp.omH").unwrap();
        let pos_b_ccl = result_b.find("rp.ccL").unwrap();
        let pos_b_cch = result_b.find("rp.ccH").unwrap();
        let pos_b_flags = result_b.find("rp.flags").unwrap();

        // Order must be the same
        assert!(pos_a_oml < pos_a_omh);
        assert!(pos_a_omh < pos_a_ccl);
        assert!(pos_a_ccl < pos_a_cch);
        assert!(pos_a_cch < pos_a_flags);

        assert!(pos_b_oml < pos_b_omh);
        assert!(pos_b_omh < pos_b_ccl);
        assert!(pos_b_ccl < pos_b_cch);
        assert!(pos_b_cch < pos_b_flags);
    }

    // === Literal string start and structure ===

    #[test]
    fn starts_with_render_params_rp() {
        let result = to_shader(0, 0, 0, 0, 0);
        assert!(result.starts_with("RenderParams rp;"));
    }

    #[test]
    fn literal_field_names() {
        let result = to_shader(1, 2, 3, 4, 5);
        assert!(result.contains("rp.omL"));
        assert!(result.contains("rp.omH"));
        assert!(result.contains("rp.ccL"));
        assert!(result.contains("rp.ccH"));
        assert!(result.contains("rp.flags"));
    }

    // === Boundary transitions ===

    #[test]
    fn transition_from_zero_to_one() {
        let result_zero = to_shader(0, 0, 0, 0, 0);
        let result_one = to_shader(1, 1, 1, 1, 1);
        assert_ne!(result_zero, result_one);
        assert!(result_one.contains("1U"));
        assert!(result_one.contains("1;"));
    }

    #[test]
    fn each_field_independently_varies() {
        let base = to_shader(0, 0, 0, 0, 0);
        let vary_cc_l = to_shader(100, 0, 0, 0, 0);
        let vary_cc_h = to_shader(0, 100, 0, 0, 0);
        let vary_om_l = to_shader(0, 0, 100, 0, 0);
        let vary_om_h = to_shader(0, 0, 0, 100, 0);
        let vary_flags = to_shader(0, 0, 0, 0, 100);

        assert_ne!(base, vary_cc_l);
        assert_ne!(base, vary_cc_h);
        assert_ne!(base, vary_om_l);
        assert_ne!(base, vary_om_h);
        assert_ne!(base, vary_flags);

        // No two should be identical
        assert_ne!(vary_cc_l, vary_cc_h);
        assert_ne!(vary_cc_h, vary_om_l);
        assert_ne!(vary_om_l, vary_om_h);
        assert_ne!(vary_om_h, vary_flags);
    }
}
