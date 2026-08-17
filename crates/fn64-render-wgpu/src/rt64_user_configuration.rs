//! Literal port of RT64's `UserConfiguration` validation logic: a literal
//! port of the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/common/rt64_user_configuration.h`/
//! `.cpp` (SHA-256 of the whole files,
//! `72c69ba81d6bbbd7d8219b84e3420a86c1029ce1c8b44865cab2b50786f6a04c` /
//! `c0c803009274c85e93fa706ca1ec67b2810f9a2ba2d8f2616fc02d829b6e2534`):
//!
//! Only `clampEnum`, `UserConfiguration::validate`, and
//! `UserConfiguration::msaaSampleCount` are ported (see "Nonclaims" for
//! everything else in these two files, including all JSON serialization).
//!
//! ```text
//! // rt64_user_configuration.h (enum declarations, lines 15-84)
//! enum class GraphicsAPI { D3D12, Vulkan, Metal, Automatic, OptionCount };
//! enum class Resolution { Original, WindowIntegerScale, Manual, OptionCount };
//! enum class DisplayBuffering { Double, Triple, OptionCount };
//! enum class Antialiasing { None, MSAA2X, MSAA4X, MSAA8X, OptionCount };
//! enum class Filtering { Nearest, Linear, AntiAliasedPixelScaling, OptionCount };
//! enum class AspectRatio { Original, Expand, Manual, OptionCount };
//! enum class Upscale2D { Original, ScaledOnly, All, OptionCount };
//! enum class RefreshRate { Original, Display, Manual, OptionCount };
//! enum class InternalColorFormat { Standard, High, Automatic, OptionCount };
//! enum class HardwareResolve { Disabled, Enabled, Automatic, OptionCount };
//!
//! // rt64_user_configuration.h (lines 86-108, fields + validate/msaaSampleCount declarations)
//! GraphicsAPI graphicsAPI;
//! Resolution resolution;
//! DisplayBuffering displayBuffering;
//! Antialiasing antialiasing;
//! double resolutionMultiplier;
//! int downsampleMultiplier;
//! Filtering filtering;
//! AspectRatio aspectRatio;
//! double aspectTarget;
//! AspectRatio extAspectRatio;
//! double extAspectTarget;
//! Upscale2D upscale2D;
//! bool threePointFiltering;
//! RefreshRate refreshRate;
//! int refreshRateTarget;
//! InternalColorFormat internalColorFormat;
//! HardwareResolve hardwareResolve;
//! bool idleWorkActive;
//! bool developerMode;
//!
//! void validate();
//! uint32_t msaaSampleCount() const;
//! static uint32_t msaaSampleCount(Antialiasing antialiasing);
//!
//! // rt64_user_configuration.cpp, lines 57-60
//! template <typename T>
//! void clampEnum(T &e) {
//!     e = std::clamp(e, T(0), T(int(T::OptionCount) - 1));
//! }
//!
//! // rt64_user_configuration.cpp, lines 64-65 (ResolutionMultiplierLimit)
//! const int UserConfiguration::ResolutionMultiplierLimit = 32;
//!
//! // rt64_user_configuration.cpp, lines 88-109 (validate)
//! void UserConfiguration::validate() {
//!     clampEnum<GraphicsAPI>(graphicsAPI);
//!     clampEnum<Resolution>(resolution);
//!     clampEnum<DisplayBuffering>(displayBuffering);
//!     clampEnum<Antialiasing>(antialiasing);
//!     clampEnum<Filtering>(filtering);
//!     clampEnum<AspectRatio>(aspectRatio);
//!     clampEnum<AspectRatio>(extAspectRatio);
//!     clampEnum<Upscale2D>(upscale2D);
//!     clampEnum<RefreshRate>(refreshRate);
//!     clampEnum<InternalColorFormat>(internalColorFormat);
//!     clampEnum<HardwareResolve>(hardwareResolve);
//!     resolutionMultiplier = std::clamp<double>(resolutionMultiplier, 0.0f, ResolutionMultiplierLimit);
//!     downsampleMultiplier = std::clamp<int>(downsampleMultiplier, 1, ResolutionMultiplierLimit);
//!     aspectTarget = std::clamp<double>(aspectTarget, 0.1f, 100.0f);
//!     extAspectTarget = std::clamp<double>(extAspectTarget, 0.1f, 100.0f);
//!     refreshRateTarget = std::clamp<int>(refreshRateTarget, 10, 1000);
//!
//!     if (!isGraphicsAPISupported(graphicsAPI)) {
//!         graphicsAPI = GraphicsAPI::Automatic;
//!     }
//! }
//!
//! // rt64_user_configuration.cpp, lines 111-126 (msaaSampleCount)
//! uint32_t UserConfiguration::msaaSampleCount() const {
//!     return UserConfiguration::msaaSampleCount(antialiasing);
//! }
//!
//! uint32_t UserConfiguration::msaaSampleCount(Antialiasing antialiasing) {
//!     switch (antialiasing) {
//!     case Antialiasing::MSAA2X:
//!         return 2;
//!     case Antialiasing::MSAA4X:
//!         return 4;
//!     case Antialiasing::MSAA8X:
//!         return 8;
//!     default:
//!         return 1;
//!     }
//! }
//! ```
//!
//! **Reuse, not new type.** This module owns its own enum/struct
//! definitions (`GraphicsApi`, `Resolution`, ..., `UserConfiguration`) --
//! there is no existing `fn64-render-ir`/`fn64-render-wgpu` configuration
//! type to reuse, and `UserConfiguration` is RT64's own top-level settings
//! struct with no analogue elsewhere in this workspace (see "Nonclaims":
//! this module does **not** become fn64's own configuration system). The
//! generic `clampEnum<T>` template is ported as a generic Rust function
//! `clamp_enum<T: ClampableEnum>` over a small local trait
//! (`ClampableEnum::{LAST, from_ordinal, ordinal}`), matching the source's
//! own generic-over-eleven-call-sites shape in `validate()` rather than
//! duplicating the clamp logic eleven times.
//!
//! ## Admitted domain
//!
//! - **`clampEnum<T>`'s comparison semantics**: `std::clamp(e, T(0),
//!   T(int(T::OptionCount) - 1))` compares scoped-`enum class` values
//!   directly. C++ scoped enums have no user-visible underlying-type
//!   conversion for relational operators unless one is written, but
//!   `std::clamp` requires `operator<`, and the *built-in* relational
//!   operators for two operands of the same unscoped-arithmetic-compatible
//!   enumeration type are defined by the language to compare via each
//!   operand's underlying integral representation ([expr.rel] p3) -- none
//!   of these eleven `enum class`es declares an explicit underlying type,
//!   so each defaults to a signed integral type big enough to hold every
//!   enumerator (in practice `int` for all eleven, since none has more
//!   than ~5 enumerators). This port's `ClampableEnum::ordinal() -> i32`
//!   models that default-`int`-underlying-type comparison exactly: the
//!   clamp is ordinal-integer clamping, not a wrap or a saturate-to-a-flag
//!   -- an out-of-range ordinal (negative or `>= OptionCount`) is mapped to
//!   the nearest in-range ordinal (`0` or `OptionCount - 1`), and an
//!   already-in-range ordinal is an exact no-op (verified per-enum at every
//!   valid ordinal in the characterization tests below).
//! - **`clampEnum`'s behavior for an out-of-range input is clamp, not
//!   wrap, and not UB.** `std::clamp` is a pure min/max composition
//!   (`std::max(lo, std::min(hi, v))` in libstdc++/libc++'s usual
//!   implementation, though the standard specifies it via a single
//!   two-comparison formulation) -- there is no modular/wraparound
//!   arithmetic anywhere in `clampEnum`, and no UB for any `int`-valued
//!   input, since `T(n)` (a C-style cast from `int` to a scoped enum with
//!   no fixed underlying-type overflow trap for a value that already fits
//!   in that underlying type) is well-defined for any ordinary `int` in
//!   range of the underlying type. This port's `ClampableEnum::from_ordinal`
//!   accepts an arbitrary `i32` ordinal (not just the closed enum's own
//!   valid range) via each enum's `$name(i32)` newtype constructor, so the
//!   characterization tests below can build out-of-range values (e.g.
//!   `GraphicsApi::from_ordinal(-1)`) and feed them straight into
//!   `clamp_enum`, exactly mirroring what `T(n)` lets C++ do.
//! - **The eleven enums' underlying type is signed** (see above -- no
//!   `: unsigned` or `: uint8_t` etc. is declared on any of the eleven, so
//!   each defaults to a signed type). This matters because `T(int(...) -
//!   1)` for `DisplayBuffering` (`OptionCount = 2`) is `T(1)`, never
//!   negative for any of these eleven enums since each has at least one
//!   non-`OptionCount` enumerator, but the signedness is still an
//!   observable fact about the type this port's `ordinal(): i32` (not
//!   `u32`) return type mirrors.
//! - **`validate()`'s float-field clamps (`resolutionMultiplier`,
//!   `aspectTarget`, `extAspectTarget`) use `std::clamp<double>`, and the
//!   struct's own fields are typed `double`** even though the constructor
//!   initializes them from `float` literals (e.g. `2.0f`, `16.0f / 9.0f`)
//!   -- this port stores them as `f64` to match the declared field type
//!   exactly, not `f32` (a literal-type-preserving, not a
//!   initializer-literal-preserving, port). **`std::clamp` on a NaN input
//!   is UB-adjacent/unspecified**: the standard defines `clamp(v, lo, hi)`
//!   as `(v < lo) ? lo : (hi < v) ? hi : v` (or the two-comparator
//!   overload's equivalent), so for `v = NaN`, both `v < lo` and `hi < v`
//!   are `false` under IEEE-754 (NaN compares unordered/false against
//!   everything), so the whole expression evaluates to `v` itself --
//!   **NaN passes through `std::clamp` unchanged**, it is not replaced by
//!   `lo` or `hi`. This port's `clamp_f64_range` helper reproduces this
//!   exact three-way-conditional shape (not Rust's `f64::clamp`, which
//!   **panics** on `min > max` and, more importantly, is specified to
//!   return `self` for `self.is_nan()` too as of Rust's current
//!   documented behavior -- but this port writes the conditional
//!   explicitly rather than depending on that being stable across Rust
//!   versions, since the whole point of this module is pinning C++'s
//!   *specific* three-way form). A NaN `resolutionMultiplier`/
//!   `aspectTarget`/`extAspectTarget` therefore survives `validate()`
//!   unchanged, characterized explicitly below. **A negative float input**
//!   (e.g. `resolutionMultiplier = -5.0`) is ordinary: `-5.0 < 0.0` is
//!   `true`, so it clamps to the low bound exactly like any other
//!   below-range value -- there is nothing NaN-like about negative-but-
//!   finite inputs.
//! - **`downsampleMultiplier`/`refreshRateTarget` are `int` clamped via
//!   `std::clamp<int>`** -- ordinary integer clamping, no float/NaN concern
//!   applies to these two fields.
//! - **`isGraphicsAPISupported` is called from inside `validate()`** (the
//!   final three lines) but is itself excluded from this port's scope (see
//!   "Nonclaims" -- it is platform-`#ifdef`-gated behavior, not range-
//!   clamping/`msaaSampleCount` logic). This port's `validate()` omits that
//!   call entirely rather than inventing a stand-in "is this GraphicsAPI
//!   supported on this build" oracle -- see "Nonclaims" for why, and note
//!   this means the ported `validate()` characterizes *only* the eleven
//!   `clampEnum` calls and the five numeric-range clamps, not the full
//!   `GraphicsAPI` fallback-to-`Automatic` behavior the C++ function also
//!   has.
//! - **`msaaSampleCount`'s exact accepted set: a fixed 4-entry list, not a
//!   power-of-two clamp.** The `switch` has exactly three non-default
//!   cases (`MSAA2X -> 2`, `MSAA4X -> 4`, `MSAA8X -> 8`) and a `default: return
//!   1` that catches every other `Antialiasing` value, including `None`
//!   (the fourth valid enumerator) *and* any out-of-range ordinal
//!   (negative, or `>= OptionCount`) that reaches this function bypassing
//!   `clampEnum` (e.g. called as the static two-argument overload directly
//!   with an arbitrary `Antialiasing` value, since C++ `switch` on an enum
//!   with no matching `case` simply falls through to `default` -- there is
//!   no implicit range check or UB for an out-of-range enum value used
//!   only as a `switch` discriminant). This port's `msaa_sample_count_for`
//!   takes an arbitrary `i32` ordinal (not just a valid `Antialiasing`) to
//!   characterize exactly this "anything not 1/2/3 maps to 1" default
//!   behavior, including negative ordinals and ordinals `> OptionCount`.
//!   It is emphatically **not** "clamp to the nearest power of two" (e.g.
//!   ordinal `5`, which is not a valid `Antialiasing` at all, does not
//!   become `8`; it becomes `1`, same as every other unmatched ordinal).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. This module does **not** become fn64's own
//! configuration system -- it exists solely to characterize RT64's
//! `validate()`/`msaaSampleCount`/`clampEnum` range-clamping behavior.
//! Deliberately not ported from `rt64_user_configuration.h`/`.cpp`:
//!
//! - **All JSON serialization**: `to_json`/`from_json`, every
//!   `NLOHMANN_JSON_SERIALIZE_ENUM` macro invocation (all eleven... ten of
//!   the eleven enums plus none for `DisplayBuffering`, which has no
//!   `NLOHMANN_JSON_SERIALIZE_ENUM` registration in the header at all),
//!   `ConfigurationJSON::read`/`write`, and the `#include <json/json.hpp>`
//!   / `using json = nlohmann::json;` dependency itself, are not ported.
//!   Porting any of this would pull `nlohmann::json` in as a dependency
//!   fn64 does not otherwise have, which the ticket scoping this module
//!   explicitly excludes.
//! - `UserConfiguration::UserConfiguration()` (the default constructor /
//!   default-value initialization) -- out of the named scope
//!   (`clampEnum`, `validate`, `msaaSampleCount`); this port's
//!   `UserConfiguration` struct has no `Default` impl or constructor
//!   function, since no characterization test needs to construct a
//!   "default" instance (each test builds the exact struct state it needs
//!   directly).
//! - `UserConfiguration::isGraphicsAPISupported` and
//!   `UserConfiguration::resolveGraphicsAPI` -- platform-`#ifdef`-gated
//!   (`_WIN32`/`__APPLE__`/else, and `_WIN64`/`__APPLE__`/else) behavior
//!   with no single portable semantics to characterize, and `validate()`'s
//!   call to `isGraphicsAPISupported` is omitted accordingly (see
//!   "Admitted domain"). `Sommelier::detectWine()` (referenced from
//!   `resolveGraphicsAPI`) is unrelated infrastructure this port does not
//!   pull in.
//! - `ConfigurationJSON::read`/`write` (JSON stream I/O; see "All JSON
//!   serialization" above -- also not portable without `nlohmann::json`
//!   and `<iostream>`/`<iomanip>` stream plumbing this crate does not use
//!   elsewhere).
//! - The `DLLEXPORT`/`extern`/namespace-boundary plumbing (`namespace
//!   RT64 { ... };`, `extern void to_json(...)` / `extern void
//!   from_json(...)` declarations) -- build-system/linkage plumbing, not
//!   portable behavior.

