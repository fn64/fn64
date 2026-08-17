//! Blender (port card §1): full one-cycle/two-cycle selector and cycle
//! semantics.
//!
//! Characterization-first, selective literal port of
//! `crate::raster::blend::{blend_fragment, blend_color, blend_a, blend_b}`
//! (`crates/fn64-render-reference/src/raster/blend.rs:157-292`), per
//! `/private/tmp/rt64-blender-depth-port-card.md` §1 ("Blender"). Covers:
//! P/M/A/B selector semantics for both blender cycles, sequential cycle
//! handoff (cycle 1's `Combined` result feeds cycle 2), the no-`FORCE_BL`
//! last-cycle bypass, the zero-factor (`a==0`/`b==0`) divisor collapse, the
//! `Framebuffer`-selecting dual-source composite path, and the exact
//! `IM_RD`-disabled loud rejection.
//!
//! `fn64-render-wgpu` has no crate dependency on `fn64-render-reference` (see
//! `depth_strict_less.rs`), so this is a self-contained literal re-expression
//! citing the reference's line numbers, matching this crate's existing
//! citation-comment convention. It reuses the already-landed
//! `crate::state::{OtherMode, BlenderCycle, CycleType}` wire decode
//! (`blender_cycle_1`/`blender_cycle_2`/`cycle_type`/`force_blend`/
//! `image_read_enabled`) rather than re-decoding those bitfields or defining
//! a duplicate mode enum -- this module's job starts one layer up, resolving
//! `BlenderCycle`'s raw 2-bit `color_a`/`alpha_a`/`color_b`/`alpha_b`
//! selectors into the semantic P/M/A/B input enums the reference's
//! `blend_color`/`blend_a`/`blend_b` consume.
//!
//! RT64 citation for the selector ordering and sequential cycle handoff:
//! `shared/rt64_blender.h:68-81,366-504` (pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`, per `docs/RT64-PORT-AUTHORITY.md`).
//! RT64's actual GPU mechanism for the `Framebuffer`-selecting composite is
//! dual-source alpha blending through the fixed-function blend unit
//! (`rt64_raster_shader.cpp:332-339`), not a shader-computed divide -- see
//! "WGSL/Rust dual-source seam" below for how this module represents that
//! contract without claiming a render-target read or draw integration.
//!
//! ## Scope
//!
//! In scope: P/M/A/B selector resolution for both cycles, sequential
//! cycle handoff, the no-`FORCE_BL` last-cycle bypass, the zero-factor
//! divisor collapse, `Framebuffer`/`FramebufferAlpha` legality gating on
//! `IM_RD`, and the dual-source output contract. Explicitly out of scope,
//! per the port card: combiner evaluation (upstream input), coverage
//! accumulation and the `blend_enabled` derivation itself (caller-supplied
//! here, matching the reference's `blend_fragment` signature), alpha
//! compare (upstream gate), depth test (upstream gate), dither (applied to
//! source alpha before this module runs), actual framebuffer resource
//! binding/readback, raster primitive/triangle execution, target storage,
//! presentation, native adapter qualification, full-ROM/pixel parity, and
//! any performance claim.
//!
//! ## WGSL/Rust dual-source seam
//!
//! The already-accepted M2.2 Metal-execution evidence
//! (`docs/RT64-PORT-DASHBOARD.md` `M2.2`, `probes/m2-wgpu-metal-headless`)
//! proves both of the port card's two structural options actually execute on
//! real wgpu/Metal: native dual-source blending via
//! `@blend_src(0)`/`@blend_src(1)` fragment outputs bound to
//! `wgpu::BlendFactor::Src1`/`OneMinusSrc1` (color) and
//! `Src1Alpha`/`OneMinusSrc1Alpha` (alpha) fixed-function blend state, and a
//! manual compute fallback computing the identical
//! `(src*factor + dst*(255-factor) + 127) / 255` per-channel round-to-nearest
//! composite when `wgpu::Features::DUAL_SOURCE_BLENDING` is unavailable. This
//! module reuses that exact contract rather than inventing a third blend
//! model: [`DualSourceBlendOutput`] carries the two values a real dual-source
//! draw call would emit as its two fragment outputs (`source` color,
//! `source1` blend factor), and [`manual_blend_composite`] performs the
//! matching integer fallback arithmetic. Neither type reads or writes an
//! actual render target -- there is no GPU execution, resource binding, or
//! draw-call integration here, matching this slice's nonclaims. The WGSL
//! seam (`BLEND_WGSL`) is a validated, retained compute-shader oracle over
//! the pre-blend selector arithmetic, not a compiled pipeline.

