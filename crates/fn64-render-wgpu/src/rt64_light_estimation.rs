//! Literal port of RT64's `LightManager::estimatedSunLight` /
//! `LightManager::estimatedAmbientLight`, a literal port of the permitted
//! MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/hle/rt64_light_manager.cpp`
//! (lines 107-165) / `src/hle/rt64_light_manager.h` (SHA-256 of the whole
//! files,
//! `ec4f5d24800c7d14c4de8acf7b57c2c25c7bd2170a14a919cdf2383368c66a34` /
//! `9bdfd951a016b142c86534d10df224b9dec02355bc73b1bbd4f77e5be2fb94de`):
//!
//! ```text
//! // rt64_light_manager.h
//! struct LightManager {
//!     struct Directional {
//!         hlslpp::float3 dir;
//!         hlslpp::float3 colTotal;
//!         float intensityTotal;
//!     };
//!
//!     std::vector<Directional> directionalLights;
//!     std::vector<interop::PointLight> pointLights;
//!     hlslpp::float3 ambientColSum;
//!     int ambientSum;
//!     ...
//! };
//!
//! // rt64_light_manager.cpp, lines 107-165
//! interop::PointLight LightManager::estimatedSunLight(const float sunIntensity, const float sunDistance) const {
//!     hlslpp::float3 sunDir = { 0.5f, 1.0f, 0.0f };
//!     hlslpp::float3 sunCol = { 0.8f, 0.7f, 0.6f };
//!     float biggestIntensity = 0;
//!     int biggestDirLight = -1;
//!     for (size_t i = 0; i < directionalLights.size(); i++) {
//!         if (directionalLights[i].intensityTotal > biggestIntensity) {
//!             biggestDirLight = i;
//!             biggestIntensity = directionalLights[i].intensityTotal;
//!         }
//!     }
//!
//!     // Pick the light with the biggest amount of matches.
//!     if (biggestDirLight >= 0) {
//!         const auto &l = directionalLights[biggestDirLight];
//!         sunDir = l.dir;
//!         sunCol.x = l.colTotal.x / l.intensityTotal;
//!         sunCol.y = l.colTotal.y / l.intensityTotal;
//!         sunCol.z = l.colTotal.z / l.intensityTotal;
//!     }
//!
//!     interop::PointLight res;
//!     float length = sqrtf(sunDir.x * sunDir.x + sunDir.y * sunDir.y + sunDir.z * sunDir.z);
//!     res.position.x = (sunDir.x / length) * sunDistance;
//!     res.position.y = (sunDir.y / length) * sunDistance;
//!     res.position.z = (sunDir.z / length) * sunDistance;
//!     res.direction.x = sunDir.x;
//!     res.direction.y = sunDir.y;
//!     res.direction.z = sunDir.z;
//!     res.diffuseColor.x = sunCol.x;
//!     res.diffuseColor.y = sunCol.y;
//!     res.diffuseColor.z = sunCol.z;
//!     res.diffuseColor.x *= sunIntensity;
//!     res.diffuseColor.y *= sunIntensity;
//!     res.diffuseColor.z *= sunIntensity;
//!     res.specularColor.x = res.diffuseColor.x * 0.5f;
//!     res.specularColor.y = res.diffuseColor.y * 0.5f;
//!     res.specularColor.z = res.diffuseColor.z * 0.5f;
//!     res.pointRadius = sunDistance * 0.01f;
//!     res.spotFalloffCosine = 1.0f;
//!     res.spotMaxCosine = 1.0f;
//!     res.attenuationRadius = 99999997952.0f;
//!     res.attenuationExponent = 1.0f;
//!     res.flickerIntensity = 0.0f;
//!     res.shadowOffset = 0.0f;
//!     res.groupBits = 1;
//!     return res;
//! }
//!
//! hlslpp::float3 LightManager::estimatedAmbientLight(const float ambientIntensity) const {
//!     hlslpp::float3 estimate;
//!     estimate.x = ambientColSum.x / ambientSum;
//!     estimate.y = ambientColSum.y / ambientSum;
//!     estimate.z = ambientColSum.z / ambientSum;
//!     estimate.x *= ambientIntensity;
//!     estimate.y *= ambientIntensity;
//!     estimate.z *= ambientIntensity;
//!     return estimate;
//! }
//! ```
//!
//! **Reuse, not new type.** `sunDir`/`sunCol`/`ambientColSum` and the
//! `Directional::dir`/`colTotal` fields all reuse
//! [`fn64_render_ir::Vec3`](fn64_render_ir) directly (matching
//! `rsp_math.rs`'s established "backend-neutral `float3`" convention) --
//! no new vector type, and no `fn64-render-ir` edit. `interop::PointLight`
//! has no existing Rust equivalent anywhere in this workspace (confirmed by
//! grep across `fn64-render-ir` and `fn64-render-wgpu`), so this module
//! defines a local [`PointLight`] carrying exactly the fields
//! `estimatedSunLight` writes (`position`, `direction`, `diffuseColor`,
//! `specularColor`, `pointRadius`, `spotFalloffCosine`, `spotMaxCosine`,
//! `attenuationRadius`, `attenuationExponent`, `flickerIntensity`,
//! `shadowOffset`, `groupBits`) -- the same field set and order as
//! `src/shared/rt64_point_light.h`'s `interop::PointLight`, using `Vec3` for
//! the three `float3` members and `u32` for `groupBits` (matching the
//! header's `uint`).
//!
//! [`Directional`] is a plain, already-accumulated data record (`dir`,
//! `colTotal`, `intensityTotal`) -- the minimal caller-supplied input this
//! port needs in place of `LightManager::directionalLights`. The
//! accumulation machinery that builds up that `Vec<Directional>` frame over
//! frame (`LightManager::reset`, `processPointLight`, `processDirLight`,
//! `processAmbientLight`, `processLight`, and the `RSP::Light`/`State`
//! object graph they read) is deliberately **not** ported -- see
//! "Nonclaims". `estimated_sun_light` takes `directional_lights: &[Directional]`
//! and `estimated_ambient_light` takes `ambient_col_sum: Vec3, ambient_sum:
//! i32` in place of reading `self.ambientColSum`/`self.ambientSum`, which is
//! the literal, minimal caller-supplied-input shape for a `const` method
//! read-only over `LightManager`'s accumulated fields.
//!
//! ## Admitted domain
//!
//! - **Tie-breaking in the `biggestDirLight` scan uses strict `>`, not
//!   `>=`.** The loop only replaces `biggestDirLight` when a later light's
//!   `intensityTotal` is *strictly greater* than the current
//!   `biggestIntensity`, so on an exact tie the **first** (lowest-index)
//!   light with that intensity wins, not the last. This port's `for` loop
//!   over `directional_lights.iter().enumerate()` preserves that exact
//!   left-to-right strict-`>` comparison order -- not reassociated, not
//!   rewritten as a fold/max-by that could silently flip tie-breaking to
//!   last-wins.
//! - **`biggestIntensity` starts at `0.0`, not `f32::MIN`/`-inf`.** A light
//!   whose `intensityTotal` is `0.0` or negative can never become the
//!   selected sun light (the comparison is `> 0.0`, so it needs to be
//!   strictly positive) -- this is preserved exactly (see the "negative
//!   intensity" and "all-zero intensity" characterization tests below): the
//!   hardcoded default `sunDir`/`sunCol` fallback (`{0.5,1.0,0.0}` /
//!   `{0.8,0.7,0.6}`) is used whenever no directional light clears that
//!   bar, including the empty-slice case.
//! - **`length = sqrtf(dot(sunDir, sunDir))` has no zero-length guard.**
//!   If the selected (or default) `sunDir` has zero length, `sunDir.x /
//!   length` etc. is an unguarded IEEE-754 division by `0.0`, producing
//!   `±inf` (nonzero numerator) or `NaN` (`0.0 / 0.0`, only possible if
//!   `sunDir` itself is exactly `(0,0,0)`, which cannot happen via the
//!   hardcoded default and can only happen via a caller-supplied
//!   `Directional.dir` of `(0,0,0)`). This port does **not** add a zero-
//!   length normalization guard -- it propagates whatever IEEE-754
//!   produces, matching `rt64_math.rs`'s and `rt64_common.rs`'s established
//!   "preserve unguarded upstream division" precedent.
//! - **`sunCol.x/y/z = l.colTotal.{x,y,z} / l.intensityTotal`** is likewise
//!   an unguarded division. Since a light is only selected when
//!   `intensityTotal > 0.0`, this specific division can never see a `0.0`
//!   denominator through the "selected" path -- but it is still ported
//!   without an added guard, since the source has none and `intensityTotal`
//!   is caller-supplied data this port does not otherwise validate (e.g. a
//!   `Directional` passed with `intensityTotal == f32::INFINITY` still
//!   divides through unguarded, per IEEE-754).
//! - **`estimated_ambient_light`'s `ambientColSum.{x,y,z} / ambientSum`**:
//!   `ambientSum` is C++ `int`; the source relies on the usual arithmetic
//!   conversion to promote it to `float` before the division. This port
//!   makes that promotion explicit (`ambient_sum as f32`) and performs the
//!   same unguarded division -- `ambient_sum == 0` yields `±inf`/`NaN`
//!   exactly as the C++ `float / int(0)` (promoted to `float / 0.0f`)
//!   would, with no added zero-guard.
//! - **Operation order is preserved verbatim, not reassociated.** Each of
//!   `res.diffuseColor.{x,y,z} *= sunIntensity` (a separate statement after
//!   the plain assignment, not fused into one multiply), `res.specularColor
//!   = res.diffuseColor * 0.5` (computed *after* `sunIntensity` has already
//!   been applied to `diffuseColor`, so `specularColor` includes the
//!   `sunIntensity` scale), and `estimate.{x,y,z} *= ambientIntensity` (also
//!   a separate statement after the division) are ported as the same
//!   two-step sequence (assign-then-scale) in the same order, per component,
//!   rather than folded into a single expression -- float multiplication
//!   and division are not generally associative/distributive-safe to
//!   reorder at the bit level, even though this particular case (`(a/b)*c`
//!   done as two ops vs. one) is order-preserving; the port keeps the
//!   literal statement sequence regardless.
//! - **All magic-literal constants are copied verbatim, unmodified**:
//!   default `sunDir = (0.5, 1.0, 0.0)`, default `sunCol = (0.8, 0.7, 0.6)`,
//!   `pointRadius = sunDistance * 0.01`, `spotFalloffCosine =
//!   spotMaxCosine = 1.0`, `attenuationRadius = 99999997952.0` (RT64's own
//!   literal, not rounded or reformatted -- it is not exactly
//!   `f32::MAX`/1e11/any "nicer" nearby constant), `attenuationExponent =
//!   1.0`, `flickerIntensity = shadowOffset = 0.0`, `groupBits = 1`.
//! - **NaN/infinity propagate with no added `is_nan`/`is_finite` checks
//!   anywhere in either function.** A `Directional` with a NaN
//!   `intensityTotal` fails every `>` comparison in the scan (NaN
//!   comparisons are always `false` in IEEE-754), so it can never become
//!   `biggestDirLight` -- this is standard float-comparison behavior, not a
//!   ported branch, and is preserved automatically by using plain `>` in
//!   Rust (which has the identical NaN-is-never-greater semantics for
//!   `f32`).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. These are gameplay-facing lighting *heuristics* RT64
//! uses to estimate a plausible sun/ambient light from accumulated N64
//! microcode light data -- not a physically-derived or radiometrically
//! correct lighting model, and this port makes no claim otherwise.
//!
//! Deliberately not ported:
//!
//! - `LightManager::reset`, `processPointLight`, `processDirLight`,
//!   `processAmbientLight`, `processLight` -- the accumulation machinery
//!   that builds `directionalLights`/`pointLights`/`ambientColSum`/
//!   `ambientSum` up over a frame from `RSP::Light` microcode data and a
//!   `State*` object graph. The ticket scopes this port to the estimation
//!   *math* over already-accumulated inputs; the accumulation loop, the
//!   `RSP::Light` byte-swapped field layout, and the `State`/`RSP` object
//!   graph are a different, much larger surface this module does not claim.
//! - `#if ENABLE_AUTOMATIC_POINT_LIGHTS` gated code inside
//!   `processPointLight` (dead code in every build RT64 ships, since the
//!   macro is `0`, and part of the unported accumulation machinery besides).
//! - `LightManager::pointLights` (the accumulated point-light list) --
//!   unrelated to either ported function, which only ever reads
//!   `directionalLights`/`ambientColSum`/`ambientSum`.
//! - `interop::PointLight`'s use as a GPU-shared/HLSL-interop struct layout
//!   (`shared/rt64_hlsl.h`'s `float3`/alignment rules) -- the local
//!   [`PointLight`] here is a plain Rust struct with the same field set and
//!   order, not a GPU-layout-compatible type.