/// A scoped enum with a `T::OptionCount`-style sentinel, modeling the
/// generic constraint `clampEnum<T>` relies on (`T(0)`, `T(int(T::OptionCount)
/// - 1)`, and comparability). `ordinal()`/`from_ordinal()` model the C++
/// scoped enum's built-in relational-operator comparison-by-underlying-type
/// (see module doc "Admitted domain").
trait ClampableEnum: Copy {
    /// `int(T::OptionCount) - 1`: the highest valid ordinal.
    const LAST: i32;

    /// `T(int)`: constructs from an arbitrary ordinal, including
    /// out-of-range ones (mirrors the C++ `T(n)` cast, which is
    /// well-defined for any `int` in the underlying type's range -- a
    /// scoped enum with no fixed underlying type has no notion of an
    /// "invalid bit pattern" the way Rust's `enum` does, so this port
    /// represents each enum as a newtype-over-`i32` with named associated
    /// constants, not a Rust `enum`, precisely so an out-of-range ordinal
    /// can be constructed and observed without `unsafe`).
    fn from_ordinal(ordinal: i32) -> Self;

    /// The enum value's underlying-type ordinal.
    fn ordinal(self) -> i32;
}

/// `clampEnum<T>(T &e)`: `e = std::clamp(e, T(0), T(int(T::OptionCount) -
/// 1))`. Clamps (not wraps) an out-of-range ordinal to `[0, LAST]`; an
/// already-in-range value is an exact no-op.
fn clamp_enum<T: ClampableEnum>(e: &mut T) {
    let clamped = e.ordinal().clamp(0, T::LAST);
    *e = T::from_ordinal(clamped);
}