use crate::state::{BlenderCycle, CycleType, OtherMode};

/// Blend-color-input selector (`P`/`M` positions), resolved from
/// `BlenderCycle::color_a`/`color_b`'s raw 2-bit wire value. Literal port of
/// the reference's `BlendColorInput` (`gbi/types.rs:1772-1793` per the port
/// card; reference source at `crates/fn64-render-reference/src/raster/
/// blend.rs:242-268`'s `blend_color` match arms). Wire encoding: `0=Combined,
/// 1=Framebuffer, 2=Blend, 3=Fog` -- the public `SetRenderMode` `G_BL_*`
/// color-selector encoding, ported 1:1 from RT64's
/// `shared/rt64_other_mode.h:14-101`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendColorInput {
    Combined,
    Framebuffer,
    Blend,
    Fog,
}

impl BlendColorInput {
    pub const fn from_wire(selector: u8) -> Self {
        match selector & 0x3 {
            0 => Self::Combined,
            1 => Self::Framebuffer,
            2 => Self::Blend,
            _ => Self::Fog,
        }
    }
}

/// Blend-alpha-input selector (`A` position). Wire encoding: `0=Combined,
/// 1=Fog, 2=Shade, 3=Zero`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendAlphaInput {
    Combined,
    Fog,
    Shade,
    Zero,
}

impl BlendAlphaInput {
    pub const fn from_wire(selector: u8) -> Self {
        match selector & 0x3 {
            0 => Self::Combined,
            1 => Self::Fog,
            2 => Self::Shade,
            _ => Self::Zero,
        }
    }
}

/// Blend-`B`-input selector. Wire encoding: `0=OneMinusA,
/// 1=FramebufferAlpha, 2=One, 3=Zero`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendBInput {
    OneMinusA,
    FramebufferAlpha,
    One,
    Zero,
}

impl BlendBInput {
    pub const fn from_wire(selector: u8) -> Self {
        match selector & 0x3 {
            0 => Self::OneMinusA,
            1 => Self::FramebufferAlpha,
            2 => Self::One,
            _ => Self::Zero,
        }
    }
}

/// One blender cycle's four selectors, resolved from the raw
/// [`crate::state::BlenderCycle`] wire decode into semantic enums. `P`/`M`
/// use [`BlendColorInput`] (from `color_a`/`color_b`), `A` uses
/// [`BlendAlphaInput`] (from `alpha_a`), `B` uses [`BlendBInput`] (from
/// `alpha_b`) -- matching the public `GBL_c1`/`GBL_c2` formula naming
/// `(P*A + M*B) / (A+B)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedBlendCycle {
    pub p: BlendColorInput,
    pub a: BlendAlphaInput,
    pub m: BlendColorInput,
    pub b: BlendBInput,
}

impl ResolvedBlendCycle {
    /// Resolve one wire-decoded [`BlenderCycle`] into semantic selectors.
    /// This function is total: every 2-bit wire value maps to a variant,
    /// there is no reserved/illegal encoding in the blender selector space
    /// (unlike, e.g., alpha compare's encoding 2).
    pub const fn from_wire(cycle: BlenderCycle) -> Self {
        Self {
            p: BlendColorInput::from_wire(cycle.color_a),
            a: BlendAlphaInput::from_wire(cycle.alpha_a),
            m: BlendColorInput::from_wire(cycle.color_b),
            b: BlendBInput::from_wire(cycle.alpha_b),
        }
    }

