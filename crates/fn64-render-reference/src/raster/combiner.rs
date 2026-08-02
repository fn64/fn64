use crate::gbi::*;
use crate::depth::EncodedDepth;
use super::*;

/// Evaluate both programmed RDP color-combiner cycles.
///
/// Each cycle computes `(A - B) * C + D` independently for RGB and alpha.
/// The source meanings follow RT64's MIT `shared/rt64_color_combiner.h`
/// `fromColorInput`/`fromAlphaInput` (lines 468-540), and the equation/cycle
/// ordering follows `runCycle` (lines 567-608). The decoded presets duplicate
/// inactive one-cycle terms, while OoT's PASS2/`*2` presets consume COMBINED
/// and therefore require the sequential two-cycle result.
#[derive(Copy, Clone)]
pub(super) struct CombinerPixel {
    pub(super) lod_fraction: f32,
    pub(super) shade: [u8; 4],
    pub(super) texel0: [u8; 4],
    pub(super) texel1: [u8; 4],
    pub(super) noise: NoiseSample,
}

#[cfg(test)]
impl CombinerPixel {
    pub(super) fn new(
        lod_fraction: f32,
        shade: [u8; 4],
        texel0: [u8; 4],
        texel1: [u8; 4],
        noise: NoiseSample,
    ) -> Self {
        Self {
            lod_fraction,
            shade,
            texel0,
            texel1,
            noise,
        }
    }
}

pub(super) fn evaluate_combiner(
    state: CombinerState,
    cycle_type: CycleType,
    key_enabled: bool,
    pixel: CombinerPixel,
) -> [u8; 4] {
    let to_unit = |rgba: [u8; 4]| rgba.map(|v| v as f32 / 255.0);
    let mut inputs = CombinerInputs {
        combined: [0.0; 4],
        texel0: to_unit(pixel.texel0),
        texel1: to_unit(pixel.texel1),
        primitive: to_unit(state.primitive),
        shade: to_unit(pixel.shade),
        environment: to_unit(state.environment),
        lod_fraction: pixel.lod_fraction,
        prim_lod_fraction: state.prim_lod_fraction as f32 / 255.0,
        k4: state.convert.k4(),
        k5: state.convert.k5(),
        key_center: state.key.center_unit(),
        key_scale: state.key.scale_unit(),
        noise: pixel.noise.unit(),
    };

    let cycle_count = match cycle_type {
        CycleType::OneCycle => 1,
        CycleType::TwoCycle => 2,
        CycleType::Copy | CycleType::Fill => {
            unreachable!("copy/fill cycle reached color combiner")
        }
    };
    for cycle in state.mode.cycles.into_iter().take(cycle_count) {
        inputs.combined = evaluate_cycle(cycle, &inputs);
    }
    if key_enabled {
        inputs.combined[3] = state.key.alpha_from_key_prime([
            inputs.combined[0],
            inputs.combined[1],
            inputs.combined[2],
        ]);
    }

    inputs
        .combined
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[derive(Copy, Clone)]
pub(super) struct CombinerInputs {
    combined: [f32; 4],
    texel0: [f32; 4],
    texel1: [f32; 4],
    primitive: [f32; 4],
    shade: [f32; 4],
    environment: [f32; 4],
    lod_fraction: f32,
    prim_lod_fraction: f32,
    k4: f32,
    k5: f32,
    key_center: [f32; 3],
    key_scale: [f32; 3],
    noise: f32,
}

pub(super) fn evaluate_cycle(cycle: CombinerCycle, inputs: &CombinerInputs) -> [f32; 4] {
    let a = color_input(cycle.rgb[0], inputs);
    let b = color_input(cycle.rgb[1], inputs);
    let c = color_input(cycle.rgb[2], inputs);
    let d = color_input(cycle.rgb[3], inputs);
    let mut out = [0.0; 4];
    for channel in 0..3 {
        out[channel] = (a[channel] - b[channel]) * c[channel] + d[channel];
    }

    let aa = alpha_input(cycle.alpha[0], inputs);
    let ab = alpha_input(cycle.alpha[1], inputs);
    let ac = alpha_input(cycle.alpha[2], inputs);
    let ad = alpha_input(cycle.alpha[3], inputs);
    out[3] = (aa - ab) * ac + ad;
    out
}

pub(super) fn color_input(source: ColorSource, inputs: &CombinerInputs) -> [f32; 3] {
    let rgb = |rgba: [f32; 4]| [rgba[0], rgba[1], rgba[2]];
    let splat = |v| [v; 3];
    match source {
        ColorSource::Combined => rgb(inputs.combined),
        ColorSource::Texel0 => rgb(inputs.texel0),
        ColorSource::Texel1 => rgb(inputs.texel1),
        ColorSource::Primitive => rgb(inputs.primitive),
        ColorSource::Shade => rgb(inputs.shade),
        ColorSource::Environment => rgb(inputs.environment),
        ColorSource::CombinedAlpha => splat(inputs.combined[3]),
        ColorSource::Texel0Alpha => splat(inputs.texel0[3]),
        ColorSource::Texel1Alpha => splat(inputs.texel1[3]),
        ColorSource::PrimitiveAlpha => splat(inputs.primitive[3]),
        ColorSource::ShadeAlpha => splat(inputs.shade[3]),
        ColorSource::EnvironmentAlpha => splat(inputs.environment[3]),
        ColorSource::LodFraction => splat(inputs.lod_fraction),
        ColorSource::PrimLodFraction => splat(inputs.prim_lod_fraction),
        ColorSource::One => [1.0; 3],
        ColorSource::Zero => [0.0; 3],
        ColorSource::K4 => splat(inputs.k4),
        ColorSource::K5 => splat(inputs.k5),
        ColorSource::KeyCenter => inputs.key_center,
        ColorSource::KeyScale => inputs.key_scale,
        ColorSource::Noise => splat(inputs.noise),
    }
}

pub(super) fn alpha_input(source: AlphaSource, inputs: &CombinerInputs) -> f32 {
    match source {
        AlphaSource::Combined => inputs.combined[3],
        AlphaSource::Texel0 => inputs.texel0[3],
        AlphaSource::Texel1 => inputs.texel1[3],
        AlphaSource::Primitive => inputs.primitive[3],
        AlphaSource::Shade => inputs.shade[3],
        AlphaSource::Environment => inputs.environment[3],
        AlphaSource::LodFraction => inputs.lod_fraction,
        AlphaSource::PrimLodFraction => inputs.prim_lod_fraction,
        AlphaSource::One => 1.0,
        AlphaSource::Zero => 0.0,
    }
}