/// `std::clamp<double>(v, lo, hi)` written as the standard's explicit
/// three-way conditional (`(v < lo) ? lo : (hi < v) ? hi : v`), so a NaN
/// `v` passes through unchanged (NaN compares false against both bounds --
/// see module doc "Admitted domain"). Deliberately not `f64::clamp`, whose
/// NaN behavior this port pins independently of any future Rust stdlib
/// documentation change.
fn clamp_f64_range(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// `std::clamp<int>(v, lo, hi)`: ordinary integer clamp (no NaN concern for
/// `i32`).
fn clamp_i32_range(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

macro_rules! clampable_enum {
    ($name:ident, $last:expr, [$($variant:ident = $ord:expr),+ $(,)?]) => {
        /// Newtype-over-`i32`, matching a C++ scoped enum's actual
        /// representation (a named set of integer constants, not a
        /// closed Rust-style discriminant set) -- see `ClampableEnum::
        /// from_ordinal`'s doc for why this shape was chosen over a plain
        /// Rust `enum`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(i32);

        #[allow(non_upper_case_globals)]
        impl $name {
            $(pub const $variant: $name = $name($ord);)+
        }

        impl ClampableEnum for $name {
            const LAST: i32 = $last;

            fn from_ordinal(ordinal: i32) -> Self {
                $name(ordinal)
            }

            fn ordinal(self) -> i32 {
                self.0
            }
        }
    };
}

// enum class GraphicsAPI { D3D12, Vulkan, Metal, Automatic, OptionCount };
clampable_enum!(
    GraphicsApi,
    3,
    [D3D12 = 0, Vulkan = 1, Metal = 2, Automatic = 3]
);

// enum class Resolution { Original, WindowIntegerScale, Manual, OptionCount };
clampable_enum!(
    Resolution,
    2,
    [Original = 0, WindowIntegerScale = 1, Manual = 2]
);

// enum class DisplayBuffering { Double, Triple, OptionCount };
clampable_enum!(DisplayBuffering, 1, [Double = 0, Triple = 1]);

// enum class Antialiasing { None, MSAA2X, MSAA4X, MSAA8X, OptionCount };
clampable_enum!(
    Antialiasing,
    3,
    [None = 0, Msaa2x = 1, Msaa4x = 2, Msaa8x = 3]
);

// enum class Filtering { Nearest, Linear, AntiAliasedPixelScaling, OptionCount };
clampable_enum!(
    Filtering,
    2,
    [Nearest = 0, Linear = 1, AntiAliasedPixelScaling = 2]
);

// enum class AspectRatio { Original, Expand, Manual, OptionCount };
clampable_enum!(AspectRatio, 2, [Original = 0, Expand = 1, Manual = 2]);

// enum class Upscale2D { Original, ScaledOnly, All, OptionCount };
clampable_enum!(Upscale2D, 2, [Original = 0, ScaledOnly = 1, All = 2]);

// enum class RefreshRate { Original, Display, Manual, OptionCount };
clampable_enum!(RefreshRate, 2, [Original = 0, Display = 1, Manual = 2]);

// enum class InternalColorFormat { Standard, High, Automatic, OptionCount };
clampable_enum!(
    InternalColorFormat,
    2,
    [Standard = 0, High = 1, Automatic = 2]
);

// enum class HardwareResolve { Disabled, Enabled, Automatic, OptionCount };
clampable_enum!(
    HardwareResolve,
    2,
    [Disabled = 0, Enabled = 1, Automatic = 2]
);

/// `UserConfiguration`'s fields (lines 86-104), minus the JSON
/// serialization surface (see module doc "Nonclaims"). `resolutionMultiplier`
/// / `aspectTarget` / `extAspectTarget` are `f64`, matching the C++ `double`
/// field type exactly (see module doc "Admitted domain").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UserConfiguration {
    pub graphics_api: GraphicsApi,
    pub resolution: Resolution,
    pub display_buffering: DisplayBuffering,
    pub antialiasing: Antialiasing,
    pub resolution_multiplier: f64,
    pub downsample_multiplier: i32,
    pub filtering: Filtering,
    pub aspect_ratio: AspectRatio,
    pub aspect_target: f64,
    pub ext_aspect_ratio: AspectRatio,
    pub ext_aspect_target: f64,
    pub upscale_2d: Upscale2D,
    pub three_point_filtering: bool,
    pub refresh_rate: RefreshRate,
    pub refresh_rate_target: i32,
    pub internal_color_format: InternalColorFormat,
    pub hardware_resolve: HardwareResolve,
    pub idle_work_active: bool,
    pub developer_mode: bool,
}

/// `UserConfiguration::ResolutionMultiplierLimit = 32`.
pub const RESOLUTION_MULTIPLIER_LIMIT: i32 = 32;

impl UserConfiguration {
    /// `UserConfiguration::validate()`, lines 88-109, **minus** the final
    /// `isGraphicsAPISupported` fallback (out of this port's scope -- see
    /// module doc "Nonclaims"/"Admitted domain"). Preserves the exact
    /// clamp order and comparison semantics of the source: eleven
    /// `clampEnum` calls, then five numeric-range clamps, in the source's
    /// own order.
    pub fn validate(&mut self) {
        clamp_enum(&mut self.graphics_api);
        clamp_enum(&mut self.resolution);
        clamp_enum(&mut self.display_buffering);
        clamp_enum(&mut self.antialiasing);
        clamp_enum(&mut self.filtering);
        clamp_enum(&mut self.aspect_ratio);
        clamp_enum(&mut self.ext_aspect_ratio);
        clamp_enum(&mut self.upscale_2d);
        clamp_enum(&mut self.refresh_rate);
        clamp_enum(&mut self.internal_color_format);
        clamp_enum(&mut self.hardware_resolve);

        self.resolution_multiplier = clamp_f64_range(
            self.resolution_multiplier,
            0.0,
            RESOLUTION_MULTIPLIER_LIMIT as f64,
        );
        self.downsample_multiplier =
            clamp_i32_range(self.downsample_multiplier, 1, RESOLUTION_MULTIPLIER_LIMIT);
        self.aspect_target = clamp_f64_range(self.aspect_target, 0.1, 100.0);
        self.ext_aspect_target = clamp_f64_range(self.ext_aspect_target, 0.1, 100.0);
        self.refresh_rate_target = clamp_i32_range(self.refresh_rate_target, 10, 1000);

        // isGraphicsAPISupported fallback intentionally omitted -- see
        // module doc "Nonclaims"/"Admitted domain".
    }

    /// `UserConfiguration::msaaSampleCount() const`: delegates to the
    /// static overload using this instance's `antialiasing` field.
    pub fn msaa_sample_count(&self) -> u32 {
        msaa_sample_count_for(self.antialiasing)
    }
}

/// `UserConfiguration::msaaSampleCount(Antialiasing antialiasing)` (static
/// overload), lines 115-126: `MSAA2X -> 2`, `MSAA4X -> 4`, `MSAA8X -> 8`,
/// everything else (including `None`) `-> 1`. A fixed 4-entry list, not a
/// power-of-two clamp (see module doc "Admitted domain").
pub fn msaa_sample_count_for(antialiasing: Antialiasing) -> u32 {
    match antialiasing {
        Antialiasing::Msaa2x => 2,
        Antialiasing::Msaa4x => 4,
        Antialiasing::Msaa8x => 8,
        _ => 1,
    }
}

/// `msaaSampleCount`'s `switch` on an arbitrary ordinal, characterizing the
/// "any unmatched value (including out-of-range ordinals bypassing
/// `clampEnum`) falls through to `default: return 1`" behavior (see module
/// doc "Admitted domain"). Not part of the C++ API surface (the C++
/// function takes a strongly-typed `Antialiasing`, not a raw `int`) --
/// added purely as a characterization seam for out-of-range ordinals, since
/// `Antialiasing::from_ordinal` panics on out-of-range input (see
/// `clampable_enum!`).
fn msaa_sample_count_for_ordinal(ordinal: i32) -> u32 {
    match ordinal {
        1 => 2,
        2 => 4,
        3 => 8,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> UserConfiguration {
        UserConfiguration {
            graphics_api: GraphicsApi::Automatic,
            resolution: Resolution::WindowIntegerScale,
            display_buffering: DisplayBuffering::Double,
            antialiasing: Antialiasing::None,
            resolution_multiplier: 2.0,
            downsample_multiplier: 1,
            filtering: Filtering::AntiAliasedPixelScaling,
            aspect_ratio: AspectRatio::Original,
            aspect_target: 16.0 / 9.0,
            ext_aspect_ratio: AspectRatio::Original,
            ext_aspect_target: 16.0 / 9.0,
            upscale_2d: Upscale2D::ScaledOnly,
            three_point_filtering: true,
            refresh_rate: RefreshRate::Original,
            refresh_rate_target: 60,
            internal_color_format: InternalColorFormat::Automatic,
            hardware_resolve: HardwareResolve::Automatic,
            idle_work_active: true,
            developer_mode: false,
        }
    }

    // --- clamp_enum: GraphicsAPI (OptionCount=4, LAST=3) ---

    #[test]
    fn clamp_enum_graphics_api_below_min_clamps_to_zero() {
        let mut e = GraphicsApi::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, GraphicsApi::D3D12);
    }

    #[test]
    fn clamp_enum_graphics_api_min_is_noop() {
        let mut e = GraphicsApi::D3D12;
        clamp_enum(&mut e);
        assert_eq!(e, GraphicsApi::D3D12);
    }

    #[test]
    fn clamp_enum_graphics_api_mid_is_noop() {
        let mut e = GraphicsApi::Vulkan;
        clamp_enum(&mut e);
        assert_eq!(e, GraphicsApi::Vulkan);
    }

    #[test]
    fn clamp_enum_graphics_api_max_is_noop() {
        let mut e = GraphicsApi::Automatic;
        clamp_enum(&mut e);
        assert_eq!(e, GraphicsApi::Automatic);
    }

    #[test]
    fn clamp_enum_graphics_api_above_max_clamps_to_last() {
        let mut e = GraphicsApi::from_ordinal(4); // OptionCount itself
        clamp_enum(&mut e);
        assert_eq!(e, GraphicsApi::Automatic);
    }

    #[test]
    fn clamp_enum_graphics_api_far_above_max_clamps_to_last() {
        let mut e = GraphicsApi::from_ordinal(100);
        clamp_enum(&mut e);
        assert_eq!(e, GraphicsApi::Automatic);
    }

    // --- clamp_enum: Resolution (OptionCount=3, LAST=2) ---

    #[test]
    fn clamp_enum_resolution_below_min_clamps_to_zero() {
        let mut e = Resolution::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, Resolution::Original);
    }

    #[test]
    fn clamp_enum_resolution_min_is_noop() {
        let mut e = Resolution::Original;
        clamp_enum(&mut e);
        assert_eq!(e, Resolution::Original);
    }

    #[test]
    fn clamp_enum_resolution_last_valid_is_noop() {
        let mut e = Resolution::Manual;
        clamp_enum(&mut e);
        assert_eq!(e, Resolution::Manual);
    }

    #[test]
    fn clamp_enum_resolution_last_valid_plus_one_clamps_to_last() {
        let mut e = Resolution::from_ordinal(3); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, Resolution::Manual);
    }

    // --- clamp_enum: DisplayBuffering (OptionCount=2, LAST=1) ---

    #[test]
    fn clamp_enum_display_buffering_neg_one_clamps_to_zero() {
        let mut e = DisplayBuffering::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, DisplayBuffering::Double);
    }

    #[test]
    fn clamp_enum_display_buffering_zero_is_noop() {
        let mut e = DisplayBuffering::Double;
        clamp_enum(&mut e);
        assert_eq!(e, DisplayBuffering::Double);
    }

    #[test]
    fn clamp_enum_display_buffering_last_valid_is_noop() {
        let mut e = DisplayBuffering::Triple;
        clamp_enum(&mut e);
        assert_eq!(e, DisplayBuffering::Triple);
    }

    #[test]
    fn clamp_enum_display_buffering_last_valid_plus_one_clamps_to_last() {
        let mut e = DisplayBuffering::from_ordinal(2); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, DisplayBuffering::Triple);
    }

    // --- clamp_enum: Antialiasing (OptionCount=4, LAST=3) ---

    #[test]
    fn clamp_enum_antialiasing_neg_one_clamps_to_zero() {
        let mut e = Antialiasing::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, Antialiasing::None);
    }

    #[test]
    fn clamp_enum_antialiasing_zero_is_noop() {
        let mut e = Antialiasing::None;
        clamp_enum(&mut e);
        assert_eq!(e, Antialiasing::None);
    }

    #[test]
    fn clamp_enum_antialiasing_last_valid_is_noop() {
        let mut e = Antialiasing::Msaa8x;
        clamp_enum(&mut e);
        assert_eq!(e, Antialiasing::Msaa8x);
    }

    #[test]
    fn clamp_enum_antialiasing_last_valid_plus_one_clamps_to_last() {
        let mut e = Antialiasing::from_ordinal(4); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, Antialiasing::Msaa8x);
    }

    // --- clamp_enum: Filtering (OptionCount=3, LAST=2) ---

    #[test]
    fn clamp_enum_filtering_neg_one_clamps_to_zero() {
        let mut e = Filtering::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, Filtering::Nearest);
    }

    #[test]
    fn clamp_enum_filtering_zero_is_noop() {
        let mut e = Filtering::Nearest;
        clamp_enum(&mut e);
        assert_eq!(e, Filtering::Nearest);
    }

    #[test]
    fn clamp_enum_filtering_last_valid_is_noop() {
        let mut e = Filtering::AntiAliasedPixelScaling;
        clamp_enum(&mut e);
        assert_eq!(e, Filtering::AntiAliasedPixelScaling);
    }

    #[test]
    fn clamp_enum_filtering_last_valid_plus_one_clamps_to_last() {
        let mut e = Filtering::from_ordinal(3); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, Filtering::AntiAliasedPixelScaling);
    }

    // --- clamp_enum: AspectRatio (OptionCount=3, LAST=2) -- used for both
    // aspectRatio and extAspectRatio in validate().

    #[test]
    fn clamp_enum_aspect_ratio_neg_one_clamps_to_zero() {
        let mut e = AspectRatio::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, AspectRatio::Original);
    }

    #[test]
    fn clamp_enum_aspect_ratio_zero_is_noop() {
        let mut e = AspectRatio::Original;
        clamp_enum(&mut e);
        assert_eq!(e, AspectRatio::Original);
    }

    #[test]
    fn clamp_enum_aspect_ratio_last_valid_is_noop() {
        let mut e = AspectRatio::Manual;
        clamp_enum(&mut e);
        assert_eq!(e, AspectRatio::Manual);
    }

    #[test]
    fn clamp_enum_aspect_ratio_last_valid_plus_one_clamps_to_last() {
        let mut e = AspectRatio::from_ordinal(3); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, AspectRatio::Manual);
    }

    // --- clamp_enum: Upscale2D (OptionCount=3, LAST=2) ---

    #[test]
    fn clamp_enum_upscale2d_neg_one_clamps_to_zero() {
        let mut e = Upscale2D::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, Upscale2D::Original);
    }

    #[test]
    fn clamp_enum_upscale2d_zero_is_noop() {
        let mut e = Upscale2D::Original;
        clamp_enum(&mut e);
        assert_eq!(e, Upscale2D::Original);
    }

    #[test]
    fn clamp_enum_upscale2d_last_valid_is_noop() {
        let mut e = Upscale2D::All;
        clamp_enum(&mut e);
        assert_eq!(e, Upscale2D::All);
    }

    #[test]
    fn clamp_enum_upscale2d_last_valid_plus_one_clamps_to_last() {
        let mut e = Upscale2D::from_ordinal(3); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, Upscale2D::All);
    }

    // --- clamp_enum: RefreshRate (OptionCount=3, LAST=2) ---

    #[test]
    fn clamp_enum_refresh_rate_neg_one_clamps_to_zero() {
        let mut e = RefreshRate::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, RefreshRate::Original);
    }

    #[test]
    fn clamp_enum_refresh_rate_zero_is_noop() {
        let mut e = RefreshRate::Original;
        clamp_enum(&mut e);
        assert_eq!(e, RefreshRate::Original);
    }

    #[test]
    fn clamp_enum_refresh_rate_last_valid_is_noop() {
        let mut e = RefreshRate::Manual;
        clamp_enum(&mut e);
        assert_eq!(e, RefreshRate::Manual);
    }

    #[test]
    fn clamp_enum_refresh_rate_last_valid_plus_one_clamps_to_last() {
        let mut e = RefreshRate::from_ordinal(3); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, RefreshRate::Manual);
    }

    // --- clamp_enum: InternalColorFormat (OptionCount=3, LAST=2) ---

    #[test]
    fn clamp_enum_internal_color_format_neg_one_clamps_to_zero() {
        let mut e = InternalColorFormat::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, InternalColorFormat::Standard);
    }

    #[test]
    fn clamp_enum_internal_color_format_zero_is_noop() {
        let mut e = InternalColorFormat::Standard;
        clamp_enum(&mut e);
        assert_eq!(e, InternalColorFormat::Standard);
    }

    #[test]
    fn clamp_enum_internal_color_format_last_valid_is_noop() {
        let mut e = InternalColorFormat::Automatic;
        clamp_enum(&mut e);
        assert_eq!(e, InternalColorFormat::Automatic);
    }

    #[test]
    fn clamp_enum_internal_color_format_last_valid_plus_one_clamps_to_last() {
        let mut e = InternalColorFormat::from_ordinal(3); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, InternalColorFormat::Automatic);
    }

    // --- clamp_enum: HardwareResolve (OptionCount=3, LAST=2) ---

    #[test]
    fn clamp_enum_hardware_resolve_neg_one_clamps_to_zero() {
        let mut e = HardwareResolve::from_ordinal(-1);
        clamp_enum(&mut e);
        assert_eq!(e, HardwareResolve::Disabled);
    }

    #[test]
    fn clamp_enum_hardware_resolve_zero_is_noop() {
        let mut e = HardwareResolve::Disabled;
        clamp_enum(&mut e);
        assert_eq!(e, HardwareResolve::Disabled);
    }

    #[test]
    fn clamp_enum_hardware_resolve_last_valid_is_noop() {
        let mut e = HardwareResolve::Automatic;
        clamp_enum(&mut e);
        assert_eq!(e, HardwareResolve::Automatic);
    }

    #[test]
    fn clamp_enum_hardware_resolve_last_valid_plus_one_clamps_to_last() {
        let mut e = HardwareResolve::from_ordinal(3); // OptionCount
        clamp_enum(&mut e);
        assert_eq!(e, HardwareResolve::Automatic);
    }

    // --- validate(): numeric-range fields, below-min/min/mid/max/above-max ---

    #[test]
    fn validate_resolution_multiplier_below_min_clamps_to_zero() {
        let mut c = base_config();
        c.resolution_multiplier = -5.0;
        c.validate();
        assert_eq!(c.resolution_multiplier, 0.0);
    }

    #[test]
    fn validate_resolution_multiplier_at_min_is_noop() {
        let mut c = base_config();
        c.resolution_multiplier = 0.0;
        c.validate();
        assert_eq!(c.resolution_multiplier, 0.0);
    }

    #[test]
    fn validate_resolution_multiplier_mid_is_noop() {
        let mut c = base_config();
        c.resolution_multiplier = 4.5;
        c.validate();
        assert_eq!(c.resolution_multiplier, 4.5);
    }

    #[test]
    fn validate_resolution_multiplier_at_max_is_noop() {
        let mut c = base_config();
        c.resolution_multiplier = 32.0;
        c.validate();
        assert_eq!(c.resolution_multiplier, 32.0);
    }

    #[test]
    fn validate_resolution_multiplier_above_max_clamps_to_limit() {
        let mut c = base_config();
        c.resolution_multiplier = 999.0;
        c.validate();
        assert_eq!(c.resolution_multiplier, 32.0);
    }

    #[test]
    fn validate_downsample_multiplier_below_min_clamps_to_one() {
        let mut c = base_config();
        c.downsample_multiplier = -5;
        c.validate();
        assert_eq!(c.downsample_multiplier, 1);
    }

    #[test]
    fn validate_downsample_multiplier_at_min_is_noop() {
        let mut c = base_config();
        c.downsample_multiplier = 1;
        c.validate();
        assert_eq!(c.downsample_multiplier, 1);
    }

    #[test]
    fn validate_downsample_multiplier_mid_is_noop() {
        let mut c = base_config();
        c.downsample_multiplier = 16;
        c.validate();
        assert_eq!(c.downsample_multiplier, 16);
    }

    #[test]
    fn validate_downsample_multiplier_at_max_is_noop() {
        let mut c = base_config();
        c.downsample_multiplier = 32;
        c.validate();
        assert_eq!(c.downsample_multiplier, 32);
    }

    #[test]
    fn validate_downsample_multiplier_above_max_clamps_to_limit() {
        let mut c = base_config();
        c.downsample_multiplier = 999;
        c.validate();
        assert_eq!(c.downsample_multiplier, 32);
    }

    #[test]
    fn validate_downsample_multiplier_zero_clamps_to_one_not_zero() {
        // Lower bound is 1, not 0 -- unlike resolutionMultiplier's 0.0 floor.
        let mut c = base_config();
        c.downsample_multiplier = 0;
        c.validate();
        assert_eq!(c.downsample_multiplier, 1);
    }

    #[test]
    fn validate_aspect_target_below_min_clamps_to_point_one() {
        let mut c = base_config();
        c.aspect_target = 0.0;
        c.validate();
        assert_eq!(c.aspect_target, 0.1);
    }

    #[test]
    fn validate_aspect_target_at_min_is_noop() {
        let mut c = base_config();
        c.aspect_target = 0.1;
        c.validate();
        assert_eq!(c.aspect_target, 0.1);
    }

    #[test]
    fn validate_aspect_target_mid_is_noop() {
        let mut c = base_config();
        c.aspect_target = 16.0 / 9.0;
        c.validate();
        assert_eq!(c.aspect_target, 16.0 / 9.0);
    }

    #[test]
    fn validate_aspect_target_at_max_is_noop() {
        let mut c = base_config();
        c.aspect_target = 100.0;
        c.validate();
        assert_eq!(c.aspect_target, 100.0);
    }

    #[test]
    fn validate_aspect_target_above_max_clamps_to_limit() {
        let mut c = base_config();
        c.aspect_target = 999.0;
        c.validate();
        assert_eq!(c.aspect_target, 100.0);
    }

    #[test]
    fn validate_ext_aspect_target_below_min_clamps_to_point_one() {
        let mut c = base_config();
        c.ext_aspect_target = -1.0;
        c.validate();
        assert_eq!(c.ext_aspect_target, 0.1);
    }

    #[test]
    fn validate_ext_aspect_target_at_min_is_noop() {
        let mut c = base_config();
        c.ext_aspect_target = 0.1;
        c.validate();
        assert_eq!(c.ext_aspect_target, 0.1);
    }

    #[test]
    fn validate_ext_aspect_target_mid_is_noop() {
        let mut c = base_config();
        c.ext_aspect_target = 16.0 / 9.0;
        c.validate();
        assert_eq!(c.ext_aspect_target, 16.0 / 9.0);
    }

    #[test]
    fn validate_ext_aspect_target_at_max_is_noop() {
        let mut c = base_config();
        c.ext_aspect_target = 100.0;
        c.validate();
        assert_eq!(c.ext_aspect_target, 100.0);
    }

    #[test]
    fn validate_ext_aspect_target_above_max_clamps_to_limit() {
        let mut c = base_config();
        c.ext_aspect_target = 999.0;
        c.validate();
        assert_eq!(c.ext_aspect_target, 100.0);
    }

    #[test]
    fn validate_refresh_rate_target_below_min_clamps_to_ten() {
        let mut c = base_config();
        c.refresh_rate_target = 0;
        c.validate();
        assert_eq!(c.refresh_rate_target, 10);
    }

    #[test]
    fn validate_refresh_rate_target_at_min_is_noop() {
        let mut c = base_config();
        c.refresh_rate_target = 10;
        c.validate();
        assert_eq!(c.refresh_rate_target, 10);
    }

    #[test]
    fn validate_refresh_rate_target_mid_is_noop() {
        let mut c = base_config();
        c.refresh_rate_target = 60;
        c.validate();
        assert_eq!(c.refresh_rate_target, 60);
    }

    #[test]
    fn validate_refresh_rate_target_at_max_is_noop() {
        let mut c = base_config();
        c.refresh_rate_target = 1000;
        c.validate();
        assert_eq!(c.refresh_rate_target, 1000);
    }

    #[test]
    fn validate_refresh_rate_target_above_max_clamps_to_limit() {
        let mut c = base_config();
        c.refresh_rate_target = 100_000;
        c.validate();
        assert_eq!(c.refresh_rate_target, 1000);
    }

    #[test]
    fn validate_refresh_rate_target_negative_clamps_to_min() {
        let mut c = base_config();
        c.refresh_rate_target = -240;
        c.validate();
        assert_eq!(c.refresh_rate_target, 10);
    }

    // --- validate(): NaN passes through unchanged (std::clamp NaN semantics) ---

    #[test]
    fn validate_resolution_multiplier_nan_passes_through_unchanged() {
        let mut c = base_config();
        c.resolution_multiplier = f64::NAN;
        c.validate();
        assert!(c.resolution_multiplier.is_nan());
    }

    #[test]
    fn validate_aspect_target_nan_passes_through_unchanged() {
        let mut c = base_config();
        c.aspect_target = f64::NAN;
        c.validate();
        assert!(c.aspect_target.is_nan());
    }

    #[test]
    fn validate_ext_aspect_target_nan_passes_through_unchanged() {
        let mut c = base_config();
        c.ext_aspect_target = f64::NAN;
        c.validate();
        assert!(c.ext_aspect_target.is_nan());
    }

    #[test]
    fn clamp_f64_range_negative_infinity_clamps_to_low() {
        assert_eq!(clamp_f64_range(f64::NEG_INFINITY, 0.1, 100.0), 0.1);
    }

    #[test]
    fn clamp_f64_range_positive_infinity_clamps_to_high() {
        assert_eq!(clamp_f64_range(f64::INFINITY, 0.1, 100.0), 100.0);
    }

    // --- validate(): field independence -- validate() touches every field,
    // spot check that mutating one field's clamp does not disturb another.

    #[test]
    fn validate_all_fields_simultaneously_out_of_range() {
        let mut c = UserConfiguration {
            graphics_api: GraphicsApi::from_ordinal(-1),
            resolution: Resolution::from_ordinal(9),
            display_buffering: DisplayBuffering::from_ordinal(-9),
            antialiasing: Antialiasing::from_ordinal(9),
            resolution_multiplier: -100.0,
            downsample_multiplier: -100,
            filtering: Filtering::from_ordinal(-1),
            aspect_ratio: AspectRatio::from_ordinal(9),
            aspect_target: -100.0,
            ext_aspect_ratio: AspectRatio::from_ordinal(-1),
            ext_aspect_target: 9999.0,
            upscale_2d: Upscale2D::from_ordinal(9),
            three_point_filtering: false,
            refresh_rate: RefreshRate::from_ordinal(-1),
            refresh_rate_target: -1,
            internal_color_format: InternalColorFormat::from_ordinal(9),
            hardware_resolve: HardwareResolve::from_ordinal(-1),
            idle_work_active: false,
            developer_mode: true,
        };
        c.validate();
        assert_eq!(c.graphics_api, GraphicsApi::D3D12);
        assert_eq!(c.resolution, Resolution::Manual);
        assert_eq!(c.display_buffering, DisplayBuffering::Double);
        assert_eq!(c.antialiasing, Antialiasing::Msaa8x);
        assert_eq!(c.resolution_multiplier, 0.0);
        assert_eq!(c.downsample_multiplier, 1);
        assert_eq!(c.filtering, Filtering::Nearest);
        assert_eq!(c.aspect_ratio, AspectRatio::Manual);
        assert_eq!(c.aspect_target, 0.1);
        assert_eq!(c.ext_aspect_ratio, AspectRatio::Original);
        assert_eq!(c.ext_aspect_target, 100.0);
        assert_eq!(c.upscale_2d, Upscale2D::All);
        assert_eq!(c.refresh_rate, RefreshRate::Original);
        assert_eq!(c.refresh_rate_target, 10);
        assert_eq!(c.internal_color_format, InternalColorFormat::Automatic);
        assert_eq!(c.hardware_resolve, HardwareResolve::Disabled);
        // Non-clamped fields (bool) must survive untouched.
        assert!(!c.three_point_filtering);
        assert!(!c.idle_work_active);
        assert!(c.developer_mode);
    }

    // --- msaaSampleCount: full Antialiasing domain ---

    #[test]
    fn msaa_sample_count_none_is_one() {
        assert_eq!(msaa_sample_count_for(Antialiasing::None), 1);
    }

    #[test]
    fn msaa_sample_count_msaa2x_is_two() {
        assert_eq!(msaa_sample_count_for(Antialiasing::Msaa2x), 2);
    }

    #[test]
    fn msaa_sample_count_msaa4x_is_four() {
        assert_eq!(msaa_sample_count_for(Antialiasing::Msaa4x), 4);
    }

    #[test]
    fn msaa_sample_count_msaa8x_is_eight() {
        assert_eq!(msaa_sample_count_for(Antialiasing::Msaa8x), 8);
    }

    #[test]
    fn msaa_sample_count_instance_method_delegates_to_static() {
        let mut c = base_config();
        c.antialiasing = Antialiasing::Msaa4x;
        assert_eq!(c.msaa_sample_count(), 4);
    }

    // --- msaaSampleCount: full ordinal domain, including 0 and
    // non-power-of-two / out-of-range values ---

    #[test]
    fn msaa_sample_count_ordinal_zero_is_one() {
        // Ordinal 0 == Antialiasing::None -> default case -> 1.
        assert_eq!(msaa_sample_count_for_ordinal(0), 1);
    }

    #[test]
    fn msaa_sample_count_ordinal_one_is_two() {
        assert_eq!(msaa_sample_count_for_ordinal(1), 2);
    }

    #[test]
    fn msaa_sample_count_ordinal_two_is_four() {
        assert_eq!(msaa_sample_count_for_ordinal(2), 4);
    }

    #[test]
    fn msaa_sample_count_ordinal_three_is_eight() {
        assert_eq!(msaa_sample_count_for_ordinal(3), 8);
    }

    #[test]
    fn msaa_sample_count_ordinal_four_option_count_is_one() {
        // OptionCount itself: unmatched -> default -> 1, not an error.
        assert_eq!(msaa_sample_count_for_ordinal(4), 1);
    }

    #[test]
    fn msaa_sample_count_ordinal_five_non_power_of_two_is_one() {
        // Not a valid Antialiasing at all, and not "nearest power of two
        // clamped" -- falls straight to default -> 1.
        assert_eq!(msaa_sample_count_for_ordinal(5), 1);
    }

    #[test]
    fn msaa_sample_count_ordinal_six_is_one() {
        assert_eq!(msaa_sample_count_for_ordinal(6), 1);
    }

    #[test]
    fn msaa_sample_count_ordinal_sixteen_non_power_of_two_target_is_one() {
        // 16 is itself a power of two but not one of {2,4,8} -- confirms
        // this is a fixed enumerated list, not "is a power of two".
        assert_eq!(msaa_sample_count_for_ordinal(16), 1);
    }

    #[test]
    fn msaa_sample_count_ordinal_negative_one_is_one() {
        assert_eq!(msaa_sample_count_for_ordinal(-1), 1);
    }

    #[test]
    fn msaa_sample_count_ordinal_large_negative_is_one() {
        assert_eq!(msaa_sample_count_for_ordinal(-9999), 1);
    }

    #[test]
    fn msaa_sample_count_ordinal_large_positive_is_one() {
        assert_eq!(msaa_sample_count_for_ordinal(999_999), 1);
    }

    // --- resolution_multiplier_limit constant sanity ---

    #[test]
    fn resolution_multiplier_limit_is_thirty_two() {
        assert_eq!(RESOLUTION_MULTIPLIER_LIMIT, 32);
    }
}