    /// `true` when this cycle's `P`/`M`/`B` selectors name
    /// [`BlendColorInput::Framebuffer`] or [`BlendBInput::FramebufferAlpha`]
    /// -- i.e. this cycle cannot be evaluated without a real destination
    /// sample ([`BlendFramebufferSample`]), matching exactly the selector
    /// combinations [`blend_fragment`]'s own dispatch (`cycle.p ==
    /// Framebuffer`/`cycle.m == Framebuffer`/`blend_b`'s
    /// `FramebufferAlpha` arm) routes through the dual-source composite or
    /// the [`BlendImageReadError`] rejection, rather than the general A/B
    /// divide. A caller that wants to admit only the memory-independent
    /// subset of the blender (this crate's current production wiring) uses
    /// this predicate to reject a triangle before ever calling
    /// [`blend_fragment`] with `memory: None`.
    pub const fn requires_framebuffer_sample(self) -> bool {
        matches!(self.p, BlendColorInput::Framebuffer)
            || matches!(self.m, BlendColorInput::Framebuffer)
            || matches!(self.b, BlendBInput::FramebufferAlpha)
    }

    /// `true` exactly when this cycle's `B` selector is
    /// [`BlendBInput::FramebufferAlpha`] -- the coverage-count half of the
    /// framebuffer-memory dependency that this crate's production wiring
    /// still does not implement (no coverage-count GPU write exists yet).
    /// Narrower than [`Self::requires_framebuffer_sample`], which also
    /// matches on `P`/`M == Framebuffer` (the destination-*color* half this
    /// crate's Slice B production wiring does now support); does not change
    /// or repurpose `requires_framebuffer_sample` itself, which remains the
    /// correct "needs ANY memory sample" predicate.
    pub const fn requires_framebuffer_alpha(self) -> bool {
        matches!(self.b, BlendBInput::FramebufferAlpha)
    }
}

/// The blender's per-fragment framebuffer sample, needed only when a
/// selector in an active cycle names [`BlendColorInput::Framebuffer`] or
/// [`BlendBInput::FramebufferAlpha`]. This module does not read an actual
/// render target -- callers supply this sample (or `None` when `IM_RD` is
/// disabled or no sample was taken), matching the reference's
/// `ReadFramebufferMemory` seam and this slice's explicit nonclaim of
/// framebuffer resource binding/readback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendFramebufferSample {
    /// Framebuffer color, `[R, G, B, A]` bytes.
    pub rgba: [u8; 4],
    /// Framebuffer coverage count, `0..=8` (see port card §2; this module
    /// only consumes the already-computed count, it does not derive it).
    pub coverage_count: u8,
}

/// Why the blender could not proceed: the fragment reached a
/// framebuffer-color or framebuffer-alpha blend term while image-read memory
/// was unavailable. Literal port of the reference's
/// `read_framebuffer_memory` panic (`blend.rs:294-304`), re-expressed as a
/// typed, loud rejection rather than a `panic!` so this module never
/// silently substitutes a fallback color -- callers decide how to surface
/// the error (this module's own panicking entry points panic with the same
/// message the reference uses, matching `require_supported_alpha_compare`'s
/// precedent of a named, primitive-scoped panic).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlendImageReadError {
    /// Which selector triggered the read: `"framebuffer color"` or
    /// `"framebuffer coverage alpha"`, matching the reference's exact
    /// selector strings (`blend.rs:254,284`).
    pub selector: &'static str,
}

impl core::fmt::Display for BlendImageReadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "blender selects {} while IM_RD is disabled",
            self.selector
        )
    }
}

impl std::error::Error for BlendImageReadError {}

/// Resolve a [`BlendColorInput`] to its `[R, G, B]` `f32` value. Literal
/// port of `blend_color` (`blend.rs:242-268`). `Combined` reads the
/// pre-blend source color on cycle 0 and the *previous cycle's* blender
/// output on cycle 1 (sequential cycle handoff: `G_BL_CLR_IN` names the
/// first cycle's result once a second cycle runs).
///
/// # Errors
/// [`BlendImageReadError`] if `input` is [`BlendColorInput::Framebuffer`]
/// and `memory` is `None`.
pub const fn blend_color(
    input: BlendColorInput,
    src_rgb: [f32; 3],
    memory: Option<BlendFramebufferSample>,
    blend_color_register: [u8; 4],
    fog_color: [u8; 4],
    is_first_cycle: bool,
    running_rgb: [f32; 3],
) -> Result<[f32; 3], BlendImageReadError> {
    match input {
        BlendColorInput::Combined if is_first_cycle => Ok(src_rgb),
        BlendColorInput::Combined => Ok(running_rgb),
        BlendColorInput::Framebuffer => match memory {
            Some(sample) => Ok([
                sample.rgba[0] as f32,
                sample.rgba[1] as f32,
                sample.rgba[2] as f32,
            ]),
            None => Err(BlendImageReadError {
                selector: "framebuffer color",
            }),
        },
        BlendColorInput::Blend => Ok([
            blend_color_register[0] as f32,
            blend_color_register[1] as f32,
            blend_color_register[2] as f32,
        ]),
        BlendColorInput::Fog => Ok([
            fog_color[0] as f32,
            fog_color[1] as f32,
            fog_color[2] as f32,
        ]),
    }
}