use fn64_render_ir::Vec3;

/// `LightManager::Directional`: one already-accumulated directional light
/// record (`dir`, `colTotal`, `intensityTotal`). The minimal caller-supplied
/// input in place of a `LightManager::directionalLights` element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Directional {
    pub dir: Vec3,
    pub col_total: Vec3,
    pub intensity_total: f32,
}

/// `interop::PointLight`: same field set and order as
/// `src/shared/rt64_point_light.h` (see module doc "Reuse, not new type").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub position: Vec3,
    pub direction: Vec3,
    pub diffuse_color: Vec3,
    pub attenuation_radius: f32,
    pub point_radius: f32,
    pub spot_falloff_cosine: f32,
    pub spot_max_cosine: f32,
    pub specular_color: Vec3,
    pub shadow_offset: f32,
    pub attenuation_exponent: f32,
    pub flicker_intensity: f32,
    pub group_bits: u32,
}

/// `LightManager::estimatedSunLight(sunIntensity, sunDistance)`: picks the
/// directional light with the largest `intensityTotal` (strict `>`,
/// first-wins on a tie -- see module doc "Admitted domain"), falling back to
/// a hardcoded default direction/color if `directional_lights` is empty or
/// every light has non-positive `intensityTotal`, then derives a
/// `PointLight` positioned along the (unguarded, possibly zero-length-
/// normalized) sun direction at `sun_distance`.
pub fn estimated_sun_light(
    directional_lights: &[Directional],
    sun_intensity: f32,
    sun_distance: f32,
) -> PointLight {
    let mut sun_dir = Vec3::new(0.5, 1.0, 0.0);
    let mut sun_col = Vec3::new(0.8, 0.7, 0.6);
    let mut biggest_intensity: f32 = 0.0;
    let mut biggest_dir_light: Option<usize> = None;

    for (i, light) in directional_lights.iter().enumerate() {
        if light.intensity_total > biggest_intensity {
            biggest_dir_light = Some(i);
            biggest_intensity = light.intensity_total;
        }
    }

    // Pick the light with the biggest amount of matches.
    if let Some(idx) = biggest_dir_light {
        let l = &directional_lights[idx];
        sun_dir = l.dir;
        sun_col.x = l.col_total.x / l.intensity_total;
        sun_col.y = l.col_total.y / l.intensity_total;
        sun_col.z = l.col_total.z / l.intensity_total;
    }

    let length = (sun_dir.x * sun_dir.x + sun_dir.y * sun_dir.y + sun_dir.z * sun_dir.z).sqrt();

    let mut res = PointLight {
        position: Vec3::new(
            (sun_dir.x / length) * sun_distance,
            (sun_dir.y / length) * sun_distance,
            (sun_dir.z / length) * sun_distance,
        ),
        direction: Vec3::new(sun_dir.x, sun_dir.y, sun_dir.z),
        diffuse_color: Vec3::new(sun_col.x, sun_col.y, sun_col.z),
        attenuation_radius: 99999997952.0,
        point_radius: sun_distance * 0.01,
        spot_falloff_cosine: 1.0,
        spot_max_cosine: 1.0,
        specular_color: Vec3::new(0.0, 0.0, 0.0),
        shadow_offset: 0.0,
        attenuation_exponent: 1.0,
        flicker_intensity: 0.0,
        group_bits: 1,
    };

    res.diffuse_color.x *= sun_intensity;
    res.diffuse_color.y *= sun_intensity;
    res.diffuse_color.z *= sun_intensity;

    res.specular_color.x = res.diffuse_color.x * 0.5;
    res.specular_color.y = res.diffuse_color.y * 0.5;
    res.specular_color.z = res.diffuse_color.z * 0.5;

    res
}