/// Resolve a [`BlendAlphaInput`] to its normalized `[0,1]` `f32` value.
/// Literal port of `blend_a` (`blend.rs:270-278`).
pub fn blend_a(input: BlendAlphaInput, combined: u8, shade: u8, fog: u8) -> f32 {
    let value = match input {
        BlendAlphaInput::Combined => combined,
        BlendAlphaInput::Fog => fog,
        BlendAlphaInput::Shade => shade,
        BlendAlphaInput::Zero => 0,
    };
    value as f32 / 255.0
}

/// Resolve a [`BlendBInput`] to its normalized `[0,1]` `f32` value. Literal
/// port of `blend_b` (`blend.rs:280-292`).
///
/// # Errors
/// [`BlendImageReadError`] if `input` is [`BlendBInput::FramebufferAlpha`]
/// and `memory` is `None`.
pub fn blend_b(
    input: BlendBInput,
    a: f32,
    memory: Option<BlendFramebufferSample>,
) -> Result<f32, BlendImageReadError> {
    match input {
        BlendBInput::OneMinusA => Ok(1.0 - a),
        BlendBInput::FramebufferAlpha => match memory {
            Some(sample) => Ok(sample.coverage_count as f32 / 8.0),
            None => Err(BlendImageReadError {
                selector: "framebuffer coverage alpha",
            }),
        },
        BlendBInput::One => Ok(1.0),
        BlendBInput::Zero => Ok(0.0),
    }
}

/// Full mode state this module needs from [`OtherMode`], gathered once per
/// draw call by a caller that already owns the decode. Mirrors the
/// reference's `BlenderState` (`cycle_count`, `cycles`, `blend_color`,
/// `fog_color`) but sources `cycle_count`/`cycles` directly from
/// [`OtherMode::cycle_type`]/`blender_cycle_1`/`blender_cycle_2` rather than
/// owning a parallel copy of that decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlendModeState {
    pub other_mode: OtherMode,
    pub blend_color_register: [u8; 4],
    pub fog_color: [u8; 4],
}

impl BlendModeState {
    /// Number of blender cycles this mode runs: `0` for Copy/Fill (blender
    /// fully bypassed, matching `BlenderState.cycle_count == 0`'s short
    /// circuit in the reference's `blend_fragment`, `blend.rs:164-166`), `1`
    /// for OneCycle, `2` for TwoCycle.
    pub const fn cycle_count(self) -> u8 {
        match self.other_mode.cycle_type() {
            CycleType::OneCycle => 1,
            CycleType::TwoCycle => 2,
            CycleType::Copy | CycleType::Fill => 0,
        }
    }

    pub const fn cycle(self, cycle_index: u8) -> ResolvedBlendCycle {
        match cycle_index {
            0 => ResolvedBlendCycle::from_wire(self.other_mode.blender_cycle_1()),
            _ => ResolvedBlendCycle::from_wire(self.other_mode.blender_cycle_2()),
        }
    }
}

/// The blender's final per-fragment output: an `[R, G, B, A]` byte quad
/// ready to write to the color framebuffer. Produced by [`blend_fragment`]
/// after both the running blend composite and the final memory-blend
/// (dual-source or bypass alpha) have been applied -- this is the same
/// "final RGB and alpha" the reference computes at `blend.rs:227-239`, not
/// an intermediate dual-source pair (see [`DualSourceBlendOutput`] for that
/// lower-level seam).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlendedFragment {
    pub rgba: [u8; 4],
}

/// Round-half-away-from-zero clamp to `[0,255]`, matching the reference's
/// `.round().clamp(0.0, 255.0) as u8` (`blend.rs:230-238`). RT64's actual
/// GPU blend-unit rounding mode is unverified (port card §1 nonclaim);
/// fixed-function blend hardware typically rounds differently from this
/// software round-to-nearest.
fn round_clamp_u8(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Evaluate the RDP blender selectors for one covered fragment across every
/// active cycle. Literal port of `blend_fragment`
/// (`crates/fn64-render-reference/src/raster/blend.rs:157-240`).
///
/// - `state.cycle_count() == 0` (Copy/Fill) returns `src` unchanged.
/// - Each cycle: without `FORCE_BL`, the *last* active cycle selects `P`
///   directly (bypass) and the memory-composite alpha becomes `0.0` if
///   `P == Framebuffer` else `1.0`; earlier cycles (and the last cycle when
///   `blend_enabled` is true) evaluate the full selector set.
/// - When `P` or `M` selects [`BlendColorInput::Framebuffer`], the cycle
///   routes through the dual-source composite (see
///   [`DualSourceBlendOutput`]) instead of the general A/B divide. **Both**
///   `P` and `M` are resolved via [`blend_color`] *before* this dispatch
///   runs (matching the reference's own unconditional
///   `blend.rs:191-192` evaluation order exactly), so a `Framebuffer`
///   selector on either input requires a memory sample even on the branch
///   whose resolved value is then discarded -- e.g. `P == Framebuffer`
///   still requires `memory` to resolve `P`'s (unused) color, not only
///   `M`'s (used) one.
/// - Otherwise: `a == 0.0` collapses to `M`; `b == 0.0` (with `a != 0.0`)
///   collapses to `P`; else the general `(P*A + M*B) / (A+B)` divide runs.
/// - The final composite blends the last cycle's `blender_rgb`/`final_alpha`
///   against the memory sample (or nothing, when `final_alpha == 1.0`) --
///   asserting (as a loud [`BlendImageReadError`]) that an `IM_RD`-disabled
///   fragment never needed a framebuffer-dependent composite.
///
/// # Errors
/// [`BlendImageReadError`] if any active cycle's selectors require a
/// framebuffer sample that `memory` does not supply, including the final
/// composite step.
pub fn blend_fragment(
    src: [u8; 4],
    memory: Option<BlendFramebufferSample>,
    shade_alpha: u8,
    state: BlendModeState,
    blend_enabled: bool,
) -> Result<BlendedFragment, BlendImageReadError> {
    let cycle_count = state.cycle_count();
    if cycle_count == 0 {
        return Ok(BlendedFragment { rgba: src });
    }

    let src_rgb = [src[0] as f32, src[1] as f32, src[2] as f32];
    let mut blender_rgb = src_rgb;
    let mut final_alpha = 1.0_f32;

    for cycle_index in 0..cycle_count {
        let cycle = state.cycle(cycle_index);
        let is_last = cycle_index + 1 == cycle_count;
        let is_first = cycle_index == 0;

        if is_last && !blend_enabled {
            blender_rgb = blend_color(
                cycle.p,
                src_rgb,
                memory,
                state.blend_color_register,
                state.fog_color,
                is_first,
                blender_rgb,
            )?;
            final_alpha = if cycle.p == BlendColorInput::Framebuffer {
                0.0
            } else {
                1.0
            };
            continue;
        }

        let a = blend_a(cycle.a, src[3], shade_alpha, state.fog_color[3]);
        let p = blend_color(
            cycle.p,
            src_rgb,
            memory,
            state.blend_color_register,
            state.fog_color,
            is_first,
            blender_rgb,
        )?;
        let m = blend_color(
            cycle.m,
            src_rgb,
            memory,
            state.blend_color_register,
            state.fog_color,
            is_first,
            blender_rgb,
        )?;

        if cycle.p == BlendColorInput::Framebuffer {
            blender_rgb = m;
            final_alpha = 1.0 - a;
        } else if cycle.m == BlendColorInput::Framebuffer {
            blender_rgb = p;
            final_alpha = a;
        } else {
            let b = blend_b(cycle.b, a, memory)?;
            if a == 0.0 {
                blender_rgb = m;
            } else if b == 0.0 {
                blender_rgb = p;
            } else {
                let divisor = a + b;
                for channel in 0..3 {
                    blender_rgb[channel] =
                        ((p[channel] * a + m[channel] * b) / divisor).clamp(0.0, 255.0);
                }
            }
            final_alpha = 1.0;
        }
    }

    let dst = memory.map(|sample| sample.rgba);
    if dst.is_none() && final_alpha != 1.0 {
        return Err(BlendImageReadError {
            selector: "framebuffer color",
        });
    }
    let mut out_rgb = [0u8; 3];
    for channel in 0..3 {
        let memory_channel = dst.map_or(0.0, |rgba| rgba[channel] as f32);
        out_rgb[channel] = round_clamp_u8(
            blender_rgb[channel] * final_alpha + memory_channel * (1.0 - final_alpha),
        );
    }
    let memory_alpha = dst.map_or(0.0, |rgba| rgba[3] as f32);
    let alpha = round_clamp_u8(255.0 * final_alpha + memory_alpha * (1.0 - final_alpha));
    Ok(BlendedFragment {
        rgba: [out_rgb[0], out_rgb[1], out_rgb[2], alpha],
    })
}

/// RT64's actual per-fragment GPU output for a `Framebuffer`-selecting
/// blend cycle: two fragment-shader outputs consumed by the fixed-function
/// dual-source blend unit, matching `@blend_src(0)`/`@blend_src(1)` in WGSL
/// and `wgpu::BlendFactor::Src1`/`Src1Alpha` in the pipeline's
/// `wgpu::BlendState` (proved executable by M2.2's `execute_dual_source`,
/// `probes/m2-wgpu-metal-headless/src/bin/metal_semantics.rs:616-`). `source`
/// is the non-framebuffer input's resolved color (this cycle's `M` when `P`
/// selects `Framebuffer`, or `P` when `M` does); `source1` is that input's
/// blend factor, broadcast across RGB and written to alpha, matching how
/// `blend_fragment` computes `final_alpha` for the same two cases (`1.0 - a`
/// when `P` is `Framebuffer`, `a` when `M` is). This type carries only the
/// values a real draw call's fragment shader would emit -- it does not read
/// or write an actual render target, and nothing in this module submits a
/// GPU draw call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DualSourceBlendOutput {
    /// Primary fragment output (`@blend_src(0)`): the resolved non-framebuffer
    /// input color, `[R, G, B]` in `[0,255]`.
    pub source: [f32; 3],
    /// Secondary fragment output (`@blend_src(1)`): the blend factor applied
    /// uniformly to RGB and alpha by the fixed-function blend unit, in
    /// `[0,1]`.
    pub source1: f32,
}