/// `LightManager::estimatedAmbientLight(ambientIntensity)`: averages
/// `ambient_col_sum` over `ambient_sum` accumulated samples (unguarded
/// division -- see module doc "Admitted domain"), then scales by
/// `ambient_intensity`.
pub fn estimated_ambient_light(
    ambient_col_sum: Vec3,
    ambient_sum: i32,
    ambient_intensity: f32,
) -> Vec3 {
    let mut estimate = Vec3::new(0.0, 0.0, 0.0);
    estimate.x = ambient_col_sum.x / ambient_sum as f32;
    estimate.y = ambient_col_sum.y / ambient_sum as f32;
    estimate.z = ambient_col_sum.z / ambient_sum as f32;

    estimate.x *= ambient_intensity;
    estimate.y *= ambient_intensity;
    estimate.z *= ambient_intensity;

    estimate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(dir: Vec3, col_total: Vec3, intensity_total: f32) -> Directional {
        Directional {
            dir,
            col_total,
            intensity_total,
        }
    }

    // --- estimated_sun_light: empty / fallback ---

    #[test]
    fn sun_light_empty_slice_uses_default_dir_and_col() {
        let res = estimated_sun_light(&[], 1.0, 100.0);
        // Default sunDir = (0.5, 1.0, 0.0), length = sqrt(1.25).
        let length = 1.25_f32.sqrt();
        assert!((res.direction.x - 0.5).abs() < 1e-6);
        assert!((res.direction.y - 1.0).abs() < 1e-6);
        assert_eq!(res.direction.z, 0.0);
        assert!((res.position.x - (0.5 / length) * 100.0).abs() < 1e-3);
        assert!((res.position.y - (1.0 / length) * 100.0).abs() < 1e-3);
        assert_eq!(res.position.z, 0.0);
        // Default sunCol = (0.8, 0.7, 0.6) * sunIntensity(1.0).
        assert!((res.diffuse_color.x - 0.8).abs() < 1e-6);
        assert!((res.diffuse_color.y - 0.7).abs() < 1e-6);
        assert!((res.diffuse_color.z - 0.6).abs() < 1e-6);
    }

    #[test]
    fn sun_light_all_zero_intensity_uses_default_fallback() {
        let lights = [
            dir(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 0.0),
            dir(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 0.0),
        ];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        // Strict `> 0.0` comparison: intensity 0.0 never beats the initial
        // biggestIntensity of 0.0, so the default (0.5,1.0,0.0) direction
        // is used, not either light's direction.
        assert!((res.direction.x - 0.5).abs() < 1e-6);
        assert!((res.direction.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sun_light_negative_intensity_uses_default_fallback() {
        let lights = [dir(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            -5.0,
        )];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        assert!((res.direction.x - 0.5).abs() < 1e-6);
        assert!((res.direction.y - 1.0).abs() < 1e-6);
    }

    // --- estimated_sun_light: single light ---

    #[test]
    fn sun_light_single_light_is_selected() {
        let lights = [dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.0, 4.0, 6.0), 2.0)];
        let res = estimated_sun_light(&lights, 1.0, 10.0);
        // direction = (0,0,1) verbatim.
        assert_eq!(res.direction, Vec3::new(0.0, 0.0, 1.0));
        // sunCol = colTotal / intensityTotal = (1.0, 2.0, 3.0).
        // diffuseColor = sunCol * sunIntensity(1.0) = (1.0, 2.0, 3.0).
        assert!((res.diffuse_color.x - 1.0).abs() < 1e-6);
        assert!((res.diffuse_color.y - 2.0).abs() < 1e-6);
        assert!((res.diffuse_color.z - 3.0).abs() < 1e-6);
        // specularColor = diffuseColor * 0.5.
        assert!((res.specular_color.x - 0.5).abs() < 1e-6);
        assert!((res.specular_color.y - 1.0).abs() < 1e-6);
        assert!((res.specular_color.z - 1.5).abs() < 1e-6);
        // position: length=1.0 (unit z), so position = direction * sunDistance.
        assert!((res.position.x - 0.0).abs() < 1e-6);
        assert!((res.position.y - 0.0).abs() < 1e-6);
        assert!((res.position.z - 10.0).abs() < 1e-4);
    }

    #[test]
    fn sun_light_sun_intensity_scales_diffuse_and_specular_but_not_position_direction() {
        let lights = [dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.0, 0.0, 0.0), 1.0)];
        let res = estimated_sun_light(&lights, 3.0, 10.0);
        // sunCol.x = 2.0/1.0 = 2.0; diffuseColor.x = 2.0*3.0 = 6.0.
        assert!((res.diffuse_color.x - 6.0).abs() < 1e-5);
        assert!((res.specular_color.x - 3.0).abs() < 1e-5);
        // direction/position are independent of sunIntensity.
        assert_eq!(res.direction, Vec3::new(0.0, 0.0, 1.0));
        assert!((res.position.z - 10.0).abs() < 1e-4);
    }

    // --- estimated_sun_light: multiple lights, tie-break ---

    #[test]
    fn sun_light_multiple_lights_picks_largest_intensity() {
        let lights = [
            dir(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 1.0),
            dir(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 2.0, 0.0), 5.0),
            dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 3.0), 2.0),
        ];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        // index 1 has the largest intensityTotal (5.0).
        assert_eq!(res.direction, Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn sun_light_exact_tie_first_occurrence_wins() {
        let lights = [
            dir(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 4.0),
            dir(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 2.0, 0.0), 4.0),
        ];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        // Strict `>` comparison never replaces an equal-intensity match, so
        // the first (index 0) light wins the tie, not the last.
        assert_eq!(res.direction, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn sun_light_later_lower_intensity_does_not_override_earlier_pick() {
        let lights = [
            dir(Vec3::new(9.0, 9.0, 9.0), Vec3::new(1.0, 1.0, 1.0), 10.0),
            dir(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 1.0),
        ];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        assert_eq!(res.direction, Vec3::new(9.0, 9.0, 9.0));
    }

    // --- estimated_sun_light: zero-length direction vector ---

    #[test]
    fn sun_light_zero_length_direction_produces_nan_position() {
        let lights = [dir(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0), 1.0)];
        let res = estimated_sun_light(&lights, 1.0, 5.0);
        // length = sqrt(0) = 0.0; 0.0/0.0 = NaN for each position component
        // (no zero-length guard -- see module doc "Admitted domain").
        assert!(res.position.x.is_nan());
        assert!(res.position.y.is_nan());
        assert!(res.position.z.is_nan());
        // direction is unaffected (assigned before the normalize divide).
        assert_eq!(res.direction, Vec3::new(0.0, 0.0, 0.0));
    }

    // --- estimated_sun_light: NaN and infinity inputs ---

    #[test]
    fn sun_light_nan_intensity_never_wins_the_scan() {
        let lights = [
            dir(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), f32::NAN),
            dir(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 1.0),
        ];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        // NaN > 0.0 is false, so index 0 never becomes biggestDirLight;
        // index 1 (intensity 1.0 > 0.0) wins.
        assert_eq!(res.direction, Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn sun_light_all_nan_intensities_uses_default_fallback() {
        let lights = [dir(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            f32::NAN,
        )];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        assert!((res.direction.x - 0.5).abs() < 1e-6);
        assert!((res.direction.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sun_light_infinite_intensity_wins_and_produces_nan_color() {
        let lights = [dir(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            f32::INFINITY,
        )];
        let res = estimated_sun_light(&lights, 1.0, 1.0);
        assert_eq!(res.direction, Vec3::new(0.0, 0.0, 1.0));
        // colTotal / INFINITY = 0.0 for a finite numerator (not NaN).
        assert_eq!(res.diffuse_color, Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn sun_light_infinite_sun_distance_produces_infinite_position() {
        let lights = [dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), 1.0)];
        let res = estimated_sun_light(&lights, 1.0, f32::INFINITY);
        assert!(res.position.z.is_infinite());
        assert!(res.point_radius.is_infinite());
    }

    #[test]
    fn sun_light_nan_sun_intensity_propagates_to_diffuse_and_specular() {
        let lights = [dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), 1.0)];
        let res = estimated_sun_light(&lights, f32::NAN, 1.0);
        assert!(res.diffuse_color.x.is_nan());
        assert!(res.specular_color.x.is_nan());
    }

    // --- estimated_sun_light: fixed/magic-literal fields ---

    #[test]
    fn sun_light_fixed_fields_match_source_literals() {
        let res = estimated_sun_light(&[], 1.0, 50.0);
        assert_eq!(res.attenuation_radius, 99999997952.0);
        assert_eq!(res.attenuation_exponent, 1.0);
        assert_eq!(res.spot_falloff_cosine, 1.0);
        assert_eq!(res.spot_max_cosine, 1.0);
        assert_eq!(res.flicker_intensity, 0.0);
        assert_eq!(res.shadow_offset, 0.0);
        assert_eq!(res.group_bits, 1);
        assert_eq!(res.point_radius, 50.0 * 0.01);
    }

    #[test]
    fn sun_light_point_radius_is_one_percent_of_distance() {
        let res = estimated_sun_light(&[], 1.0, 1234.0);
        assert!((res.point_radius - 12.34).abs() < 1e-3);
    }

    #[test]
    fn sun_light_negative_sun_distance_negates_position_and_point_radius() {
        let res = estimated_sun_light(&[], 1.0, -10.0);
        assert!(res.position.x < 0.0 || res.position.x == 0.0);
        // -10.0 * 0.01 in f32 is -0.099999994, not exactly -0.1 (0.01 has
        // no exact binary32 representation) -- compare with tolerance.
        assert!((res.point_radius - (-0.1)).abs() < 1e-6);
    }

    #[test]
    fn sun_light_zero_sun_intensity_zeroes_diffuse_and_specular() {
        let lights = [dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.0, 2.0, 2.0), 1.0)];
        let res = estimated_sun_light(&lights, 0.0, 1.0);
        assert_eq!(res.diffuse_color, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(res.specular_color, Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn sun_light_negative_sun_intensity_negates_diffuse_and_specular() {
        let lights = [dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.0, 2.0, 2.0), 1.0)];
        let res = estimated_sun_light(&lights, -1.0, 1.0);
        assert!((res.diffuse_color.x - (-2.0)).abs() < 1e-5);
        assert!((res.specular_color.x - (-1.0)).abs() < 1e-5);
    }

    // --- estimated_ambient_light ---

    #[test]
    fn ambient_light_basic_average_and_scale() {
        // ambientColSum = (4.0, 8.0, 12.0), ambientSum = 4 -> avg = (1,2,3).
        let sum = Vec3::new(4.0, 8.0, 12.0);
        let res = estimated_ambient_light(sum, 4, 1.0);
        assert!((res.x - 1.0).abs() < 1e-6);
        assert!((res.y - 2.0).abs() < 1e-6);
        assert!((res.z - 3.0).abs() < 1e-6);
    }

    #[test]
    fn ambient_light_intensity_scales_after_division() {
        let sum = Vec3::new(4.0, 8.0, 12.0);
        let res = estimated_ambient_light(sum, 4, 2.0);
        assert!((res.x - 2.0).abs() < 1e-6);
        assert!((res.y - 4.0).abs() < 1e-6);
        assert!((res.z - 6.0).abs() < 1e-6);
    }

    #[test]
    fn ambient_light_single_sample_returns_sum_itself_scaled() {
        let sum = Vec3::new(0.5, 0.25, 0.75);
        let res = estimated_ambient_light(sum, 1, 1.0);
        assert!((res.x - 0.5).abs() < 1e-6);
        assert!((res.y - 0.25).abs() < 1e-6);
        assert!((res.z - 0.75).abs() < 1e-6);
    }

    #[test]
    fn ambient_light_zero_sum_zero_samples_is_nan() {
        // ambientColSum = (0,0,0), ambientSum = 0 -> 0.0/0.0 = NaN, matching
        // C++ float/int(0) promoted to float/0.0f with no guard (see module
        // doc "Admitted domain").
        let res = estimated_ambient_light(Vec3::new(0.0, 0.0, 0.0), 0, 1.0);
        assert!(res.x.is_nan());
        assert!(res.y.is_nan());
        assert!(res.z.is_nan());
    }

    #[test]
    fn ambient_light_nonzero_sum_zero_samples_is_infinite() {
        let res = estimated_ambient_light(Vec3::new(1.0, -1.0, 2.0), 0, 1.0);
        assert!(res.x.is_infinite() && res.x > 0.0);
        assert!(res.y.is_infinite() && res.y < 0.0);
        assert!(res.z.is_infinite() && res.z > 0.0);
    }

    #[test]
    fn ambient_light_negative_sample_count_negates_the_average() {
        // ambientSum can go negative only via caller-supplied misuse, but the
        // source has no guard against it -- ambientSum as f32 = -4.0.
        let sum = Vec3::new(4.0, 8.0, 12.0);
        let res = estimated_ambient_light(sum, -4, 1.0);
        assert!((res.x - (-1.0)).abs() < 1e-6);
        assert!((res.y - (-2.0)).abs() < 1e-6);
        assert!((res.z - (-3.0)).abs() < 1e-6);
    }

    #[test]
    fn ambient_light_nan_ambient_intensity_propagates() {
        let sum = Vec3::new(4.0, 8.0, 12.0);
        let res = estimated_ambient_light(sum, 4, f32::NAN);
        assert!(res.x.is_nan());
        assert!(res.y.is_nan());
        assert!(res.z.is_nan());
    }

    #[test]
    fn ambient_light_infinite_col_sum_component_propagates() {
        let sum = Vec3::new(f32::INFINITY, 8.0, 12.0);
        let res = estimated_ambient_light(sum, 4, 1.0);
        assert!(res.x.is_infinite() && res.x > 0.0);
    }

    #[test]
    fn ambient_light_negative_col_sum_component_stays_negative() {
        let sum = Vec3::new(-4.0, 8.0, 12.0);
        let res = estimated_ambient_light(sum, 4, 1.0);
        assert!((res.x - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn ambient_light_zero_intensity_zeroes_result_even_with_finite_average() {
        let sum = Vec3::new(4.0, 8.0, 12.0);
        let res = estimated_ambient_light(sum, 4, 0.0);
        assert_eq!(res, Vec3::new(0.0, 0.0, 0.0));
    }
}