/// Compute the dual-source fragment-shader output pair for one blend cycle
/// whose `P` or `M` selects [`BlendColorInput::Framebuffer`]. Returns `None`
/// if neither selector is `Framebuffer` (the cycle should use the general
/// A/B divide instead, via [`blend_fragment`]'s own dispatch).
///
/// `P == Framebuffer` takes precedence over `M == Framebuffer` when both
/// selectors name it -- a legally reachable wire state (`color_a` and
/// `color_b` are independent 2-bit fields) -- exactly matching
/// [`blend_fragment`]'s own `if cycle.p == Framebuffer { .. } else if
/// cycle.m == Framebuffer { .. }` precedence. In that combination `M`'s own
/// resolved color legitimately *is* the framebuffer sample, so this
/// function requires `memory` whenever `M` also selects `Framebuffer` in
/// the `P == Framebuffer` branch (and symmetrically for `P` in the `M ==
/// Framebuffer` branch), rather than treating the combination as
/// unreachable.
///
/// # Panics
/// If the non-primary selector (`M` in the `P == Framebuffer` branch, `P`
/// in the `M == Framebuffer` branch) also selects `Framebuffer` and
/// `memory` is `None` -- matching [`blend_fragment`]'s own
/// [`BlendImageReadError`] trigger for the identical wire state, but
/// surfaced as a panic here since this function's `Option`-only signature
/// has no error channel (this seam does not yet have a caller; a future
/// caller wiring this to a real draw path should thread `memory` through
/// and reject the same way [`blend_fragment`] does).
#[allow(clippy::too_many_arguments)]
pub fn dual_source_blend_output(
    cycle: ResolvedBlendCycle,
    src_rgb: [f32; 3],
    running_rgb: [f32; 3],
    is_first_cycle: bool,
    blend_color_register: [u8; 4],
    fog_color: [u8; 4],
    combined_alpha: u8,
    shade_alpha: u8,
    memory: Option<BlendFramebufferSample>,
) -> Option<DualSourceBlendOutput> {
    let a = blend_a(cycle.a, combined_alpha, shade_alpha, fog_color[3]);
    if cycle.p == BlendColorInput::Framebuffer {
        let m = blend_color(
            cycle.m,
            src_rgb,
            memory,
            blend_color_register,
            fog_color,
            is_first_cycle,
            running_rgb,
        )
        .expect("cycle.m selects Framebuffer while no memory sample was supplied");
        Some(DualSourceBlendOutput {
            source: m,
            source1: 1.0 - a,
        })
    } else if cycle.m == BlendColorInput::Framebuffer {
        let p = blend_color(
            cycle.p,
            src_rgb,
            memory,
            blend_color_register,
            fog_color,
            is_first_cycle,
            running_rgb,
        )
        .expect("cycle.p selects Framebuffer while no memory sample was supplied");
        Some(DualSourceBlendOutput {
            source: p,
            source1: a,
        })
    } else {
        None
    }
}

/// RT64's manual-blend fallback arithmetic, proved executable by M2.2's
/// `execute_manual_blend`
/// (`probes/m2-wgpu-metal-headless/src/bin/metal_semantics.rs:584-614`) for
/// GPUs that do not advertise `wgpu::Features::DUAL_SOURCE_BLENDING`. Not a
/// third blend model: this computes the *same* dual-source composite
/// ([`DualSourceBlendOutput`]'s `source`/`source1`) against a supplied
/// destination color, using the exact per-channel integer formula
/// `(src*factor + dst*(255-factor) + 127) / 255` (round-to-nearest via the
/// `+127` bias before the `/255` integer divide) that the fixed-function
/// blend unit's `Src1`/`OneMinusSrc1` factors would otherwise compute in
/// hardware.
pub fn manual_blend_composite(output: DualSourceBlendOutput, destination_rgba: [u8; 4]) -> [u8; 4] {
    let factor = (output.source1.clamp(0.0, 1.0) * 255.0).round() as u32;
    let mut result = [0u8; 4];
    for channel in 0..3 {
        let source_channel = output.source[channel].round().clamp(0.0, 255.0) as u32;
        let destination_channel = destination_rgba[channel] as u32;
        let numerator = source_channel * factor + destination_channel * (255 - factor) + 127;
        result[channel] = (numerator / 255) as u8;
    }
    // Alpha channel uses the same factor broadcast, matching
    // `Src1Alpha`/`OneMinusSrc1Alpha` reading the identical `source1` value.
    let source_alpha = (output.source1.clamp(0.0, 1.0) * 255.0).round() as u32;
    let destination_alpha = destination_rgba[3] as u32;
    let alpha_numerator = source_alpha * factor + destination_alpha * (255 - factor) + 127;
    result[3] = (alpha_numerator / 255) as u8;
    result
}

pub const BLEND_WGSL: &str = include_str!("shaders/blend.wgsl");
pub const BLEND_ENTRY_POINT: &str = "blend_fragment_cycle";

/// Fragment-callable admitted-subset blend library (production blend wiring
/// slice 1): concatenated into the production triangle fragment shader by
/// `shader_manifest.rs`, same mechanism as `alpha_compare.rs`'s/
/// `coverage.rs`'s own `*_FRAGMENT_FN_WGSL` constants. See
/// `shaders/blend_fragment_fn.wgsl`'s own header for scope.
pub const BLEND_FRAGMENT_FN_WGSL: &str = include_str!("shaders/blend_fragment_fn.wgsl");

#[cfg(test)]
mod tests;
