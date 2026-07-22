//! Strict interchange for externally captured synthetic raw-DPC vectors.
//!
//! This crate validates evidence envelopes; it does not execute RDP commands
//! and cannot promote an emulator or synthetic fixture to hardware evidence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod consensus;

pub use consensus::{validate_hardware_consensus, ConsensusRun, HardwareConsensus};

pub const SCHEMA: &str = "fn64.rdp-silicon-vectors.v1";
pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CASES: usize = 4096;
const MAX_BLOB_BYTES: usize = 8 * 1024 * 1024;
const RDRAM_END: u32 = 0x0080_0000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerKind {
    Hardware,
    BlackBox,
    SyntheticFixture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    pub kind: ProducerKind,
    pub name: String,
    pub version: String,
    pub platform: String,
    pub adapter: String,
    pub adapter_version: String,
    pub producer_binary_sha256: String,
    pub settings_sha256: String,
    pub capture_method: String,
    pub recorded_at_utc: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blob {
    pub byte_len: u64,
    pub sha256: String,
    pub bytes_hex: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterName {
    DpcStart,
    DpcEnd,
    DpcStatus,
    MiIntrMask,
    ViControl,
    ViOrigin,
    ViWidth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterValue {
    pub name: RegisterName,
    pub value: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRole {
    Texture,
    ColorImage,
    DepthImage,
    Auxiliary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRegion {
    pub region_id: String,
    pub role: MemoryRole,
    pub address: u32,
    pub contents: Blob,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub registers: Vec<RegisterValue>,
    pub initial_memory: Vec<MemoryRegion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramebufferEncoding {
    Rgba16BigEndian,
    Rgba32BigEndian,
    Ci8,
}

impl FramebufferEncoding {
    fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba16BigEndian => 2,
            Self::Rgba32BigEndian => 4,
            Self::Ci8 => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramebufferPlane {
    pub address: u32,
    pub width: u32,
    pub height: u32,
    pub row_stride_bytes: u32,
    pub encoding: FramebufferEncoding,
    pub contents: Blob,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepthPlane {
    pub address: u32,
    pub width: u32,
    pub height: u32,
    pub row_stride_bytes: u32,
    pub contents: Blob,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageEncoding {
    /// One byte per pixel; only the low two physical hidden bits may be set.
    Rgba16HiddenBitsU2,
    /// One normalized byte per pixel containing stored coverage 0..=7.
    StoredCoverageU3,
    /// One normalized byte per pixel containing actual coverage 0..=8.
    CoverageCountU4,
}

/// RDP cycle mode declared by a synthetic capture probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeCycleType {
    OneCycle,
    TwoCycle,
}

/// Public RGB-dither selector encoded by Other Modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RgbDitherMode {
    MagicSquare,
    Bayer,
    Noise,
    Disabled,
}

/// RGBA16 channel isolated by a controlled RGB-dither sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RgbDitherChannel {
    Red,
    Green,
    Blue,
}

/// Named depth relation selected by a controlled `ZMODE_INTER` probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZModeInterRelation {
    InFrontControl,
    Interpenetrating,
    BehindControl,
}

/// Exact position around a producer-declared three-nearest fractional
/// diagonal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterTieBoundaryPosition {
    Below,
    On,
    Above,
}

/// Exact position around a producer-declared reciprocal-to-S10.5 boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReciprocalBoundaryPosition {
    Below,
    On,
    Above,
}

/// Exact position around a producer-declared average-filter accumulator tie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AverageFilterTiePosition {
    Below,
    On,
    Above,
}

/// RGBA channel isolated by an average-filter output-tie experiment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AverageFilterChannel {
    Red,
    Green,
    Blue,
    Alpha,
}

/// Public texture-LOD selection family isolated by a boundary experiment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureLodMode {
    Mip,
    Detail,
    Sharpen,
}

/// Exact position around a producer-declared texture-LOD boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureLodBoundaryPosition {
    Below,
    On,
    Above,
}

/// Producer-declared blender path isolated by the precision matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlenderProbeMode {
    Ordinary,
    ForceBlend,
    FogPass,
}

/// Exact position around a producer-declared blender denominator boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlenderDenominatorPosition {
    Below,
    On,
    Above,
}

/// Exact producer-declared depth controls for one relation label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZModeInterControls {
    pub incoming_z_u18: u32,
    pub memory_z_u18: u32,
    pub incoming_delta_z_u16: u16,
    pub memory_delta_z_u16: u16,
}

/// Independently encoded attribute used to expose the covered checkerboard
/// sample selected for fragment interpolation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentativeSampleObservable {
    Shade,
    Texture,
    Depth,
}

/// Exact raw-accumulator position around a producer-declared narrow-edge
/// boundary. The integer step is one least-significant bit at the declared
/// fixed-point scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NarrowEdgeBoundaryPosition {
    Below,
    On,
    Above,
}

/// Fixed marker domains for one representative-sample experiment. Every
/// sample index has one unique value per observable, while the control values
/// make the inactive output channels auditable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeSampleMarkers {
    pub shade_rgba32_be: [u32; 8],
    pub texture_rgba32_be: [u32; 8],
    pub depth_u16_be: [u16; 8],
    pub depth_observable_color_control_rgba32_be: u32,
    pub color_observable_depth_control_u16_be: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeSampleControls {
    pub pixel_x: u16,
    pub pixel_y: u16,
    pub markers: RepresentativeSampleMarkers,
}

/// Fixed controls repeated by every point in a narrow-edge capture matrix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NarrowEdgeCoverageControls {
    pub pixel_x: u16,
    pub pixel_y: u16,
    pub edge_fractional_bits_u8: u8,
    pub selected_boundaries_i64: Vec<i64>,
    pub markers: RepresentativeSampleMarkers,
}

/// Typed intent for experiments whose cross-case shape needs validation in
/// addition to the generic evidence envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureIntent {
    /// One input-code point in a 4x4 RGB-dither tile. A complete sweep holds
    /// two channels fixed and varies the selected channel through 0..=255.
    RgbDitherSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        mode: RgbDitherMode,
        swept_channel: RgbDitherChannel,
        input_rgb8: [u8; 3],
        channel_value: u8,
        origin_x: u16,
        origin_y: u16,
        replay_from_reset: bool,
        sample_index: u32,
    },
    /// One point in a controlled 0..=255 combined-alpha sweep using
    /// `G_AC_DITHER`. The capture producer remains responsible for making the
    /// declared command semantics true; this metadata makes the intended
    /// cross-case experiment mechanically auditable.
    AlphaCompareDitherSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        combined_alpha: u8,
        replay_from_reset: bool,
        sample_index: u32,
        pass_rgba16_be: u16,
        reject_rgba16_be: u16,
    },
    /// One point in a controlled `CVG_X_ALPHA` transfer sweep. The target is
    /// cleared to full coverage and rendered with `CVG_DST_WRAP`, making the
    /// resulting normalized coverage count the observable product.
    AlphaCoverageProductSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        input_coverage: u8,
        combined_alpha: u8,
        replay_from_reset: bool,
    },
    /// One threshold point observing `ALPHA_CVG_SEL` through
    /// `G_AC_THRESHOLD`. A complete sweep identifies the selected pixel-alpha
    /// code for every input coverage without using blender RGB as a proxy.
    CoverageToAlphaSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        input_coverage: u8,
        threshold_alpha: u8,
        replay_from_reset: bool,
        pass_rgba16_be: u16,
        reject_rgba16_be: u16,
    },
    /// One point in the bounded `ZMODE_INTER` admission and stored-coverage
    /// experiment. Numeric controls are explicit so a relation label cannot
    /// silently stand in for a different geometry.
    ZModeInterCoverageSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        relation: ZModeInterRelation,
        incoming_coverage: u8,
        initial_stored_coverage: u8,
        replay_from_reset: bool,
        pass_rgba16_be: u16,
        reject_rgba16_be: u16,
        incoming_z_u18: u32,
        memory_z_u18: u32,
        incoming_delta_z_u16: u16,
        memory_delta_z_u16: u16,
    },
    /// One point in the exhaustive nonzero eight-sample coverage-mask sweep.
    /// Shade, texture, and depth probes are independent so agreement is an
    /// observation rather than an assumption embedded in the schema.
    RepresentativeSampleSelectorSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        observable: RepresentativeSampleObservable,
        coverage_mask_u8: u8,
        replay_from_reset: bool,
        controls: RepresentativeSampleControls,
    },
    /// One independently reset shade, texture, or depth observation at a raw
    /// edge-accumulator value exactly below, on, or above a selected boundary.
    /// Mask/count and boundary labels are producer assertions retained for
    /// audit; they do not encode a silicon correction formula.
    NarrowEdgeCoverageCorrectionSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        observable: RepresentativeSampleObservable,
        boundary_position: NarrowEdgeBoundaryPosition,
        replay_from_reset: bool,
        controls: NarrowEdgeCoverageControls,
        edge_boundary_i64: i64,
        edge_accumulator_i64: i64,
        coverage_mask_u8: u8,
        coverage_count_u4: u8,
    },
    /// One independently reset point immediately below, on, or above a
    /// three-nearest filter diagonal. Exact numeric controls make the intended
    /// boundary auditable without claiming that opaque commands implement it.
    TextureFilterTieSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        boundary_position: FilterTieBoundaryPosition,
        replay_from_reset: bool,
        sample_x: u16,
        sample_y: u16,
        texture_address: u32,
        texel_rgba16_be: [u16; 4],
        s_texel_i10: i16,
        t_texel_i10: i16,
        s_fraction_u5: u8,
        t_fraction_u5: u8,
        diagonal_boundary_u6: u8,
    },
    /// One independently reset rational input immediately below, on, or above
    /// a signed S10.5 boundary. All arithmetic and output expectations are
    /// producer declarations; the analyzer preserves observations without
    /// deriving a silicon reciprocal rule.
    ReciprocalS10_5BoundarySweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        boundary_position: ReciprocalBoundaryPosition,
        replay_from_reset: bool,
        sample_x: u16,
        sample_y: u16,
        boundary_s10_5_i16: i16,
        perspective_numerator_i64: i64,
        perspective_denominator_u64: u64,
        producer_expected_output_s10_5_i16: i16,
        producer_expected_framebuffer_rgba32_be: u32,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
    },
    /// One independently reset rational accumulator point immediately below,
    /// on, or above an average-filter output tie. The declared accumulator is
    /// auditable metadata, not a silicon formula derived by this tool.
    AverageFilterOutputTieSweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        tie_position: AverageFilterTiePosition,
        replay_from_reset: bool,
        sample_x: u16,
        sample_y: u16,
        texture_address: u32,
        texel_rgba16_be: [u16; 4],
        s_texel_i10: i16,
        t_texel_i10: i16,
        s_fraction_u5: u8,
        t_fraction_u5: u8,
        isolated_channel: AverageFilterChannel,
        accumulator_numerator_i64: i64,
        accumulator_denominator_u64: u64,
        tie_numerator_i64: i64,
        producer_expected_output_u8: u8,
        producer_expected_framebuffer_rgba32_be: u32,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
    },
    /// One independently reset derivative/LOD point. Coordinates and
    /// derivatives are exact; the rational LOD metric and expected selection
    /// remain producer declarations rather than a formula inferred here.
    TextureLodBoundarySweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        lod_mode: TextureLodMode,
        boundary_position: TextureLodBoundaryPosition,
        replay_from_reset: bool,
        sample_x: u16,
        sample_y: u16,
        center_s_s10_5_i16: i16,
        center_t_s10_5_i16: i16,
        x_neighbor_s_s10_5_i16: i16,
        x_neighbor_t_s10_5_i16: i16,
        y_neighbor_s_s10_5_i16: i16,
        y_neighbor_t_s10_5_i16: i16,
        dsdx_s10_5_i32: i32,
        dtdx_s10_5_i32: i32,
        dsdy_s10_5_i32: i32,
        dtdy_s10_5_i32: i32,
        lod_metric_numerator_i64: i64,
        lod_metric_denominator_u64: u64,
        lod_boundary_numerator_i64: i64,
        primitive_tile_u3: u8,
        max_mip_level_u3: u8,
        min_lod_u8: u8,
        producer_expected_tile0_u3: u8,
        producer_expected_tile1_u3: u8,
        producer_expected_lod_fraction_s9_8_i16: i16,
        producer_expected_framebuffer_rgba32_be: u32,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
    },
    /// One reset-isolated point in a finite blender precision matrix. Alpha,
    /// denominator, and expected output are producer declarations; this tool
    /// checks the matrix geometry but does not infer a division formula.
    BlenderPrecisionBoundarySweep {
        sweep_id: String,
        cycle_type: ProbeCycleType,
        mode: BlenderProbeMode,
        isolated_alpha_u5: u8,
        denominator_position: BlenderDenominatorPosition,
        replay_from_reset: bool,
        sample_x: u16,
        sample_y: u16,
        denominator_boundary_u6: u8,
        producer_declared_denominator_u6: u8,
        pixel_color_rgba32_be: u32,
        memory_color_rgba32_be: u32,
        fog_color_rgba32_be: u32,
        producer_expected_framebuffer_rgba32_be: u32,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
    },
    /// One reset-started two-cycle draw containing exactly two horizontally
    /// adjacent pixels. The command digest explicitly binds their ordered
    /// producer provenance while distinct candidate markers expose whether
    /// the second pixel follows cycle-one handoff or prior-memory timing.
    BlenderMemoryFeedbackPair {
        sweep_id: String,
        mode: BlenderProbeMode,
        cycle_type: ProbeCycleType,
        replay_from_reset: bool,
        first_pixel_x: u16,
        first_pixel_y: u16,
        second_pixel_x: u16,
        second_pixel_y: u16,
        ordered_pair_command_sha256: String,
        cycle_one_handoff_color_rgba32_be: u32,
        prior_memory_color_rgba32_be: u32,
        cycle_one_handoff_coverage_u3: u8,
        prior_memory_coverage_u3: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoveragePlane {
    pub color_image_address: u32,
    pub width: u32,
    pub height: u32,
    pub encoding: CoverageEncoding,
    pub contents: Blob,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedOutputs {
    pub framebuffer: FramebufferPlane,
    pub depth: DepthPlane,
    pub coverage: CoveragePlane,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorCase {
    pub case_id: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_intent: Option<CaptureIntent>,
    pub command_bytes: Blob,
    pub setup: Setup,
    pub expected: ExpectedOutputs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaDitherCycleTransition {
    pub cycle_type: ProbeCycleType,
    /// `None` preserves an all-rejected sweep; exact endpoint/tie behavior is
    /// one of the silicon facts this experiment is intended to discover.
    pub first_passing_alpha: Option<u8>,
    pub first_reject_after_pass: Option<u8>,
    pub pass_count: u16,
    pub transition_count: u16,
    pub monotonic_reject_then_pass: bool,
    /// Alpha 0 is bit zero of byte zero; alpha 255 is bit seven of byte 31.
    pub pass_bitmap_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaDitherSweepAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub sweep_id: String,
    pub sample_index: u32,
    pub pass_rgba16_be: u16,
    pub reject_rgba16_be: u16,
    pub transitions: Vec<AlphaDitherCycleTransition>,
    pub cycle_transitions_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RgbDitherCycleAnalysis {
    pub cycle_type: ProbeCycleType,
    /// Two lowercase hexadecimal digits per observed five-bit channel code,
    /// ordered by input value, then framebuffer row, then column.
    pub channel_u5_hex: String,
    pub distinct_channel_codes: u8,
    pub monotonic_per_pixel: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RgbDitherSweepAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub sweep_id: String,
    pub mode: RgbDitherMode,
    pub swept_channel: RgbDitherChannel,
    pub fixed_rgb8: [u8; 3],
    pub origin_x: u16,
    pub origin_y: u16,
    pub sample_index: u32,
    pub framebuffer_address: u32,
    pub cycles: Vec<RgbDitherCycleAnalysis>,
    pub cycle_results_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RgbDitherControls {
    mode: RgbDitherMode,
    swept_channel: RgbDitherChannel,
    fixed_rgb8: [u8; 3],
    origin_x: u16,
    origin_y: u16,
    sample_index: u32,
    framebuffer_address: u32,
    depth_address: u32,
    coverage_address: u32,
    coverage_encoding: CoverageEncoding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaCoverageProductCurve {
    pub cycle_type: ProbeCycleType,
    pub input_coverage: u8,
    /// Raw alpha-zero observation. A rejected zero-product fragment may leave
    /// the target's cleared full coverage (8) untouched.
    pub alpha_zero_output_coverage: u8,
    pub first_nonzero_alpha: Option<u8>,
    pub first_full_alpha: Option<u8>,
    pub transition_count: u16,
    pub monotonic_nondecreasing_from_alpha_one: bool,
    /// One lowercase hexadecimal digit per combined-alpha value, ordered
    /// alpha 0 through 255. Every digit is an observed coverage count 0..=8.
    pub coverage_u4_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaCoverageProductAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub sweep_id: String,
    pub curves: Vec<AlphaCoverageProductCurve>,
    pub cycle_curves_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageToAlphaCurve {
    pub cycle_type: ProbeCycleType,
    pub input_coverage: u8,
    pub greatest_passing_threshold: Option<u8>,
    pub first_pass_after_reject: Option<u8>,
    pub pass_count: u16,
    pub transition_count: u16,
    pub monotonic_pass_then_reject: bool,
    /// Threshold 0 is bit zero of byte zero; threshold 255 is bit seven of
    /// byte 31.
    pub pass_bitmap_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageToAlphaAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub sweep_id: String,
    pub pass_rgba16_be: u16,
    pub reject_rgba16_be: u16,
    pub curves: Vec<CoverageToAlphaCurve>,
    pub cycle_curves_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZModeInterGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub color_image_address: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZModeInterRelationAnalysis {
    pub cycle_type: ProbeCycleType,
    pub relation: ZModeInterRelation,
    pub controls: ZModeInterControls,
    pub admitted_count: u8,
    /// Point `(incoming_coverage - 1) * 8 + initial_stored_coverage` is one
    /// bit, least-significant bit first, in this exact 64-point bitmap.
    pub admission_bitmap_hex: String,
    /// One lowercase hexadecimal digit per point in the same order. Each
    /// digit is the exact observed stored coverage 0..=7.
    pub stored_coverage_u3_hex: String,
    pub changed_from_initial_count: u8,
    pub rejected_coverage_changed_count: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZModeInterAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub pass_rgba16_be: u16,
    pub reject_rgba16_be: u16,
    pub geometry: ZModeInterGeometry,
    pub relations: Vec<ZModeInterRelationAnalysis>,
    pub cycle_results_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeSampleGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub color_image_address: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeSampleSelectorTable {
    pub cycle_type: ProbeCycleType,
    pub observable: RepresentativeSampleObservable,
    /// One lowercase hexadecimal digit per nonzero coverage mask, ordered
    /// mask 0x01 through 0xff. Every digit is selected sample index 0..=7.
    pub selected_sample_u3_hex: String,
    pub selected_sample_counts: [u16; 8],
    pub uncovered_selection_count: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeSampleCycleComparison {
    pub observable: RepresentativeSampleObservable,
    pub matches: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeSampleObservableComparison {
    pub cycle_type: ProbeCycleType,
    pub shade_texture_match: bool,
    pub shade_depth_match: bool,
    pub texture_depth_match: bool,
    pub all_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeSampleSelectorAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub controls: RepresentativeSampleControls,
    pub geometry: RepresentativeSampleGeometry,
    pub tables: Vec<RepresentativeSampleSelectorTable>,
    pub cycle_comparisons: Vec<RepresentativeSampleCycleComparison>,
    pub observable_comparisons: Vec<RepresentativeSampleObservableComparison>,
    pub all_cycle_results_match: bool,
    pub all_observable_results_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NarrowEdgeCoverageGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub color_image_address: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NarrowEdgeObservableObservation {
    pub observable: RepresentativeSampleObservable,
    pub framebuffer_rgba32_be: u32,
    pub depth_u16_be: u16,
    pub observed_coverage_count_u4: u8,
    pub observed_sample_index_u3: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NarrowEdgePointAnalysis {
    pub cycle_type: ProbeCycleType,
    pub boundary_position: NarrowEdgeBoundaryPosition,
    pub edge_accumulator_i64: i64,
    pub coverage_mask_u8: u8,
    pub coverage_count_u4: u8,
    pub observations: Vec<NarrowEdgeObservableObservation>,
    pub observable_sample_indices_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NarrowEdgeBoundaryAnalysis {
    pub edge_boundary_i64: i64,
    pub points: Vec<NarrowEdgePointAnalysis>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NarrowEdgeCoverageAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub controls: NarrowEdgeCoverageControls,
    pub geometry: NarrowEdgeCoverageGeometry,
    pub boundaries: Vec<NarrowEdgeBoundaryAnalysis>,
    pub all_cycle_results_match: bool,
    pub all_observable_sample_indices_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureFilterTieGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub coverage_address: u32,
    pub sample_x: u16,
    pub sample_y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureFilterTieObservation {
    pub boundary_position: FilterTieBoundaryPosition,
    pub s_fraction_u5: u8,
    pub t_fraction_u5: u8,
    pub framebuffer_rgba32_be: u32,
    pub depth_u16_be: u16,
    pub stored_coverage_u3: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureFilterTieCycleAnalysis {
    pub cycle_type: ProbeCycleType,
    pub observations: Vec<TextureFilterTieObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureFilterTieAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub texture_address: u32,
    pub texel_rgba16_be: [u16; 4],
    pub s_texel_i10: i16,
    pub t_texel_i10: i16,
    pub diagonal_boundary_u6: u8,
    pub geometry: TextureFilterTieGeometry,
    pub cycles: Vec<TextureFilterTieCycleAnalysis>,
    pub cycle_results_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReciprocalS10_5Geometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub coverage_address: u32,
    pub sample_x: u16,
    pub sample_y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReciprocalS10_5Observation {
    pub boundary_position: ReciprocalBoundaryPosition,
    pub perspective_numerator_i64: i64,
    pub perspective_denominator_u64: u64,
    pub producer_expected_output_s10_5_i16: i16,
    pub producer_expected_framebuffer_rgba32_be: u32,
    pub framebuffer_rgba32_be: u32,
    /// Present only when the observed color equals one of this sweep's
    /// producer-declared output markers.
    pub observed_output_s10_5_i16: Option<i16>,
    pub output_matches_producer_expectation: bool,
    pub depth_u16_be: u16,
    pub stored_coverage_u3: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReciprocalS10_5CycleAnalysis {
    pub cycle_type: ProbeCycleType,
    pub observations: Vec<ReciprocalS10_5Observation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReciprocalS10_5Analysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub boundary_s10_5_i16: i16,
    pub depth_control_u16_be: u16,
    pub stored_coverage_control_u3: u8,
    pub geometry: ReciprocalS10_5Geometry,
    pub cycles: Vec<ReciprocalS10_5CycleAnalysis>,
    pub unexpected_output_count: u8,
    pub cycle_results_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AverageFilterTieGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub coverage_address: u32,
    pub sample_x: u16,
    pub sample_y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AverageFilterTieObservation {
    pub tie_position: AverageFilterTiePosition,
    pub s_fraction_u5: u8,
    pub t_fraction_u5: u8,
    pub accumulator_numerator_i64: i64,
    pub accumulator_denominator_u64: u64,
    pub producer_expected_output_u8: u8,
    pub producer_expected_framebuffer_rgba32_be: u32,
    pub framebuffer_rgba32_be: u32,
    /// Present only when the observed color equals one of this sweep's
    /// producer-declared output markers.
    pub observed_output_u8: Option<u8>,
    pub output_matches_producer_expectation: bool,
    pub depth_u16_be: u16,
    pub stored_coverage_u3: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AverageFilterTieCycleAnalysis {
    pub cycle_type: ProbeCycleType,
    pub observations: Vec<AverageFilterTieObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AverageFilterTieAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub texture_address: u32,
    pub texel_rgba16_be: [u16; 4],
    pub s_texel_i10: i16,
    pub t_texel_i10: i16,
    pub isolated_channel: AverageFilterChannel,
    pub tie_numerator_i64: i64,
    pub accumulator_denominator_u64: u64,
    pub depth_control_u16_be: u16,
    pub stored_coverage_control_u3: u8,
    pub geometry: AverageFilterTieGeometry,
    pub cycles: Vec<AverageFilterTieCycleAnalysis>,
    pub unexpected_output_count: u8,
    pub cycle_results_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureLodExpectedSelection {
    pub tile0_u3: u8,
    pub tile1_u3: u8,
    pub lod_fraction_s9_8_i16: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureLodGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub coverage_address: u32,
    pub sample_x: u16,
    pub sample_y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureLodObservation {
    pub boundary_position: TextureLodBoundaryPosition,
    pub center_s_s10_5_i16: i16,
    pub center_t_s10_5_i16: i16,
    pub x_neighbor_s_s10_5_i16: i16,
    pub x_neighbor_t_s10_5_i16: i16,
    pub y_neighbor_s_s10_5_i16: i16,
    pub y_neighbor_t_s10_5_i16: i16,
    pub dsdx_s10_5_i32: i32,
    pub dtdx_s10_5_i32: i32,
    pub dsdy_s10_5_i32: i32,
    pub dtdy_s10_5_i32: i32,
    pub lod_metric_numerator_i64: i64,
    pub lod_metric_denominator_u64: u64,
    pub producer_expected_selection: TextureLodExpectedSelection,
    pub producer_expected_framebuffer_rgba32_be: u32,
    pub framebuffer_rgba32_be: u32,
    /// Present only when the observed color equals a declared selection marker.
    pub observed_selection: Option<TextureLodExpectedSelection>,
    pub output_matches_producer_expectation: bool,
    pub depth_u16_be: u16,
    pub depth_matches_producer_control: bool,
    pub stored_coverage_u3: u8,
    pub coverage_matches_producer_control: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureLodCycleAnalysis {
    pub cycle_type: ProbeCycleType,
    pub observations: Vec<TextureLodObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureLodModeAnalysis {
    pub lod_mode: TextureLodMode,
    pub cycles: Vec<TextureLodCycleAnalysis>,
    pub cycle_results_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureLodBoundaryAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub lod_boundary_numerator_i64: i64,
    pub lod_metric_denominator_u64: u64,
    pub primitive_tile_u3: u8,
    pub max_mip_level_u3: u8,
    pub min_lod_u8: u8,
    pub depth_control_u16_be: u16,
    pub stored_coverage_control_u3: u8,
    pub geometry: TextureLodGeometry,
    pub modes: Vec<TextureLodModeAnalysis>,
    pub unexpected_output_count: u8,
    pub unexpected_depth_count: u8,
    pub unexpected_coverage_count: u8,
    pub all_cycle_results_match: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderPrecisionGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub coverage_address: u32,
    pub sample_x: u16,
    pub sample_y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderFeedbackGeometry {
    pub framebuffer_address: u32,
    pub depth_address: u32,
    pub coverage_address: u32,
    pub first_pixel_x: u16,
    pub first_pixel_y: u16,
    pub second_pixel_x: u16,
    pub second_pixel_y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderPrecisionObservation {
    pub isolated_alpha_u5: u8,
    pub denominator_position: BlenderDenominatorPosition,
    pub producer_declared_denominator_u6: u8,
    pub producer_expected_framebuffer_rgba32_be: u32,
    pub framebuffer_rgba32_be: u32,
    pub output_matches_producer_expectation: bool,
    pub depth_u16_be: u16,
    pub depth_matches_producer_control: bool,
    pub stored_coverage_u3: u8,
    pub coverage_matches_producer_control: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderPrecisionCycleAnalysis {
    pub cycle_type: ProbeCycleType,
    pub observations: Vec<BlenderPrecisionObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderPrecisionModeAnalysis {
    pub mode: BlenderProbeMode,
    pub cycles: Vec<BlenderPrecisionCycleAnalysis>,
    pub cycle_divergence_count: u8,
    pub cycle_results_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderMemoryFeedbackObservation {
    pub mode: BlenderProbeMode,
    pub cycle_type: ProbeCycleType,
    pub ordered_pair_command_sha256: String,
    pub framebuffer_rgba32_be: [u32; 2],
    pub depth_u16_be: [u16; 2],
    pub stored_coverage_u3: [u8; 2],
    pub cycle_one_handoff_color_rgba32_be: u32,
    pub prior_memory_color_rgba32_be: u32,
    pub cycle_one_handoff_coverage_u3: u8,
    pub prior_memory_coverage_u3: u8,
    pub second_color_matches_cycle_one_handoff: bool,
    pub second_color_matches_prior_memory: bool,
    pub second_coverage_matches_cycle_one_handoff: bool,
    pub second_coverage_matches_prior_memory: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderPrecisionAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub producer_kind: ProducerKind,
    /// A single analyzed bundle is never sufficient to close a hardware row.
    pub base_matrix_row_closed: bool,
    pub alpha_values_u5: [u8; 4],
    pub denominator_boundary_u6: u8,
    pub pixel_color_rgba32_be: u32,
    pub memory_color_rgba32_be: u32,
    pub fog_color_rgba32_be: u32,
    pub depth_control_u16_be: u16,
    pub stored_coverage_control_u3: u8,
    pub precision_geometry: BlenderPrecisionGeometry,
    pub feedback_geometry: BlenderFeedbackGeometry,
    pub modes: Vec<BlenderPrecisionModeAnalysis>,
    pub feedback_pairs: Vec<BlenderMemoryFeedbackObservation>,
    pub unexpected_output_count: u8,
    pub unexpected_depth_count: u8,
    pub unexpected_coverage_count: u8,
    pub total_cycle_divergence_count: u8,
    pub all_cycle_results_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorBundle {
    pub schema: String,
    pub suite_id: String,
    pub content_class: String,
    pub producer: Producer,
    pub cases: Vec<VectorCase>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedBundle {
    bundle: VectorBundle,
    canonical_sha256: String,
}

impl ValidatedBundle {
    pub fn bundle(&self) -> &VectorBundle {
        &self.bundle
    }

    pub fn canonical_sha256(&self) -> &str {
        &self.canonical_sha256
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError(String);

impl ValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValidationError {}

pub fn validate_json(bytes: &[u8]) -> Result<ValidatedBundle, ValidationError> {
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(ValidationError::new(format!(
            "bundle exceeds {MAX_BUNDLE_BYTES} bytes"
        )));
    }
    let bundle: VectorBundle = serde_json::from_slice(bytes)
        .map_err(|error| ValidationError::new(format!("malformed bundle: {error}")))?;
    validate_bundle(bundle)
}

pub fn validate_bundle(bundle: VectorBundle) -> Result<ValidatedBundle, ValidationError> {
    if bundle.schema != SCHEMA {
        return Err(ValidationError::new(format!(
            "unsupported schema {:?}; expected {SCHEMA:?}",
            bundle.schema
        )));
    }
    text("suite_id", &bundle.suite_id)?;
    if bundle.content_class != "synthetic_raw_dpc" {
        return Err(ValidationError::new(
            "content_class must be `synthetic_raw_dpc`; ROM/game-derived input is outside this schema",
        ));
    }
    validate_producer(&bundle.producer)?;
    if bundle.cases.is_empty() {
        return Err(ValidationError::new("bundle contains no cases"));
    }
    if bundle.cases.len() > MAX_CASES {
        return Err(ValidationError::new(format!(
            "bundle contains more than {MAX_CASES} cases"
        )));
    }
    let mut case_ids = BTreeSet::new();
    for case in &bundle.cases {
        text("case_id", &case.case_id)?;
        text("case description", &case.description)?;
        if !case_ids.insert(&case.case_id) {
            return Err(ValidationError::new(format!(
                "duplicate case_id {:?}",
                case.case_id
            )));
        }
        validate_case(case)
            .map_err(|error| ValidationError::new(format!("case {:?}: {error}", case.case_id)))?;
    }

    let canonical = serde_json::to_vec(&bundle)
        .map_err(|error| ValidationError::new(format!("canonicalize bundle: {error}")))?;
    let canonical_sha256 = hex(&Sha256::digest(canonical));
    Ok(ValidatedBundle {
        bundle,
        canonical_sha256,
    })
}

fn validate_producer(producer: &Producer) -> Result<(), ValidationError> {
    for (label, value) in [
        ("producer name", &producer.name),
        ("producer version", &producer.version),
        ("producer platform", &producer.platform),
        ("adapter", &producer.adapter),
        ("adapter version", &producer.adapter_version),
        ("capture method", &producer.capture_method),
        ("recorded_at_utc", &producer.recorded_at_utc),
    ] {
        text(label, value)?;
    }
    sha256("producer_binary_sha256", &producer.producer_binary_sha256)?;
    sha256("settings_sha256", &producer.settings_sha256)?;
    if !producer.recorded_at_utc.ends_with('Z') || !producer.recorded_at_utc.contains('T') {
        return Err(ValidationError::new(
            "recorded_at_utc must be an explicit UTC timestamp ending in `Z`",
        ));
    }
    Ok(())
}

fn validate_case(case: &VectorCase) -> Result<(), ValidationError> {
    let command = decode_blob("command_bytes", &case.command_bytes)?;
    if command.is_empty() || command.len() % 8 != 0 {
        return Err(ValidationError::new(
            "command_bytes must contain a nonempty whole number of 64-bit RDP command words",
        ));
    }
    if let Some(CaptureIntent::AlphaCompareDitherSweep {
        sweep_id,
        pass_rgba16_be,
        reject_rgba16_be,
        ..
    }) = &case.capture_intent
    {
        text("alpha-dither sweep_id", sweep_id)?;
        if pass_rgba16_be == reject_rgba16_be {
            return Err(ValidationError::new(
                "alpha-dither pass and reject RGBA16 markers must differ",
            ));
        }
    }
    if let Some(CaptureIntent::RgbDitherSweep {
        sweep_id,
        swept_channel,
        input_rgb8,
        channel_value,
        ..
    }) = &case.capture_intent
    {
        text("RGB-dither sweep_id", sweep_id)?;
        let selected = match swept_channel {
            RgbDitherChannel::Red => input_rgb8[0],
            RgbDitherChannel::Green => input_rgb8[1],
            RgbDitherChannel::Blue => input_rgb8[2],
        };
        if selected != *channel_value {
            return Err(ValidationError::new(
                "RGB-dither channel_value must equal the selected input_rgb8 component",
            ));
        }
    }
    if let Some(CaptureIntent::AlphaCoverageProductSweep {
        sweep_id,
        input_coverage,
        ..
    }) = &case.capture_intent
    {
        text("alpha-coverage sweep_id", sweep_id)?;
        if !(1..=8).contains(input_coverage) {
            return Err(ValidationError::new(
                "alpha-coverage input_coverage must be in 1..=8",
            ));
        }
    }
    if let Some(CaptureIntent::CoverageToAlphaSweep {
        sweep_id,
        input_coverage,
        pass_rgba16_be,
        reject_rgba16_be,
        ..
    }) = &case.capture_intent
    {
        text("coverage-to-alpha sweep_id", sweep_id)?;
        if !(1..=8).contains(input_coverage) {
            return Err(ValidationError::new(
                "coverage-to-alpha input_coverage must be in 1..=8",
            ));
        }
        if pass_rgba16_be == reject_rgba16_be {
            return Err(ValidationError::new(
                "coverage-to-alpha pass and reject RGBA16 markers must differ",
            ));
        }
    }
    if let Some(CaptureIntent::ZModeInterCoverageSweep {
        sweep_id,
        incoming_coverage,
        initial_stored_coverage,
        pass_rgba16_be,
        reject_rgba16_be,
        incoming_z_u18,
        memory_z_u18,
        ..
    }) = &case.capture_intent
    {
        text("ZMODE_INTER sweep_id", sweep_id)?;
        if !(1..=8).contains(incoming_coverage) {
            return Err(ValidationError::new(
                "ZMODE_INTER incoming_coverage must be in 1..=8",
            ));
        }
        if *initial_stored_coverage > 7 {
            return Err(ValidationError::new(
                "ZMODE_INTER initial_stored_coverage must be in 0..=7",
            ));
        }
        if pass_rgba16_be == reject_rgba16_be {
            return Err(ValidationError::new(
                "ZMODE_INTER pass and reject RGBA16 markers must differ",
            ));
        }
        if *incoming_z_u18 > 0x3_ffff || *memory_z_u18 > 0x3_ffff {
            return Err(ValidationError::new(
                "ZMODE_INTER incoming and memory Z must fit unsigned 18-bit values",
            ));
        }
    }
    if let Some(CaptureIntent::RepresentativeSampleSelectorSweep {
        sweep_id,
        coverage_mask_u8,
        controls,
        ..
    }) = &case.capture_intent
    {
        text("representative-sample sweep_id", sweep_id)?;
        if *coverage_mask_u8 == 0 {
            return Err(ValidationError::new(
                "representative-sample coverage_mask_u8 must be nonzero",
            ));
        }
        let shade = controls
            .markers
            .shade_rgba32_be
            .into_iter()
            .collect::<BTreeSet<_>>();
        let texture = controls
            .markers
            .texture_rgba32_be
            .into_iter()
            .collect::<BTreeSet<_>>();
        let depth = controls
            .markers
            .depth_u16_be
            .into_iter()
            .collect::<BTreeSet<_>>();
        if shade.len() != 8 || texture.len() != 8 || depth.len() != 8 {
            return Err(ValidationError::new(
                "representative-sample marker sets must uniquely identify all eight samples",
            ));
        }
        if !shade.is_disjoint(&texture)
            || shade.contains(&controls.markers.depth_observable_color_control_rgba32_be)
            || texture.contains(&controls.markers.depth_observable_color_control_rgba32_be)
            || depth.contains(&controls.markers.color_observable_depth_control_u16_be)
        {
            return Err(ValidationError::new(
                "representative-sample marker/control domains must be disjoint across observable labels",
            ));
        }
    }
    if let Some(CaptureIntent::NarrowEdgeCoverageCorrectionSweep {
        sweep_id,
        boundary_position,
        controls,
        edge_boundary_i64,
        edge_accumulator_i64,
        coverage_mask_u8,
        coverage_count_u4,
        ..
    }) = &case.capture_intent
    {
        text("narrow-edge-coverage sweep_id", sweep_id)?;
        if controls.edge_fractional_bits_u8 > 62 {
            return Err(ValidationError::new(
                "narrow-edge-coverage edge_fractional_bits_u8 must be in 0..=62",
            ));
        }
        if controls.selected_boundaries_i64.is_empty() {
            return Err(ValidationError::new(
                "narrow-edge-coverage selected_boundaries_i64 must not be empty",
            ));
        }
        if controls
            .selected_boundaries_i64
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(ValidationError::new(
                "narrow-edge-coverage selected_boundaries_i64 must be strictly increasing",
            ));
        }
        if !controls.selected_boundaries_i64.contains(edge_boundary_i64) {
            return Err(ValidationError::new(
                "narrow-edge-coverage edge_boundary_i64 must name a selected boundary",
            ));
        }
        let expected_accumulator = match boundary_position {
            NarrowEdgeBoundaryPosition::Below => edge_boundary_i64.checked_sub(1),
            NarrowEdgeBoundaryPosition::On => Some(*edge_boundary_i64),
            NarrowEdgeBoundaryPosition::Above => edge_boundary_i64.checked_add(1),
        }
        .ok_or_else(|| {
            ValidationError::new(
                "narrow-edge-coverage selected boundary cannot represent its one-LSB neighbors",
            )
        })?;
        if *edge_accumulator_i64 != expected_accumulator {
            return Err(ValidationError::new(format!(
                "narrow-edge-coverage {boundary_position:?} accumulator must be exactly {expected_accumulator}"
            )));
        }
        if *coverage_mask_u8 == 0 {
            return Err(ValidationError::new(
                "narrow-edge-coverage coverage_mask_u8 must be nonzero",
            ));
        }
        let mask_count = coverage_mask_u8.count_ones() as u8;
        if *coverage_count_u4 != mask_count {
            return Err(ValidationError::new(format!(
                "narrow-edge-coverage mask 0x{coverage_mask_u8:02x} has count {mask_count}, not declared count {coverage_count_u4}"
            )));
        }
        let shade = controls
            .markers
            .shade_rgba32_be
            .into_iter()
            .collect::<BTreeSet<_>>();
        let texture = controls
            .markers
            .texture_rgba32_be
            .into_iter()
            .collect::<BTreeSet<_>>();
        let depth = controls
            .markers
            .depth_u16_be
            .into_iter()
            .collect::<BTreeSet<_>>();
        if shade.len() != 8 || texture.len() != 8 || depth.len() != 8 {
            return Err(ValidationError::new(
                "narrow-edge-coverage marker sets must uniquely identify all eight samples",
            ));
        }
        if !shade.is_disjoint(&texture)
            || shade.contains(&controls.markers.depth_observable_color_control_rgba32_be)
            || texture.contains(&controls.markers.depth_observable_color_control_rgba32_be)
            || depth.contains(&controls.markers.color_observable_depth_control_u16_be)
        {
            return Err(ValidationError::new(
                "narrow-edge-coverage marker/control domains must be disjoint across observable labels",
            ));
        }
    }
    if let Some(CaptureIntent::TextureFilterTieSweep {
        sweep_id,
        boundary_position,
        texture_address,
        s_texel_i10,
        t_texel_i10,
        s_fraction_u5,
        t_fraction_u5,
        diagonal_boundary_u6,
        ..
    }) = &case.capture_intent
    {
        text("texture-filter-tie sweep_id", sweep_id)?;
        if !(-512..=511).contains(s_texel_i10) || !(-512..=511).contains(t_texel_i10) {
            return Err(ValidationError::new(
                "texture-filter-tie integer texel coordinates must fit signed 10-bit values",
            ));
        }
        if *s_fraction_u5 > 31 || *t_fraction_u5 > 31 {
            return Err(ValidationError::new(
                "texture-filter-tie fractions must fit unsigned 5-bit values",
            ));
        }
        if !(1..=61).contains(diagonal_boundary_u6) {
            return Err(ValidationError::new(
                "texture-filter-tie diagonal boundary must permit immediate below and above sums",
            ));
        }
        let expected_sum = match boundary_position {
            FilterTieBoundaryPosition::Below => diagonal_boundary_u6 - 1,
            FilterTieBoundaryPosition::On => *diagonal_boundary_u6,
            FilterTieBoundaryPosition::Above => diagonal_boundary_u6 + 1,
        };
        if s_fraction_u5 + t_fraction_u5 != expected_sum {
            return Err(ValidationError::new(format!(
                "texture-filter-tie {:?} fractions must sum to {expected_sum}",
                boundary_position
            )));
        }
        if !texture_address.is_multiple_of(8) || *texture_address >= RDRAM_END {
            return Err(ValidationError::new(
                "texture-filter-tie texture address must be eight-byte aligned in physical RDRAM",
            ));
        }
    }
    if let Some(CaptureIntent::ReciprocalS10_5BoundarySweep {
        sweep_id,
        boundary_position,
        boundary_s10_5_i16,
        perspective_numerator_i64,
        perspective_denominator_u64,
        stored_coverage_control_u3,
        ..
    }) = &case.capture_intent
    {
        text("reciprocal-S10.5 sweep_id", sweep_id)?;
        if *perspective_denominator_u64 == 0 {
            return Err(ValidationError::new(
                "reciprocal-S10.5 perspective denominator must be nonzero",
            ));
        }
        if *stored_coverage_control_u3 > 7 {
            return Err(ValidationError::new(
                "reciprocal-S10.5 stored coverage control must be in 0..=7",
            ));
        }
        if *boundary_s10_5_i16 == i16::MIN || *boundary_s10_5_i16 == i16::MAX {
            return Err(ValidationError::new(
                "reciprocal-S10.5 boundary must permit adjacent signed S10.5 outputs",
            ));
        }
        let boundary_numerator =
            i128::from(*boundary_s10_5_i16) * i128::from(*perspective_denominator_u64);
        let numerator = i128::from(*perspective_numerator_i64);
        let relation_is_exact = match boundary_position {
            ReciprocalBoundaryPosition::Below => numerator < boundary_numerator,
            ReciprocalBoundaryPosition::On => numerator == boundary_numerator,
            ReciprocalBoundaryPosition::Above => numerator > boundary_numerator,
        };
        if !relation_is_exact {
            return Err(ValidationError::new(format!(
                "reciprocal-S10.5 {boundary_position:?} numerator/denominator does not have the declared exact relation to the boundary"
            )));
        }
    }
    if let Some(CaptureIntent::AverageFilterOutputTieSweep {
        sweep_id,
        tie_position,
        texture_address,
        s_texel_i10,
        t_texel_i10,
        s_fraction_u5,
        t_fraction_u5,
        accumulator_numerator_i64,
        accumulator_denominator_u64,
        tie_numerator_i64,
        stored_coverage_control_u3,
        ..
    }) = &case.capture_intent
    {
        text("average-filter-tie sweep_id", sweep_id)?;
        if !(-512..=511).contains(s_texel_i10) || !(-512..=511).contains(t_texel_i10) {
            return Err(ValidationError::new(
                "average-filter-tie integer texel coordinates must fit signed 10-bit values",
            ));
        }
        if *s_fraction_u5 > 31 || *t_fraction_u5 > 31 {
            return Err(ValidationError::new(
                "average-filter-tie fractions must fit unsigned 5-bit values",
            ));
        }
        if *accumulator_denominator_u64 == 0 {
            return Err(ValidationError::new(
                "average-filter-tie accumulator denominator must be nonzero",
            ));
        }
        if *stored_coverage_control_u3 > 7 {
            return Err(ValidationError::new(
                "average-filter-tie stored coverage control must be in 0..=7",
            ));
        }
        let relation_is_exact = match tie_position {
            AverageFilterTiePosition::Below => accumulator_numerator_i64 < tie_numerator_i64,
            AverageFilterTiePosition::On => accumulator_numerator_i64 == tie_numerator_i64,
            AverageFilterTiePosition::Above => accumulator_numerator_i64 > tie_numerator_i64,
        };
        if !relation_is_exact {
            return Err(ValidationError::new(format!(
                "average-filter-tie {tie_position:?} accumulator does not have the declared exact relation to the tie"
            )));
        }
        if !texture_address.is_multiple_of(8) || *texture_address >= RDRAM_END {
            return Err(ValidationError::new(
                "average-filter-tie texture address must be eight-byte aligned in physical RDRAM",
            ));
        }
    }
    if let Some(CaptureIntent::TextureLodBoundarySweep {
        sweep_id,
        boundary_position,
        center_s_s10_5_i16,
        center_t_s10_5_i16,
        x_neighbor_s_s10_5_i16,
        x_neighbor_t_s10_5_i16,
        y_neighbor_s_s10_5_i16,
        y_neighbor_t_s10_5_i16,
        dsdx_s10_5_i32,
        dtdx_s10_5_i32,
        dsdy_s10_5_i32,
        dtdy_s10_5_i32,
        lod_metric_numerator_i64,
        lod_metric_denominator_u64,
        lod_boundary_numerator_i64,
        primitive_tile_u3,
        max_mip_level_u3,
        producer_expected_tile0_u3,
        producer_expected_tile1_u3,
        producer_expected_lod_fraction_s9_8_i16,
        stored_coverage_control_u3,
        ..
    }) = &case.capture_intent
    {
        text("texture-LOD sweep_id", sweep_id)?;
        let expected_derivatives = (
            i32::from(*x_neighbor_s_s10_5_i16) - i32::from(*center_s_s10_5_i16),
            i32::from(*x_neighbor_t_s10_5_i16) - i32::from(*center_t_s10_5_i16),
            i32::from(*y_neighbor_s_s10_5_i16) - i32::from(*center_s_s10_5_i16),
            i32::from(*y_neighbor_t_s10_5_i16) - i32::from(*center_t_s10_5_i16),
        );
        if expected_derivatives
            != (
                *dsdx_s10_5_i32,
                *dtdx_s10_5_i32,
                *dsdy_s10_5_i32,
                *dtdy_s10_5_i32,
            )
        {
            return Err(ValidationError::new(
                "texture-LOD declared derivatives must exactly equal neighbor minus center S10.5 coordinates",
            ));
        }
        if *lod_metric_denominator_u64 == 0 {
            return Err(ValidationError::new(
                "texture-LOD metric denominator must be nonzero",
            ));
        }
        if *primitive_tile_u3 > 7
            || *max_mip_level_u3 > 7
            || *producer_expected_tile0_u3 > 7
            || *producer_expected_tile1_u3 > 7
        {
            return Err(ValidationError::new(
                "texture-LOD tile and maximum-level controls must fit unsigned 3-bit values",
            ));
        }
        if !(-256..=255).contains(producer_expected_lod_fraction_s9_8_i16) {
            return Err(ValidationError::new(
                "texture-LOD expected fraction must fit signed S9.8 control range",
            ));
        }
        if *stored_coverage_control_u3 > 7 {
            return Err(ValidationError::new(
                "texture-LOD stored coverage control must be in 0..=7",
            ));
        }
        let relation_is_exact = match boundary_position {
            TextureLodBoundaryPosition::Below => {
                lod_metric_numerator_i64 < lod_boundary_numerator_i64
            }
            TextureLodBoundaryPosition::On => {
                lod_metric_numerator_i64 == lod_boundary_numerator_i64
            }
            TextureLodBoundaryPosition::Above => {
                lod_metric_numerator_i64 > lod_boundary_numerator_i64
            }
        };
        if !relation_is_exact {
            return Err(ValidationError::new(format!(
                "texture-LOD {boundary_position:?} metric does not have the declared exact relation to the boundary"
            )));
        }
    }
    if let Some(CaptureIntent::BlenderPrecisionBoundarySweep {
        sweep_id,
        isolated_alpha_u5,
        denominator_position,
        denominator_boundary_u6,
        producer_declared_denominator_u6,
        stored_coverage_control_u3,
        ..
    }) = &case.capture_intent
    {
        text("blender-precision sweep_id", sweep_id)?;
        if ![0, 1, 30, 31].contains(isolated_alpha_u5) {
            return Err(ValidationError::new(
                "blender-precision isolated alpha must be one of the exact 5-bit extrema/adjacent codes 0, 1, 30, or 31",
            ));
        }
        if !(1..=62).contains(denominator_boundary_u6) {
            return Err(ValidationError::new(
                "blender-precision denominator boundary must permit immediate unsigned six-bit neighbors",
            ));
        }
        let expected_denominator = match denominator_position {
            BlenderDenominatorPosition::Below => denominator_boundary_u6 - 1,
            BlenderDenominatorPosition::On => *denominator_boundary_u6,
            BlenderDenominatorPosition::Above => denominator_boundary_u6 + 1,
        };
        if *producer_declared_denominator_u6 != expected_denominator {
            return Err(ValidationError::new(format!(
                "blender-precision {denominator_position:?} denominator must be exactly {expected_denominator}"
            )));
        }
        if *producer_declared_denominator_u6 > 63 {
            return Err(ValidationError::new(
                "blender-precision denominator must fit an unsigned six-bit declaration",
            ));
        }
        if *stored_coverage_control_u3 > 7 {
            return Err(ValidationError::new(
                "blender-precision stored coverage control must be in 0..=7",
            ));
        }
    }
    if let Some(CaptureIntent::BlenderMemoryFeedbackPair {
        sweep_id,
        cycle_type,
        first_pixel_x,
        first_pixel_y,
        second_pixel_x,
        second_pixel_y,
        ordered_pair_command_sha256,
        cycle_one_handoff_color_rgba32_be,
        prior_memory_color_rgba32_be,
        cycle_one_handoff_coverage_u3,
        prior_memory_coverage_u3,
        ..
    }) = &case.capture_intent
    {
        text("blender-feedback sweep_id", sweep_id)?;
        if *cycle_type != ProbeCycleType::TwoCycle {
            return Err(ValidationError::new(
                "blender-feedback ordered pair must declare two_cycle",
            ));
        }
        sha256(
            "blender-feedback ordered_pair_command_sha256",
            ordered_pair_command_sha256,
        )?;
        if ordered_pair_command_sha256 != &case.command_bytes.sha256 {
            return Err(ValidationError::new(
                "blender-feedback ordered pair digest must equal the exact ordered command_bytes digest",
            ));
        }
        if *second_pixel_y != *first_pixel_y
            || first_pixel_x.checked_add(1) != Some(*second_pixel_x)
        {
            return Err(ValidationError::new(
                "blender-feedback pair must name two horizontally adjacent pixels in first/second order",
            ));
        }
        if cycle_one_handoff_color_rgba32_be == prior_memory_color_rgba32_be {
            return Err(ValidationError::new(
                "blender-feedback cycle-one and prior-memory color markers must differ",
            ));
        }
        if *cycle_one_handoff_coverage_u3 > 7 || *prior_memory_coverage_u3 > 7 {
            return Err(ValidationError::new(
                "blender-feedback coverage markers must be in 0..=7",
            ));
        }
        if cycle_one_handoff_coverage_u3 == prior_memory_coverage_u3 {
            return Err(ValidationError::new(
                "blender-feedback cycle-one and prior-memory coverage markers must differ",
            ));
        }
    }

    let mut registers = BTreeSet::new();
    for register in &case.setup.registers {
        if !registers.insert(register.name) {
            return Err(ValidationError::new(format!(
                "duplicate setup register {:?}",
                register.name
            )));
        }
    }
    for required in [
        RegisterName::DpcStart,
        RegisterName::DpcEnd,
        RegisterName::DpcStatus,
    ] {
        if !registers.contains(&required) {
            return Err(ValidationError::new(format!(
                "missing required setup register {required:?}"
            )));
        }
    }
    let start = register(case, RegisterName::DpcStart);
    let end = register(case, RegisterName::DpcEnd);
    if !start.is_multiple_of(8) || !end.is_multiple_of(8) || end <= start || end > RDRAM_END {
        return Err(ValidationError::new(
            "DPC START/END must be an aligned, increasing physical RDRAM range",
        ));
    }
    if u64::from(end - start) != case.command_bytes.byte_len {
        return Err(ValidationError::new(
            "DPC START/END range does not equal command_bytes.byte_len",
        ));
    }

    let mut region_ids = BTreeSet::new();
    let mut ranges = vec![(start, end, "command_bytes")];
    for region in &case.setup.initial_memory {
        text("memory region_id", &region.region_id)?;
        if !region_ids.insert(&region.region_id) {
            return Err(ValidationError::new(format!(
                "duplicate memory region_id {:?}",
                region.region_id
            )));
        }
        decode_blob("initial_memory contents", &region.contents)?;
        let range = physical_range(region.address, region.contents.byte_len, "initial memory")?;
        for &(other_start, other_end, other_name) in &ranges {
            if range.0 < other_end && other_start < range.1 {
                return Err(ValidationError::new(format!(
                    "initial memory region {:?} overlaps {other_name}",
                    region.region_id
                )));
            }
        }
        ranges.push((range.0, range.1, "another initial-memory region"));
    }

    validate_framebuffer(&case.expected.framebuffer)?;
    validate_depth(&case.expected.depth)?;
    validate_coverage(&case.expected.coverage)?;
    let framebuffer = &case.expected.framebuffer;
    let coverage = &case.expected.coverage;
    if coverage.color_image_address != framebuffer.address
        || coverage.width != framebuffer.width
        || coverage.height != framebuffer.height
    {
        return Err(ValidationError::new(
            "coverage geometry/address must identify the expected framebuffer exactly",
        ));
    }
    Ok(())
}

/// Validate and preserve a complete 4x4 RGB-dither transfer sweep for one
/// selector and one input channel. Metadata remains a producer assertion;
/// this function neither decodes the command stream nor promotes fixtures to
/// hardware evidence.
pub fn analyze_rgb_dither_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<RgbDitherSweepAnalysis, ValidationError> {
    text("RGB-dither sweep_id", sweep_id)?;
    type Outcomes = [Option<[u8; 16]>; 256];
    let mut points = BTreeMap::<ProbeCycleType, Outcomes>::new();
    let mut controls: Option<RgbDitherControls> = None;
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::RgbDitherSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            mode,
            swept_channel,
            input_rgb8,
            channel_value,
            origin_x,
            origin_y,
            replay_from_reset,
            sample_index,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: RGB-dither sweep must replay from reset before every point",
                case.case_id
            )));
        }
        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 4
            || framebuffer.height != 4
            || framebuffer.row_stride_bytes != 8
            || framebuffer.encoding != FramebufferEncoding::Rgba16BigEndian
            || depth.width != 4
            || depth.height != 4
            || depth.row_stride_bytes != 8
            || coverage.width != 4
            || coverage.height != 4
        {
            return Err(ValidationError::new(format!(
                "case {:?}: RGB-dither sweep requires exact 4x4 RGBA16 framebuffer, depth, and coverage planes",
                case.case_id
            )));
        }
        let mut fixed_rgb8 = *input_rgb8;
        match swept_channel {
            RgbDitherChannel::Red => fixed_rgb8[0] = 0,
            RgbDitherChannel::Green => fixed_rgb8[1] = 0,
            RgbDitherChannel::Blue => fixed_rgb8[2] = 0,
        }
        let point_controls = RgbDitherControls {
            mode: *mode,
            swept_channel: *swept_channel,
            fixed_rgb8,
            origin_x: *origin_x,
            origin_y: *origin_y,
            sample_index: *sample_index,
            framebuffer_address: framebuffer.address,
            depth_address: depth.address,
            coverage_address: coverage.color_image_address,
            coverage_encoding: coverage.encoding,
        };
        if let Some(expected) = controls {
            if expected != point_controls {
                return Err(ValidationError::new(format!(
                    "case {:?}: RGB-dither selector, fixed input, sample, origin, or output geometry differs within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            controls = Some(point_controls);
        }

        let bytes = decode_blob("RGB-dither framebuffer", &framebuffer.contents)?;
        let shift = match swept_channel {
            RgbDitherChannel::Red => 11,
            RgbDitherChannel::Green => 6,
            RgbDitherChannel::Blue => 1,
        };
        let mut observed = [0u8; 16];
        for (pixel, pair) in bytes.chunks_exact(2).enumerate() {
            let rgba16 = u16::from_be_bytes([pair[0], pair[1]]);
            observed[pixel] = ((rgba16 >> shift) & 0x1f) as u8;
        }
        let outcomes = points.entry(*cycle_type).or_insert([None; 256]);
        let slot = &mut outcomes[usize::from(*channel_value)];
        if slot.replace(observed).is_some() {
            return Err(ValidationError::new(format!(
                "duplicate RGB-dither point for {cycle_type:?} channel value {channel_value} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no RGB-dither capture intent for sweep {sweep_id:?}"
        )));
    }
    let controls = controls.expect("a matching case establishes controls");
    let mut cycles = Vec::with_capacity(2);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        let outcomes = points.get(&cycle_type).ok_or_else(|| {
            ValidationError::new(format!(
                "RGB-dither sweep {sweep_id:?} is missing every {cycle_type:?} point"
            ))
        })?;
        if let Some(value) = outcomes.iter().position(Option::is_none) {
            return Err(ValidationError::new(format!(
                "RGB-dither sweep {sweep_id:?} is missing {cycle_type:?} channel value {value}"
            )));
        }
        let observed = outcomes
            .iter()
            .map(|point| point.expect("completeness checked above"))
            .collect::<Vec<_>>();
        let mut channel_u5_hex = String::with_capacity(256 * 16 * 2);
        let mut distinct = BTreeSet::new();
        for tile in &observed {
            for &code in tile {
                use std::fmt::Write as _;
                write!(&mut channel_u5_hex, "{code:02x}")
                    .expect("writing to a String is infallible");
                distinct.insert(code);
            }
        }
        let monotonic_per_pixel = (0..16).all(|pixel| {
            observed
                .windows(2)
                .all(|pair| pair[0][pixel] <= pair[1][pixel])
        });
        cycles.push(RgbDitherCycleAnalysis {
            cycle_type,
            channel_u5_hex,
            distinct_channel_codes: u8::try_from(distinct.len())
                .expect("a five-bit channel has at most 32 codes"),
            monotonic_per_pixel,
        });
    }
    let cycle_results_match = cycles[0].channel_u5_hex == cycles[1].channel_u5_hex;
    Ok(RgbDitherSweepAnalysis {
        schema: "fn64.rdp-rgb-dither-analysis.v1",
        bundle_sha256: bundle.canonical_sha256.clone(),
        sweep_id: sweep_id.to_owned(),
        mode: controls.mode,
        swept_channel: controls.swept_channel,
        fixed_rgb8: controls.fixed_rgb8,
        origin_x: controls.origin_x,
        origin_y: controls.origin_y,
        sample_index: controls.sample_index,
        framebuffer_address: controls.framebuffer_address,
        cycles,
        cycle_results_match,
    })
}

/// Validate and summarize a complete, controlled `G_AC_DITHER` threshold
/// sweep. This establishes capture completeness and observed transitions; it
/// does not establish that metadata matches opaque command bytes or identify
/// the silicon random generator.
pub fn analyze_alpha_dither_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<AlphaDitherSweepAnalysis, ValidationError> {
    text("alpha-dither sweep_id", sweep_id)?;
    let mut points = BTreeMap::<ProbeCycleType, [Option<bool>; 256]>::new();
    let mut controls = None;
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::AlphaCompareDitherSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            combined_alpha,
            replay_from_reset,
            sample_index,
            pass_rgba16_be,
            reject_rgba16_be,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: alpha-dither sweep must replay from reset before every point",
                case.case_id
            )));
        }
        let point_controls = (*sample_index, *pass_rgba16_be, *reject_rgba16_be);
        if let Some(expected) = controls {
            if expected != point_controls {
                return Err(ValidationError::new(format!(
                    "case {:?}: alpha-dither sample index or pass/reject markers differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            controls = Some(point_controls);
        }

        let framebuffer = &case.expected.framebuffer;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 2
            || framebuffer.encoding != FramebufferEncoding::Rgba16BigEndian
        {
            return Err(ValidationError::new(format!(
                "case {:?}: alpha-dither sweep requires an exact 1x1 RGBA16 framebuffer",
                case.case_id
            )));
        }
        let bytes = decode_blob("alpha-dither framebuffer", &framebuffer.contents)?;
        let observed = u16::from_be_bytes([bytes[0], bytes[1]]);
        let passed = if observed == *pass_rgba16_be {
            true
        } else if observed == *reject_rgba16_be {
            false
        } else {
            return Err(ValidationError::new(format!(
                "case {:?}: alpha-dither probe observed RGBA16 0x{observed:04x}, neither pass marker 0x{pass_rgba16_be:04x} nor reject marker 0x{reject_rgba16_be:04x}",
                case.case_id
            )));
        };
        let outcomes = points.entry(*cycle_type).or_insert([None; 256]);
        let slot = &mut outcomes[usize::from(*combined_alpha)];
        if slot.replace(passed).is_some() {
            return Err(ValidationError::new(format!(
                "duplicate alpha-dither point for {cycle_type:?} alpha {combined_alpha} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no alpha-dither capture intent for sweep {sweep_id:?}"
        )));
    }
    let (sample_index, pass_rgba16_be, reject_rgba16_be) =
        controls.expect("a matching case establishes controls");
    let mut transitions = Vec::with_capacity(2);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        let outcomes = points.get(&cycle_type).ok_or_else(|| {
            ValidationError::new(format!(
                "alpha-dither sweep {sweep_id:?} is missing every {cycle_type:?} point"
            ))
        })?;
        let missing = outcomes.iter().position(Option::is_none);
        if let Some(alpha) = missing {
            return Err(ValidationError::new(format!(
                "alpha-dither sweep {sweep_id:?} is missing {cycle_type:?} alpha {alpha}"
            )));
        }
        let observed = outcomes
            .iter()
            .map(|outcome| outcome.expect("completeness checked above"))
            .collect::<Vec<_>>();
        let first_passing = observed.iter().position(|&passed| passed);
        let first_reject_after_pass = first_passing.and_then(|first| {
            observed[first..]
                .iter()
                .position(|&passed| !passed)
                .map(|offset| first + offset)
        });
        let mut pass_bitmap = [0u8; 32];
        for (alpha, &passed) in observed.iter().enumerate() {
            if passed {
                pass_bitmap[alpha / 8] |= 1 << (alpha % 8);
            }
        }
        transitions.push(AlphaDitherCycleTransition {
            cycle_type,
            first_passing_alpha: first_passing.map(|alpha| {
                u8::try_from(alpha).expect("alpha sweep indices are limited to 0..=255")
            }),
            first_reject_after_pass: first_reject_after_pass.map(|alpha| {
                u8::try_from(alpha).expect("alpha sweep indices are limited to 0..=255")
            }),
            pass_count: u16::try_from(observed.iter().filter(|&&passed| passed).count())
                .expect("a 256-point sweep fits u16"),
            transition_count: u16::try_from(
                observed
                    .windows(2)
                    .filter(|pair| pair[0] != pair[1])
                    .count(),
            )
            .expect("a 256-point sweep has at most 255 transitions"),
            monotonic_reject_then_pass: first_reject_after_pass.is_none(),
            pass_bitmap_hex: hex(&pass_bitmap),
        });
    }
    let cycle_transitions_match = transitions[0].pass_bitmap_hex == transitions[1].pass_bitmap_hex;
    Ok(AlphaDitherSweepAnalysis {
        schema: "fn64.rdp-alpha-dither-analysis.v1",
        bundle_sha256: bundle.canonical_sha256.clone(),
        sweep_id: sweep_id.to_owned(),
        sample_index,
        pass_rgba16_be,
        reject_rgba16_be,
        transitions,
        cycle_transitions_match,
    })
}

/// Validate and summarize the complete public `CVG_X_ALPHA` input domain.
/// This proves capture completeness and preserves the observed transfer
/// curves; it does not establish that the opaque commands match their intent.
pub fn analyze_alpha_coverage_product_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<AlphaCoverageProductAnalysis, ValidationError> {
    text("alpha-coverage sweep_id", sweep_id)?;
    let mut points = BTreeMap::<(ProbeCycleType, u8), [Option<u8>; 256]>::new();
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::AlphaCoverageProductSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            input_coverage,
            combined_alpha,
            replay_from_reset,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: alpha-coverage sweep must replay from reset before every point",
                case.case_id
            )));
        }
        let coverage = &case.expected.coverage;
        if coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::CoverageCountU4
        {
            return Err(ValidationError::new(format!(
                "case {:?}: alpha-coverage sweep requires an exact 1x1 coverage_count_u4 plane",
                case.case_id
            )));
        }
        let observed = decode_blob("alpha-coverage output", &coverage.contents)?[0];
        let outcomes = points
            .entry((*cycle_type, *input_coverage))
            .or_insert([None; 256]);
        let slot = &mut outcomes[usize::from(*combined_alpha)];
        if slot.replace(observed).is_some() {
            return Err(ValidationError::new(format!(
                "duplicate alpha-coverage point for {cycle_type:?} input coverage {input_coverage} alpha {combined_alpha} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no alpha-coverage capture intent for sweep {sweep_id:?}"
        )));
    }
    let mut curves = Vec::with_capacity(16);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        for input_coverage in 1..=8 {
            let outcomes = points.get(&(cycle_type, input_coverage)).ok_or_else(|| {
                ValidationError::new(format!(
                    "alpha-coverage sweep {sweep_id:?} is missing every {cycle_type:?} input coverage {input_coverage} point"
                ))
            })?;
            if let Some(alpha) = outcomes.iter().position(Option::is_none) {
                return Err(ValidationError::new(format!(
                    "alpha-coverage sweep {sweep_id:?} is missing {cycle_type:?} input coverage {input_coverage} alpha {alpha}"
                )));
            }
            let observed = outcomes
                .iter()
                .map(|value| value.expect("completeness checked above"))
                .collect::<Vec<_>>();
            let first_nonzero_alpha = observed[1..]
                .iter()
                .position(|&value| value != 0)
                .map(|offset| offset + 1);
            let first_full_alpha = observed[1..]
                .iter()
                .position(|&value| value == input_coverage)
                .map(|offset| offset + 1);
            curves.push(AlphaCoverageProductCurve {
                cycle_type,
                input_coverage,
                alpha_zero_output_coverage: observed[0],
                first_nonzero_alpha: first_nonzero_alpha.map(|alpha| {
                    u8::try_from(alpha).expect("alpha sweep indices are limited to 0..=255")
                }),
                first_full_alpha: first_full_alpha.map(|alpha| {
                    u8::try_from(alpha).expect("alpha sweep indices are limited to 0..=255")
                }),
                transition_count: u16::try_from(
                    observed
                        .windows(2)
                        .filter(|pair| pair[0] != pair[1])
                        .count(),
                )
                .expect("a 256-point sweep has at most 255 transitions"),
                monotonic_nondecreasing_from_alpha_one: observed[1..]
                    .windows(2)
                    .all(|pair| pair[0] <= pair[1]),
                coverage_u4_hex: observed
                    .iter()
                    .map(|value| {
                        char::from_digit(u32::from(*value), 16).expect("coverage is 0..=8")
                    })
                    .collect(),
            });
        }
    }
    let cycle_curves_match = curves[..8]
        .iter()
        .zip(&curves[8..])
        .all(|(one, two)| one.coverage_u4_hex == two.coverage_u4_hex);
    Ok(AlphaCoverageProductAnalysis {
        schema: "fn64.rdp-alpha-coverage-product-analysis.v1",
        bundle_sha256: bundle.canonical_sha256.clone(),
        sweep_id: sweep_id.to_owned(),
        curves,
        cycle_curves_match,
    })
}

/// Validate and summarize the complete public coverage-to-alpha input domain.
/// Exact pass bitmaps preserve threshold ties and unexpected non-monotonic or
/// cycle-dependent observations instead of fitting them to a host formula.
pub fn analyze_coverage_to_alpha_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<CoverageToAlphaAnalysis, ValidationError> {
    text("coverage-to-alpha sweep_id", sweep_id)?;
    let mut points = BTreeMap::<(ProbeCycleType, u8), [Option<bool>; 256]>::new();
    let mut markers = None;
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::CoverageToAlphaSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            input_coverage,
            threshold_alpha,
            replay_from_reset,
            pass_rgba16_be,
            reject_rgba16_be,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: coverage-to-alpha sweep must replay from reset before every point",
                case.case_id
            )));
        }
        let point_markers = (*pass_rgba16_be, *reject_rgba16_be);
        if let Some(expected) = markers {
            if expected != point_markers {
                return Err(ValidationError::new(format!(
                    "case {:?}: coverage-to-alpha pass/reject markers differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            markers = Some(point_markers);
        }
        let framebuffer = &case.expected.framebuffer;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 2
            || framebuffer.encoding != FramebufferEncoding::Rgba16BigEndian
        {
            return Err(ValidationError::new(format!(
                "case {:?}: coverage-to-alpha sweep requires an exact 1x1 RGBA16 framebuffer",
                case.case_id
            )));
        }
        let bytes = decode_blob("coverage-to-alpha framebuffer", &framebuffer.contents)?;
        let observed = u16::from_be_bytes([bytes[0], bytes[1]]);
        let passed = if observed == *pass_rgba16_be {
            true
        } else if observed == *reject_rgba16_be {
            false
        } else {
            return Err(ValidationError::new(format!(
                "case {:?}: coverage-to-alpha probe observed RGBA16 0x{observed:04x}, neither pass marker 0x{pass_rgba16_be:04x} nor reject marker 0x{reject_rgba16_be:04x}",
                case.case_id
            )));
        };
        let outcomes = points
            .entry((*cycle_type, *input_coverage))
            .or_insert([None; 256]);
        let slot = &mut outcomes[usize::from(*threshold_alpha)];
        if slot.replace(passed).is_some() {
            return Err(ValidationError::new(format!(
                "duplicate coverage-to-alpha point for {cycle_type:?} input coverage {input_coverage} threshold {threshold_alpha} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no coverage-to-alpha capture intent for sweep {sweep_id:?}"
        )));
    }
    let (pass_rgba16_be, reject_rgba16_be) = markers.expect("a matching case establishes markers");
    let mut curves = Vec::with_capacity(16);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        for input_coverage in 1..=8 {
            let outcomes = points.get(&(cycle_type, input_coverage)).ok_or_else(|| {
                ValidationError::new(format!(
                    "coverage-to-alpha sweep {sweep_id:?} is missing every {cycle_type:?} input coverage {input_coverage} point"
                ))
            })?;
            if let Some(threshold) = outcomes.iter().position(Option::is_none) {
                return Err(ValidationError::new(format!(
                    "coverage-to-alpha sweep {sweep_id:?} is missing {cycle_type:?} input coverage {input_coverage} threshold {threshold}"
                )));
            }
            let observed = outcomes
                .iter()
                .map(|value| value.expect("completeness checked above"))
                .collect::<Vec<_>>();
            let greatest_passing_threshold = observed.iter().rposition(|&passed| passed);
            let first_reject = observed.iter().position(|&passed| !passed);
            let first_pass_after_reject = first_reject.and_then(|first| {
                observed[first..]
                    .iter()
                    .position(|&passed| passed)
                    .map(|offset| first + offset)
            });
            let mut pass_bitmap = [0u8; 32];
            for (threshold, &passed) in observed.iter().enumerate() {
                if passed {
                    pass_bitmap[threshold / 8] |= 1 << (threshold % 8);
                }
            }
            curves.push(CoverageToAlphaCurve {
                cycle_type,
                input_coverage,
                greatest_passing_threshold: greatest_passing_threshold.map(|threshold| {
                    u8::try_from(threshold)
                        .expect("alpha threshold sweep indices are limited to 0..=255")
                }),
                first_pass_after_reject: first_pass_after_reject.map(|threshold| {
                    u8::try_from(threshold)
                        .expect("alpha threshold sweep indices are limited to 0..=255")
                }),
                pass_count: u16::try_from(observed.iter().filter(|&&passed| passed).count())
                    .expect("a 256-point sweep fits u16"),
                transition_count: u16::try_from(
                    observed
                        .windows(2)
                        .filter(|pair| pair[0] != pair[1])
                        .count(),
                )
                .expect("a 256-point sweep has at most 255 transitions"),
                monotonic_pass_then_reject: first_pass_after_reject.is_none(),
                pass_bitmap_hex: hex(&pass_bitmap),
            });
        }
    }
    let cycle_curves_match = curves[..8]
        .iter()
        .zip(&curves[8..])
        .all(|(one, two)| one.pass_bitmap_hex == two.pass_bitmap_hex);
    Ok(CoverageToAlphaAnalysis {
        schema: "fn64.rdp-coverage-to-alpha-analysis.v1",
        bundle_sha256: bundle.canonical_sha256.clone(),
        sweep_id: sweep_id.to_owned(),
        pass_rgba16_be,
        reject_rgba16_be,
        curves,
        cycle_curves_match,
    })
}

/// Validate and summarize the complete bounded `ZMODE_INTER` admission and
/// stored-coverage matrix. The relation and numeric controls are producer
/// assertions; the analyzer preserves observations without deriving silicon
/// arithmetic from them or from opaque command bytes.
pub fn analyze_zmode_inter_coverage_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<ZModeInterAnalysis, ValidationError> {
    text("ZMODE_INTER sweep_id", sweep_id)?;
    type Outcomes = [[Option<(bool, u8)>; 8]; 8];
    let mut points = BTreeMap::<(ProbeCycleType, ZModeInterRelation), Outcomes>::new();
    let mut controls = BTreeMap::<ZModeInterRelation, ZModeInterControls>::new();
    let mut markers = None;
    let mut geometry = None;
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::ZModeInterCoverageSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            relation,
            incoming_coverage,
            initial_stored_coverage,
            replay_from_reset,
            pass_rgba16_be,
            reject_rgba16_be,
            incoming_z_u18,
            memory_z_u18,
            incoming_delta_z_u16,
            memory_delta_z_u16,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: ZMODE_INTER sweep must replay from reset before every point",
                case.case_id
            )));
        }

        let point_markers = (*pass_rgba16_be, *reject_rgba16_be);
        if let Some(expected) = markers {
            if expected != point_markers {
                return Err(ValidationError::new(format!(
                    "case {:?}: ZMODE_INTER pass/reject markers differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            markers = Some(point_markers);
        }

        let point_controls = ZModeInterControls {
            incoming_z_u18: *incoming_z_u18,
            memory_z_u18: *memory_z_u18,
            incoming_delta_z_u16: *incoming_delta_z_u16,
            memory_delta_z_u16: *memory_delta_z_u16,
        };
        if let Some(expected) = controls.get(relation) {
            if *expected != point_controls {
                return Err(ValidationError::new(format!(
                    "case {:?}: ZMODE_INTER numeric controls differ within relation {relation:?}",
                    case.case_id
                )));
            }
        } else {
            controls.insert(*relation, point_controls);
        }

        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 2
            || framebuffer.encoding != FramebufferEncoding::Rgba16BigEndian
            || depth.width != 1
            || depth.height != 1
            || depth.row_stride_bytes != 2
            || coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::StoredCoverageU3
        {
            return Err(ValidationError::new(format!(
                "case {:?}: ZMODE_INTER sweep requires exact 1x1 RGBA16, depth, and stored_coverage_u3 planes",
                case.case_id
            )));
        }
        let point_geometry = ZModeInterGeometry {
            framebuffer_address: framebuffer.address,
            depth_address: depth.address,
            color_image_address: coverage.color_image_address,
        };
        if let Some(expected) = geometry {
            if expected != point_geometry {
                return Err(ValidationError::new(format!(
                    "case {:?}: ZMODE_INTER output addresses differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            geometry = Some(point_geometry);
        }

        let framebuffer_bytes = decode_blob("ZMODE_INTER framebuffer", &framebuffer.contents)?;
        let observed_marker = u16::from_be_bytes([framebuffer_bytes[0], framebuffer_bytes[1]]);
        let admitted = if observed_marker == *pass_rgba16_be {
            true
        } else if observed_marker == *reject_rgba16_be {
            false
        } else {
            return Err(ValidationError::new(format!(
                "case {:?}: ZMODE_INTER probe observed RGBA16 0x{observed_marker:04x}, neither pass marker 0x{pass_rgba16_be:04x} nor reject marker 0x{reject_rgba16_be:04x}",
                case.case_id
            )));
        };
        let stored_coverage = decode_blob("ZMODE_INTER stored coverage", &coverage.contents)?[0];
        let outcomes = points
            .entry((*cycle_type, *relation))
            .or_insert([[None; 8]; 8]);
        let incoming_index = usize::from(*incoming_coverage - 1);
        let initial_index = usize::from(*initial_stored_coverage);
        if outcomes[incoming_index][initial_index]
            .replace((admitted, stored_coverage))
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate ZMODE_INTER point for {cycle_type:?} {relation:?} incoming coverage {incoming_coverage} initial stored coverage {initial_stored_coverage} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no ZMODE_INTER capture intent for sweep {sweep_id:?}"
        )));
    }

    let relation_order = [
        ZModeInterRelation::InFrontControl,
        ZModeInterRelation::Interpenetrating,
        ZModeInterRelation::BehindControl,
    ];
    for (index, relation) in relation_order.iter().enumerate() {
        let relation_controls = controls.get(relation).ok_or_else(|| {
            ValidationError::new(format!(
                "ZMODE_INTER sweep {sweep_id:?} is missing every {relation:?} point"
            ))
        })?;
        for other in &relation_order[..index] {
            if controls.get(other) == Some(relation_controls) {
                return Err(ValidationError::new(format!(
                    "ZMODE_INTER relation labels {other:?} and {relation:?} reuse the same numeric controls"
                )));
            }
        }
    }

    let mut relations = Vec::with_capacity(6);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        for relation in relation_order {
            let outcomes = points.get(&(cycle_type, relation)).ok_or_else(|| {
                ValidationError::new(format!(
                    "ZMODE_INTER sweep {sweep_id:?} is missing every {cycle_type:?} {relation:?} point"
                ))
            })?;
            let mut admission_bitmap = [0u8; 8];
            let mut stored_coverage_u3_hex = String::with_capacity(64);
            let mut admitted_count = 0u8;
            let mut changed_from_initial_count = 0u8;
            let mut rejected_coverage_changed_count = 0u8;
            for (incoming_index, incoming_row) in outcomes.iter().enumerate() {
                for (initial_index, point) in incoming_row.iter().enumerate() {
                    let point = point.ok_or_else(|| {
                        ValidationError::new(format!(
                            "ZMODE_INTER sweep {sweep_id:?} is missing {cycle_type:?} {relation:?} incoming coverage {} initial stored coverage {initial_index}",
                            incoming_index + 1
                        ))
                    })?;
                    let point_index = incoming_index * 8 + initial_index;
                    if point.0 {
                        admission_bitmap[point_index / 8] |= 1 << (point_index % 8);
                        admitted_count += 1;
                    }
                    stored_coverage_u3_hex.push(
                        char::from_digit(u32::from(point.1), 16)
                            .expect("stored coverage is validated as 0..=7"),
                    );
                    if usize::from(point.1) != initial_index {
                        changed_from_initial_count += 1;
                        if !point.0 {
                            rejected_coverage_changed_count += 1;
                        }
                    }
                }
            }
            relations.push(ZModeInterRelationAnalysis {
                cycle_type,
                relation,
                controls: *controls
                    .get(&relation)
                    .expect("all relation controls checked above"),
                admitted_count,
                admission_bitmap_hex: hex(&admission_bitmap),
                stored_coverage_u3_hex,
                changed_from_initial_count,
                rejected_coverage_changed_count,
            });
        }
    }
    let cycle_results_match = relations[..3]
        .iter()
        .zip(&relations[3..])
        .all(|(one, two)| {
            one.admission_bitmap_hex == two.admission_bitmap_hex
                && one.stored_coverage_u3_hex == two.stored_coverage_u3_hex
        });
    let (pass_rgba16_be, reject_rgba16_be) = markers.expect("a matching case sets markers");
    let geometry = geometry.expect("a matching case sets geometry");

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        pass_rgba16_be: u16,
        reject_rgba16_be: u16,
        geometry: ZModeInterGeometry,
        relations: &'a [ZModeInterRelationAnalysis],
        cycle_results_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-zmode-inter-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        pass_rgba16_be,
        reject_rgba16_be,
        geometry,
        relations: &relations,
        cycle_results_match,
    };
    let canonical = serde_json::to_vec(&hash_input)
        .map_err(|error| ValidationError::new(format!("hash ZMODE_INTER analysis: {error}")))?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-zmode-inter-analysis.v1\0");
    hasher.update(canonical);
    let analysis_sha256 = hex(&hasher.finalize());

    Ok(ZModeInterAnalysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256,
        sweep_id: sweep_id.to_owned(),
        pass_rgba16_be,
        reject_rgba16_be,
        geometry,
        relations,
        cycle_results_match,
    })
}

/// Validate and summarize all nonzero eight-sample masks for independently
/// marker-encoded shade, texture, and depth observables in both cycle modes.
/// Exact tables retain disagreements and even uncovered selections rather
/// than fitting observations to the host renderer's bounded selector policy.
pub fn analyze_representative_sample_selector_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<RepresentativeSampleSelectorAnalysis, ValidationError> {
    text("representative-sample sweep_id", sweep_id)?;
    let mut points =
        BTreeMap::<(ProbeCycleType, RepresentativeSampleObservable), [Option<u8>; 256]>::new();
    let mut controls = None;
    let mut geometry = None;
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::RepresentativeSampleSelectorSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            observable,
            coverage_mask_u8,
            replay_from_reset,
            controls: point_controls,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: representative-sample sweep must replay from reset before every point",
                case.case_id
            )));
        }
        if let Some(expected) = controls {
            if expected != *point_controls {
                return Err(ValidationError::new(format!(
                    "case {:?}: representative-sample fixed controls differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            controls = Some(*point_controls);
        }

        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 4
            || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
            || depth.width != 1
            || depth.height != 1
            || depth.row_stride_bytes != 2
            || coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::CoverageCountU4
        {
            return Err(ValidationError::new(format!(
                "case {:?}: representative-sample sweep requires exact 1x1 RGBA32, depth, and coverage_count_u4 planes",
                case.case_id
            )));
        }
        let point_geometry = RepresentativeSampleGeometry {
            framebuffer_address: framebuffer.address,
            depth_address: depth.address,
            color_image_address: coverage.color_image_address,
        };
        if let Some(expected) = geometry {
            if expected != point_geometry {
                return Err(ValidationError::new(format!(
                    "case {:?}: representative-sample output addresses differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            geometry = Some(point_geometry);
        }

        let coverage_count = decode_blob("representative-sample coverage", &coverage.contents)?[0];
        let expected_coverage = coverage_mask_u8.count_ones() as u8;
        if coverage_count != expected_coverage {
            return Err(ValidationError::new(format!(
                "case {:?}: representative-sample mask 0x{coverage_mask_u8:02x} has coverage count {expected_coverage}, observed {coverage_count}",
                case.case_id
            )));
        }
        let framebuffer_bytes =
            decode_blob("representative-sample framebuffer", &framebuffer.contents)?;
        let framebuffer_word = u32::from_be_bytes(
            framebuffer_bytes
                .as_slice()
                .try_into()
                .expect("exact RGBA32 geometry has four bytes"),
        );
        let depth_bytes = decode_blob("representative-sample depth", &depth.contents)?;
        let depth_word = u16::from_be_bytes(
            depth_bytes
                .as_slice()
                .try_into()
                .expect("exact depth geometry has two bytes"),
        );
        let markers = point_controls.markers;
        let selected_sample = match observable {
            RepresentativeSampleObservable::Shade => {
                if depth_word != markers.color_observable_depth_control_u16_be {
                    return Err(ValidationError::new(format!(
                        "case {:?}: shade selector changed the fixed depth control",
                        case.case_id
                    )));
                }
                markers
                    .shade_rgba32_be
                    .iter()
                    .position(|&marker| marker == framebuffer_word)
            }
            RepresentativeSampleObservable::Texture => {
                if depth_word != markers.color_observable_depth_control_u16_be {
                    return Err(ValidationError::new(format!(
                        "case {:?}: texture selector changed the fixed depth control",
                        case.case_id
                    )));
                }
                markers
                    .texture_rgba32_be
                    .iter()
                    .position(|&marker| marker == framebuffer_word)
            }
            RepresentativeSampleObservable::Depth => {
                if framebuffer_word != markers.depth_observable_color_control_rgba32_be {
                    return Err(ValidationError::new(format!(
                        "case {:?}: depth selector changed the fixed color control",
                        case.case_id
                    )));
                }
                markers
                    .depth_u16_be
                    .iter()
                    .position(|&marker| marker == depth_word)
            }
        }
        .ok_or_else(|| {
            ValidationError::new(format!(
                "case {:?}: representative-sample output does not identify one {observable:?} sample; cross-label markers are not admitted",
                case.case_id
            ))
        })?;
        let selected_sample =
            u8::try_from(selected_sample).expect("marker sets contain exactly eight samples");
        let outcomes = points
            .entry((*cycle_type, *observable))
            .or_insert([None; 256]);
        if outcomes[usize::from(*coverage_mask_u8)]
            .replace(selected_sample)
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate representative-sample point for {cycle_type:?} {observable:?} mask 0x{coverage_mask_u8:02x} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no representative-sample capture intent for sweep {sweep_id:?}"
        )));
    }
    let observable_order = [
        RepresentativeSampleObservable::Shade,
        RepresentativeSampleObservable::Texture,
        RepresentativeSampleObservable::Depth,
    ];
    let mut tables = Vec::with_capacity(6);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        for observable in observable_order {
            let outcomes = points.get(&(cycle_type, observable)).ok_or_else(|| {
                ValidationError::new(format!(
                    "representative-sample sweep {sweep_id:?} is missing every {cycle_type:?} {observable:?} point"
                ))
            })?;
            let mut selected_sample_u3_hex = String::with_capacity(255);
            let mut selected_sample_counts = [0u16; 8];
            let mut uncovered_selection_count = 0u16;
            for mask in 1u16..=255 {
                let selected = outcomes[usize::from(mask)].ok_or_else(|| {
                    ValidationError::new(format!(
                        "representative-sample sweep {sweep_id:?} is missing {cycle_type:?} {observable:?} mask 0x{mask:02x}"
                    ))
                })?;
                selected_sample_u3_hex.push(
                    char::from_digit(u32::from(selected), 16)
                        .expect("selected sample is validated as 0..=7"),
                );
                selected_sample_counts[usize::from(selected)] += 1;
                if mask & (1u16 << selected) == 0 {
                    uncovered_selection_count += 1;
                }
            }
            tables.push(RepresentativeSampleSelectorTable {
                cycle_type,
                observable,
                selected_sample_u3_hex,
                selected_sample_counts,
                uncovered_selection_count,
            });
        }
    }

    let cycle_comparisons = observable_order
        .iter()
        .enumerate()
        .map(|(index, &observable)| RepresentativeSampleCycleComparison {
            observable,
            matches: tables[index].selected_sample_u3_hex
                == tables[index + 3].selected_sample_u3_hex,
        })
        .collect::<Vec<_>>();
    let observable_comparisons = [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle]
        .iter()
        .enumerate()
        .map(|(cycle_index, &cycle_type)| {
            let base = cycle_index * 3;
            let shade_texture_match =
                tables[base].selected_sample_u3_hex == tables[base + 1].selected_sample_u3_hex;
            let shade_depth_match =
                tables[base].selected_sample_u3_hex == tables[base + 2].selected_sample_u3_hex;
            let texture_depth_match =
                tables[base + 1].selected_sample_u3_hex == tables[base + 2].selected_sample_u3_hex;
            RepresentativeSampleObservableComparison {
                cycle_type,
                shade_texture_match,
                shade_depth_match,
                texture_depth_match,
                all_match: shade_texture_match && shade_depth_match,
            }
        })
        .collect::<Vec<_>>();
    let all_cycle_results_match = cycle_comparisons
        .iter()
        .all(|comparison| comparison.matches);
    let all_observable_results_match = observable_comparisons
        .iter()
        .all(|comparison| comparison.all_match);
    let controls = controls.expect("a matching case sets controls");
    let geometry = geometry.expect("a matching case sets geometry");

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        controls: RepresentativeSampleControls,
        geometry: RepresentativeSampleGeometry,
        tables: &'a [RepresentativeSampleSelectorTable],
        cycle_comparisons: &'a [RepresentativeSampleCycleComparison],
        observable_comparisons: &'a [RepresentativeSampleObservableComparison],
        all_cycle_results_match: bool,
        all_observable_results_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-representative-sample-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        controls,
        geometry,
        tables: &tables,
        cycle_comparisons: &cycle_comparisons,
        observable_comparisons: &observable_comparisons,
        all_cycle_results_match,
        all_observable_results_match,
    };
    let canonical = serde_json::to_vec(&hash_input).map_err(|error| {
        ValidationError::new(format!("hash representative-sample analysis: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-representative-sample-analysis.v1\0");
    hasher.update(canonical);

    Ok(RepresentativeSampleSelectorAnalysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256: hex(&hasher.finalize()),
        sweep_id: sweep_id.to_owned(),
        controls,
        geometry,
        tables,
        cycle_comparisons,
        observable_comparisons,
        all_cycle_results_match,
        all_observable_results_match,
    })
}

/// Validate and preserve a complete narrow-edge boundary matrix. This checks
/// only the declared fixed-point envelope and exact observations; it neither
/// derives nor fits a silicon coverage-correction rule.
pub fn analyze_narrow_edge_coverage_correction_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<NarrowEdgeCoverageAnalysis, ValidationError> {
    text("narrow-edge-coverage sweep_id", sweep_id)?;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Point {
        edge_accumulator_i64: i64,
        coverage_mask_u8: u8,
        coverage_count_u4: u8,
        observations: [Option<NarrowEdgeObservableObservation>; 3],
    }

    let mut points = BTreeMap::<(i64, ProbeCycleType, NarrowEdgeBoundaryPosition), Point>::new();
    let mut controls: Option<NarrowEdgeCoverageControls> = None;
    let mut setup: Option<Setup> = None;
    let mut geometry = None;
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::NarrowEdgeCoverageCorrectionSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            observable,
            boundary_position,
            replay_from_reset,
            controls: point_controls,
            edge_boundary_i64,
            edge_accumulator_i64,
            coverage_mask_u8,
            coverage_count_u4,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: narrow-edge-coverage sweep must replay from reset before every point",
                case.case_id
            )));
        }
        if let Some(expected) = &controls {
            if expected != point_controls {
                return Err(ValidationError::new(format!(
                    "case {:?}: narrow-edge-coverage fixed controls differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            controls = Some(point_controls.clone());
        }
        if let Some(expected) = &setup {
            if expected != &case.setup {
                return Err(ValidationError::new(format!(
                    "case {:?}: narrow-edge-coverage setup differs within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            setup = Some(case.setup.clone());
        }

        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 4
            || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
            || depth.width != 1
            || depth.height != 1
            || depth.row_stride_bytes != 2
            || coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::CoverageCountU4
        {
            return Err(ValidationError::new(format!(
                "case {:?}: narrow-edge-coverage sweep requires exact 1x1 RGBA32, depth, and coverage_count_u4 planes",
                case.case_id
            )));
        }
        let point_geometry = NarrowEdgeCoverageGeometry {
            framebuffer_address: framebuffer.address,
            depth_address: depth.address,
            color_image_address: coverage.color_image_address,
        };
        if let Some(expected) = geometry {
            if expected != point_geometry {
                return Err(ValidationError::new(format!(
                    "case {:?}: narrow-edge-coverage output addresses differ within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            geometry = Some(point_geometry);
        }

        let observed_coverage_count =
            decode_blob("narrow-edge-coverage coverage", &coverage.contents)?[0];
        if observed_coverage_count != *coverage_count_u4 {
            return Err(ValidationError::new(format!(
                "case {:?}: declared coverage count {coverage_count_u4}, observed {observed_coverage_count}",
                case.case_id
            )));
        }
        let framebuffer_bytes =
            decode_blob("narrow-edge-coverage framebuffer", &framebuffer.contents)?;
        let framebuffer_word = u32::from_be_bytes(
            framebuffer_bytes
                .as_slice()
                .try_into()
                .expect("exact RGBA32 geometry has four bytes"),
        );
        let depth_bytes = decode_blob("narrow-edge-coverage depth", &depth.contents)?;
        let depth_word = u16::from_be_bytes(
            depth_bytes
                .as_slice()
                .try_into()
                .expect("exact depth geometry has two bytes"),
        );
        let markers = point_controls.markers;
        let observed_sample = match observable {
            RepresentativeSampleObservable::Shade => {
                if depth_word != markers.color_observable_depth_control_u16_be {
                    return Err(ValidationError::new(format!(
                        "case {:?}: narrow-edge shade observation changed the fixed depth control",
                        case.case_id
                    )));
                }
                markers
                    .shade_rgba32_be
                    .iter()
                    .position(|&marker| marker == framebuffer_word)
            }
            RepresentativeSampleObservable::Texture => {
                if depth_word != markers.color_observable_depth_control_u16_be {
                    return Err(ValidationError::new(format!(
                        "case {:?}: narrow-edge texture observation changed the fixed depth control",
                        case.case_id
                    )));
                }
                markers
                    .texture_rgba32_be
                    .iter()
                    .position(|&marker| marker == framebuffer_word)
            }
            RepresentativeSampleObservable::Depth => {
                if framebuffer_word != markers.depth_observable_color_control_rgba32_be {
                    return Err(ValidationError::new(format!(
                        "case {:?}: narrow-edge depth observation changed the fixed color control",
                        case.case_id
                    )));
                }
                markers
                    .depth_u16_be
                    .iter()
                    .position(|&marker| marker == depth_word)
            }
        }
        .ok_or_else(|| {
            ValidationError::new(format!(
                "case {:?}: narrow-edge output does not identify one {observable:?} sample; cross-label markers are not admitted",
                case.case_id
            ))
        })?;
        let observation = NarrowEdgeObservableObservation {
            observable: *observable,
            framebuffer_rgba32_be: framebuffer_word,
            depth_u16_be: depth_word,
            observed_coverage_count_u4: observed_coverage_count,
            observed_sample_index_u3: u8::try_from(observed_sample)
                .expect("marker sets contain exactly eight samples"),
        };
        let observable_index = match observable {
            RepresentativeSampleObservable::Shade => 0,
            RepresentativeSampleObservable::Texture => 1,
            RepresentativeSampleObservable::Depth => 2,
        };
        let point = points
            .entry((*edge_boundary_i64, *cycle_type, *boundary_position))
            .or_insert(Point {
                edge_accumulator_i64: *edge_accumulator_i64,
                coverage_mask_u8: *coverage_mask_u8,
                coverage_count_u4: *coverage_count_u4,
                observations: [None, None, None],
            });
        if point.edge_accumulator_i64 != *edge_accumulator_i64
            || point.coverage_mask_u8 != *coverage_mask_u8
            || point.coverage_count_u4 != *coverage_count_u4
        {
            return Err(ValidationError::new(format!(
                "case {:?}: narrow-edge raw accumulator or coverage declaration differs across observables",
                case.case_id
            )));
        }
        if point.observations[observable_index]
            .replace(observation)
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate narrow-edge-coverage point for boundary {edge_boundary_i64}, {cycle_type:?} {boundary_position:?} {observable:?} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no narrow-edge-coverage capture intent for sweep {sweep_id:?}"
        )));
    }
    let controls = controls.expect("a matching case sets controls");
    let geometry = geometry.expect("a matching case sets geometry");
    let positions = [
        NarrowEdgeBoundaryPosition::Below,
        NarrowEdgeBoundaryPosition::On,
        NarrowEdgeBoundaryPosition::Above,
    ];
    let mut boundaries = Vec::with_capacity(controls.selected_boundaries_i64.len());
    let mut all_observable_sample_indices_match = true;
    for &edge_boundary_i64 in &controls.selected_boundaries_i64 {
        let mut boundary_points = Vec::with_capacity(6);
        for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
            for boundary_position in positions {
                let point = points
                    .get(&(edge_boundary_i64, cycle_type, boundary_position))
                    .ok_or_else(|| {
                        ValidationError::new(format!(
                            "narrow-edge-coverage sweep {sweep_id:?} is missing boundary {edge_boundary_i64} {cycle_type:?} {boundary_position:?}"
                        ))
                    })?;
                let observations = point
                    .observations
                    .iter()
                    .enumerate()
                    .map(|(index, observation)| {
                        observation.ok_or_else(|| {
                            let observable = [
                                RepresentativeSampleObservable::Shade,
                                RepresentativeSampleObservable::Texture,
                                RepresentativeSampleObservable::Depth,
                            ][index];
                            ValidationError::new(format!(
                                "narrow-edge-coverage sweep {sweep_id:?} is missing boundary {edge_boundary_i64} {cycle_type:?} {boundary_position:?} {observable:?}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let observable_sample_indices_match = observations.windows(2).all(|pair| {
                    pair[0].observed_sample_index_u3 == pair[1].observed_sample_index_u3
                });
                all_observable_sample_indices_match &= observable_sample_indices_match;
                boundary_points.push(NarrowEdgePointAnalysis {
                    cycle_type,
                    boundary_position,
                    edge_accumulator_i64: point.edge_accumulator_i64,
                    coverage_mask_u8: point.coverage_mask_u8,
                    coverage_count_u4: point.coverage_count_u4,
                    observations,
                    observable_sample_indices_match,
                });
            }
        }
        boundaries.push(NarrowEdgeBoundaryAnalysis {
            edge_boundary_i64,
            points: boundary_points,
        });
    }
    if points.len() != controls.selected_boundaries_i64.len() * 6 {
        return Err(ValidationError::new(format!(
            "narrow-edge-coverage sweep {sweep_id:?} contains a point outside its selected boundary matrix"
        )));
    }
    let all_cycle_results_match = boundaries.iter().all(|boundary| {
        (0..3).all(|position| {
            let one = &boundary.points[position];
            let two = &boundary.points[position + 3];
            one.edge_accumulator_i64 == two.edge_accumulator_i64
                && one.coverage_mask_u8 == two.coverage_mask_u8
                && one.coverage_count_u4 == two.coverage_count_u4
                && one.observations == two.observations
        })
    });

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        controls: &'a NarrowEdgeCoverageControls,
        geometry: NarrowEdgeCoverageGeometry,
        boundaries: &'a [NarrowEdgeBoundaryAnalysis],
        all_cycle_results_match: bool,
        all_observable_sample_indices_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-narrow-edge-coverage-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        controls: &controls,
        geometry,
        boundaries: &boundaries,
        all_cycle_results_match,
        all_observable_sample_indices_match,
    };
    let canonical = serde_json::to_vec(&hash_input).map_err(|error| {
        ValidationError::new(format!("hash narrow-edge-coverage analysis: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-narrow-edge-coverage-analysis.v1\0");
    hasher.update(canonical);

    Ok(NarrowEdgeCoverageAnalysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256: hex(&hasher.finalize()),
        sweep_id: sweep_id.to_owned(),
        controls,
        geometry,
        boundaries,
        all_cycle_results_match,
        all_observable_sample_indices_match,
    })
}

/// Validate and preserve one complete below/on/above three-nearest filter
/// boundary matrix in both cycle modes. This proves envelope completeness and
/// deterministic reduction only; capture intent remains a producer assertion
/// and the observations do not establish a silicon arithmetic rule.
pub fn analyze_texture_filter_tie_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<TextureFilterTieAnalysis, ValidationError> {
    text("texture-filter-tie sweep_id", sweep_id)?;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Controls {
        texture_address: u32,
        texel_rgba16_be: [u16; 4],
        s_texel_i10: i16,
        t_texel_i10: i16,
        diagonal_boundary_u6: u8,
        geometry: TextureFilterTieGeometry,
        setup: Setup,
    }

    let mut controls: Option<Controls> = None;
    let mut points =
        BTreeMap::<(ProbeCycleType, FilterTieBoundaryPosition), TextureFilterTieObservation>::new();
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::TextureFilterTieSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            boundary_position,
            replay_from_reset,
            sample_x,
            sample_y,
            texture_address,
            texel_rgba16_be,
            s_texel_i10,
            t_texel_i10,
            s_fraction_u5,
            t_fraction_u5,
            diagonal_boundary_u6,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: texture-filter-tie sweep must replay from reset before every point",
                case.case_id
            )));
        }

        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 4
            || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
            || depth.width != 1
            || depth.height != 1
            || depth.row_stride_bytes != 2
            || coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::StoredCoverageU3
        {
            return Err(ValidationError::new(format!(
                "case {:?}: texture-filter-tie sweep requires exact 1x1 RGBA32 framebuffer, depth, and stored-coverage planes",
                case.case_id
            )));
        }

        let texture_regions = case
            .setup
            .initial_memory
            .iter()
            .filter(|region| {
                region.role == MemoryRole::Texture && region.address == *texture_address
            })
            .collect::<Vec<_>>();
        if texture_regions.len() != 1 {
            return Err(ValidationError::new(format!(
                "case {:?}: texture-filter-tie sweep requires exactly one declared texture region at {texture_address:#010x}",
                case.case_id
            )));
        }
        let expected_texture = texel_rgba16_be
            .iter()
            .flat_map(|texel| texel.to_be_bytes())
            .collect::<Vec<_>>();
        if decode_blob("texture-filter-tie texture", &texture_regions[0].contents)?
            != expected_texture
        {
            return Err(ValidationError::new(format!(
                "case {:?}: texture region bytes do not equal declared RGBA16 texels",
                case.case_id
            )));
        }

        let point_controls = Controls {
            texture_address: *texture_address,
            texel_rgba16_be: *texel_rgba16_be,
            s_texel_i10: *s_texel_i10,
            t_texel_i10: *t_texel_i10,
            diagonal_boundary_u6: *diagonal_boundary_u6,
            geometry: TextureFilterTieGeometry {
                framebuffer_address: framebuffer.address,
                depth_address: depth.address,
                coverage_address: coverage.color_image_address,
                sample_x: *sample_x,
                sample_y: *sample_y,
            },
            setup: case.setup.clone(),
        };
        if let Some(expected) = &controls {
            if expected != &point_controls {
                return Err(ValidationError::new(format!(
                    "case {:?}: texture, integer coordinates, boundary, reset setup, or output geometry differs within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            controls = Some(point_controls);
        }

        let framebuffer_bytes =
            decode_blob("texture-filter-tie framebuffer", &framebuffer.contents)?;
        let depth_bytes = decode_blob("texture-filter-tie depth", &depth.contents)?;
        let coverage_bytes = decode_blob("texture-filter-tie stored coverage", &coverage.contents)?;
        let observation = TextureFilterTieObservation {
            boundary_position: *boundary_position,
            s_fraction_u5: *s_fraction_u5,
            t_fraction_u5: *t_fraction_u5,
            framebuffer_rgba32_be: u32::from_be_bytes(
                framebuffer_bytes
                    .try_into()
                    .expect("validated 1x1 RGBA32 framebuffer is four bytes"),
            ),
            depth_u16_be: u16::from_be_bytes(
                depth_bytes
                    .try_into()
                    .expect("validated 1x1 depth plane is two bytes"),
            ),
            stored_coverage_u3: coverage_bytes[0],
        };
        if points
            .insert((*cycle_type, *boundary_position), observation)
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate texture-filter-tie point for {cycle_type:?} {boundary_position:?} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no texture-filter-tie capture intent for sweep {sweep_id:?}"
        )));
    }
    let controls = controls.expect("a matching case establishes controls");
    let positions = [
        FilterTieBoundaryPosition::Below,
        FilterTieBoundaryPosition::On,
        FilterTieBoundaryPosition::Above,
    ];
    let mut cycles = Vec::with_capacity(2);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        let observations = positions
            .into_iter()
            .map(|position| {
                points
                    .get(&(cycle_type, position))
                    .copied()
                    .ok_or_else(|| {
                        ValidationError::new(format!(
                            "texture-filter-tie sweep {sweep_id:?} is missing {cycle_type:?} {position:?} point"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        cycles.push(TextureFilterTieCycleAnalysis {
            cycle_type,
            observations,
        });
    }
    for (one_cycle, two_cycle) in cycles[0].observations.iter().zip(&cycles[1].observations) {
        if (one_cycle.s_fraction_u5, one_cycle.t_fraction_u5)
            != (two_cycle.s_fraction_u5, two_cycle.t_fraction_u5)
        {
            return Err(ValidationError::new(format!(
                "texture-filter-tie sweep {sweep_id:?} uses different {:?} fraction pairs across cycle modes",
                one_cycle.boundary_position
            )));
        }
    }
    let cycle_results_match = cycles[0]
        .observations
        .iter()
        .zip(&cycles[1].observations)
        .all(|(one, two)| {
            one.framebuffer_rgba32_be == two.framebuffer_rgba32_be
                && one.depth_u16_be == two.depth_u16_be
                && one.stored_coverage_u3 == two.stored_coverage_u3
        });

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        texture_address: u32,
        texel_rgba16_be: [u16; 4],
        s_texel_i10: i16,
        t_texel_i10: i16,
        diagonal_boundary_u6: u8,
        geometry: TextureFilterTieGeometry,
        cycles: &'a [TextureFilterTieCycleAnalysis],
        cycle_results_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-texture-filter-tie-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        texture_address: controls.texture_address,
        texel_rgba16_be: controls.texel_rgba16_be,
        s_texel_i10: controls.s_texel_i10,
        t_texel_i10: controls.t_texel_i10,
        diagonal_boundary_u6: controls.diagonal_boundary_u6,
        geometry: controls.geometry,
        cycles: &cycles,
        cycle_results_match,
    };
    let canonical = serde_json::to_vec(&hash_input).map_err(|error| {
        ValidationError::new(format!("hash texture-filter-tie analysis: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-texture-filter-tie-analysis.v1\0");
    hasher.update(canonical);

    Ok(TextureFilterTieAnalysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256: hex(&hasher.finalize()),
        sweep_id: sweep_id.to_owned(),
        texture_address: controls.texture_address,
        texel_rgba16_be: controls.texel_rgba16_be,
        s_texel_i10: controls.s_texel_i10,
        t_texel_i10: controls.t_texel_i10,
        diagonal_boundary_u6: controls.diagonal_boundary_u6,
        geometry: controls.geometry,
        cycles,
        cycle_results_match,
    })
}

/// Validate and preserve one complete reciprocal-to-signed-S10.5 boundary
/// matrix in both cycle modes. The rational inputs and expected output
/// markers remain producer declarations; no silicon reciprocal arithmetic is
/// inferred from them.
pub fn analyze_reciprocal_s10_5_boundary_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<ReciprocalS10_5Analysis, ValidationError> {
    text("reciprocal-S10.5 sweep_id", sweep_id)?;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct CommonControls {
        boundary_s10_5_i16: i16,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        geometry: ReciprocalS10_5Geometry,
        setup: Setup,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct PointControls {
        perspective_numerator_i64: i64,
        perspective_denominator_u64: u64,
        producer_expected_output_s10_5_i16: i16,
        producer_expected_framebuffer_rgba32_be: u32,
    }

    let mut common_controls: Option<CommonControls> = None;
    let mut point_controls = BTreeMap::<ReciprocalBoundaryPosition, PointControls>::new();
    let mut points =
        BTreeMap::<(ProbeCycleType, ReciprocalBoundaryPosition), ReciprocalS10_5Observation>::new();
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::ReciprocalS10_5BoundarySweep {
            sweep_id: case_sweep_id,
            cycle_type,
            boundary_position,
            replay_from_reset,
            sample_x,
            sample_y,
            boundary_s10_5_i16,
            perspective_numerator_i64,
            perspective_denominator_u64,
            producer_expected_output_s10_5_i16,
            producer_expected_framebuffer_rgba32_be,
            depth_control_u16_be,
            stored_coverage_control_u3,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: reciprocal-S10.5 sweep must replay from reset before every point",
                case.case_id
            )));
        }

        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 4
            || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
            || depth.width != 1
            || depth.height != 1
            || depth.row_stride_bytes != 2
            || coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::StoredCoverageU3
        {
            return Err(ValidationError::new(format!(
                "case {:?}: reciprocal-S10.5 sweep requires exact 1x1 RGBA32 framebuffer, depth, and stored-coverage planes",
                case.case_id
            )));
        }

        let candidate_common = CommonControls {
            boundary_s10_5_i16: *boundary_s10_5_i16,
            depth_control_u16_be: *depth_control_u16_be,
            stored_coverage_control_u3: *stored_coverage_control_u3,
            geometry: ReciprocalS10_5Geometry {
                framebuffer_address: framebuffer.address,
                depth_address: depth.address,
                coverage_address: coverage.color_image_address,
                sample_x: *sample_x,
                sample_y: *sample_y,
            },
            setup: case.setup.clone(),
        };
        if let Some(expected) = &common_controls {
            if expected != &candidate_common {
                return Err(ValidationError::new(format!(
                    "case {:?}: reciprocal boundary, reset setup, output controls, or geometry differs within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            common_controls = Some(candidate_common);
        }

        let candidate_point = PointControls {
            perspective_numerator_i64: *perspective_numerator_i64,
            perspective_denominator_u64: *perspective_denominator_u64,
            producer_expected_output_s10_5_i16: *producer_expected_output_s10_5_i16,
            producer_expected_framebuffer_rgba32_be: *producer_expected_framebuffer_rgba32_be,
        };
        if let Some(expected) = point_controls.get(boundary_position) {
            if expected != &candidate_point {
                return Err(ValidationError::new(format!(
                    "reciprocal-S10.5 sweep {sweep_id:?} uses different {boundary_position:?} input/output controls across cycle modes"
                )));
            }
        } else {
            point_controls.insert(*boundary_position, candidate_point);
        }

        let framebuffer_bytes = decode_blob("reciprocal-S10.5 framebuffer", &framebuffer.contents)?;
        let depth_bytes = decode_blob("reciprocal-S10.5 depth", &depth.contents)?;
        let coverage_bytes = decode_blob("reciprocal-S10.5 stored coverage", &coverage.contents)?;
        let observed_depth = u16::from_be_bytes(
            depth_bytes
                .try_into()
                .expect("validated 1x1 depth plane is two bytes"),
        );
        let observed_coverage = coverage_bytes[0];
        if observed_depth != *depth_control_u16_be
            || observed_coverage != *stored_coverage_control_u3
        {
            return Err(ValidationError::new(format!(
                "case {:?}: reciprocal-S10.5 depth or coverage output changed from its fixed control",
                case.case_id
            )));
        }
        let observed_framebuffer = u32::from_be_bytes(
            framebuffer_bytes
                .try_into()
                .expect("validated 1x1 RGBA32 framebuffer is four bytes"),
        );
        let observation = ReciprocalS10_5Observation {
            boundary_position: *boundary_position,
            perspective_numerator_i64: *perspective_numerator_i64,
            perspective_denominator_u64: *perspective_denominator_u64,
            producer_expected_output_s10_5_i16: *producer_expected_output_s10_5_i16,
            producer_expected_framebuffer_rgba32_be: *producer_expected_framebuffer_rgba32_be,
            framebuffer_rgba32_be: observed_framebuffer,
            observed_output_s10_5_i16: None,
            output_matches_producer_expectation: observed_framebuffer
                == *producer_expected_framebuffer_rgba32_be,
            depth_u16_be: observed_depth,
            stored_coverage_u3: observed_coverage,
        };
        if points
            .insert((*cycle_type, *boundary_position), observation)
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate reciprocal-S10.5 point for {cycle_type:?} {boundary_position:?} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no reciprocal-S10.5 capture intent for sweep {sweep_id:?}"
        )));
    }
    let common_controls = common_controls.expect("a matching case establishes controls");
    let positions = [
        ReciprocalBoundaryPosition::Below,
        ReciprocalBoundaryPosition::On,
        ReciprocalBoundaryPosition::Above,
    ];
    let ordered_controls = positions
        .into_iter()
        .map(|position| {
            point_controls.get(&position).copied().ok_or_else(|| {
                ValidationError::new(format!(
                    "reciprocal-S10.5 sweep {sweep_id:?} has no declared {position:?} controls"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let denominator = ordered_controls[1].perspective_denominator_u64;
    let on_numerator = i128::from(ordered_controls[1].perspective_numerator_i64);
    let exact_boundary_numerator =
        i128::from(common_controls.boundary_s10_5_i16) * i128::from(denominator);
    if ordered_controls
        .iter()
        .any(|control| control.perspective_denominator_u64 != denominator)
        || on_numerator != exact_boundary_numerator
        || i128::from(ordered_controls[0].perspective_numerator_i64) != on_numerator - 1
        || i128::from(ordered_controls[2].perspective_numerator_i64) != on_numerator + 1
    {
        return Err(ValidationError::new(format!(
            "reciprocal-S10.5 sweep {sweep_id:?} must use one denominator and exact numerator boundary-1/boundary/boundary+1 inputs"
        )));
    }

    let mut marker_to_output = BTreeMap::<u32, i16>::new();
    let mut output_to_marker = BTreeMap::<i16, u32>::new();
    for control in &ordered_controls {
        if let Some(output) = marker_to_output.insert(
            control.producer_expected_framebuffer_rgba32_be,
            control.producer_expected_output_s10_5_i16,
        ) {
            if output != control.producer_expected_output_s10_5_i16 {
                return Err(ValidationError::new(format!(
                    "reciprocal-S10.5 sweep {sweep_id:?} assigns one output marker to different expected S10.5 values"
                )));
            }
        }
        if let Some(marker) = output_to_marker.insert(
            control.producer_expected_output_s10_5_i16,
            control.producer_expected_framebuffer_rgba32_be,
        ) {
            if marker != control.producer_expected_framebuffer_rgba32_be {
                return Err(ValidationError::new(format!(
                    "reciprocal-S10.5 sweep {sweep_id:?} assigns different output markers to one expected S10.5 value"
                )));
            }
        }
    }
    for observation in points.values_mut() {
        observation.observed_output_s10_5_i16 = marker_to_output
            .get(&observation.framebuffer_rgba32_be)
            .copied();
    }

    let mut cycles = Vec::with_capacity(2);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        let observations = positions
            .into_iter()
            .map(|position| {
                points
                    .get(&(cycle_type, position))
                    .copied()
                    .ok_or_else(|| {
                        ValidationError::new(format!(
                            "reciprocal-S10.5 sweep {sweep_id:?} is missing {cycle_type:?} {position:?} point"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        cycles.push(ReciprocalS10_5CycleAnalysis {
            cycle_type,
            observations,
        });
    }
    let unexpected_output_count = cycles
        .iter()
        .flat_map(|cycle| &cycle.observations)
        .filter(|observation| !observation.output_matches_producer_expectation)
        .count() as u8;
    let cycle_results_match = cycles[0]
        .observations
        .iter()
        .zip(&cycles[1].observations)
        .all(|(one, two)| {
            one.framebuffer_rgba32_be == two.framebuffer_rgba32_be
                && one.depth_u16_be == two.depth_u16_be
                && one.stored_coverage_u3 == two.stored_coverage_u3
        });

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        boundary_s10_5_i16: i16,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        geometry: ReciprocalS10_5Geometry,
        cycles: &'a [ReciprocalS10_5CycleAnalysis],
        unexpected_output_count: u8,
        cycle_results_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-reciprocal-s10-5-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        boundary_s10_5_i16: common_controls.boundary_s10_5_i16,
        depth_control_u16_be: common_controls.depth_control_u16_be,
        stored_coverage_control_u3: common_controls.stored_coverage_control_u3,
        geometry: common_controls.geometry,
        cycles: &cycles,
        unexpected_output_count,
        cycle_results_match,
    };
    let canonical = serde_json::to_vec(&hash_input).map_err(|error| {
        ValidationError::new(format!("hash reciprocal-S10.5 analysis: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-reciprocal-s10-5-analysis.v1\0");
    hasher.update(canonical);

    Ok(ReciprocalS10_5Analysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256: hex(&hasher.finalize()),
        sweep_id: sweep_id.to_owned(),
        boundary_s10_5_i16: common_controls.boundary_s10_5_i16,
        depth_control_u16_be: common_controls.depth_control_u16_be,
        stored_coverage_control_u3: common_controls.stored_coverage_control_u3,
        geometry: common_controls.geometry,
        cycles,
        unexpected_output_count,
        cycle_results_match,
    })
}

/// Validate and preserve one complete average-filter output-tie matrix in
/// both cycle modes. Exact accumulator controls remain producer declarations;
/// this function neither derives them from the texels nor infers a silicon
/// averaging or rounding rule.
pub fn analyze_average_filter_output_tie_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<AverageFilterTieAnalysis, ValidationError> {
    text("average-filter-tie sweep_id", sweep_id)?;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct CommonControls {
        texture_address: u32,
        texel_rgba16_be: [u16; 4],
        s_texel_i10: i16,
        t_texel_i10: i16,
        isolated_channel: AverageFilterChannel,
        tie_numerator_i64: i64,
        accumulator_denominator_u64: u64,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        geometry: AverageFilterTieGeometry,
        setup: Setup,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct PointControls {
        s_fraction_u5: u8,
        t_fraction_u5: u8,
        accumulator_numerator_i64: i64,
        producer_expected_output_u8: u8,
        producer_expected_framebuffer_rgba32_be: u32,
    }

    let mut common_controls: Option<CommonControls> = None;
    let mut point_controls = BTreeMap::<AverageFilterTiePosition, PointControls>::new();
    let mut points =
        BTreeMap::<(ProbeCycleType, AverageFilterTiePosition), AverageFilterTieObservation>::new();
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::AverageFilterOutputTieSweep {
            sweep_id: case_sweep_id,
            cycle_type,
            tie_position,
            replay_from_reset,
            sample_x,
            sample_y,
            texture_address,
            texel_rgba16_be,
            s_texel_i10,
            t_texel_i10,
            s_fraction_u5,
            t_fraction_u5,
            isolated_channel,
            accumulator_numerator_i64,
            accumulator_denominator_u64,
            tie_numerator_i64,
            producer_expected_output_u8,
            producer_expected_framebuffer_rgba32_be,
            depth_control_u16_be,
            stored_coverage_control_u3,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: average-filter-tie sweep must replay from reset before every point",
                case.case_id
            )));
        }

        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 4
            || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
            || depth.width != 1
            || depth.height != 1
            || depth.row_stride_bytes != 2
            || coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::StoredCoverageU3
        {
            return Err(ValidationError::new(format!(
                "case {:?}: average-filter-tie sweep requires exact 1x1 RGBA32 framebuffer, depth, and stored-coverage planes",
                case.case_id
            )));
        }

        let texture_regions = case
            .setup
            .initial_memory
            .iter()
            .filter(|region| {
                region.role == MemoryRole::Texture && region.address == *texture_address
            })
            .collect::<Vec<_>>();
        if texture_regions.len() != 1 {
            return Err(ValidationError::new(format!(
                "case {:?}: average-filter-tie sweep requires exactly one declared texture region at {texture_address:#010x}",
                case.case_id
            )));
        }
        let expected_texture = texel_rgba16_be
            .iter()
            .flat_map(|texel| texel.to_be_bytes())
            .collect::<Vec<_>>();
        if decode_blob("average-filter-tie texture", &texture_regions[0].contents)?
            != expected_texture
        {
            return Err(ValidationError::new(format!(
                "case {:?}: texture region bytes do not equal declared average-filter RGBA16 texels",
                case.case_id
            )));
        }

        let candidate_common = CommonControls {
            texture_address: *texture_address,
            texel_rgba16_be: *texel_rgba16_be,
            s_texel_i10: *s_texel_i10,
            t_texel_i10: *t_texel_i10,
            isolated_channel: *isolated_channel,
            tie_numerator_i64: *tie_numerator_i64,
            accumulator_denominator_u64: *accumulator_denominator_u64,
            depth_control_u16_be: *depth_control_u16_be,
            stored_coverage_control_u3: *stored_coverage_control_u3,
            geometry: AverageFilterTieGeometry {
                framebuffer_address: framebuffer.address,
                depth_address: depth.address,
                coverage_address: coverage.color_image_address,
                sample_x: *sample_x,
                sample_y: *sample_y,
            },
            setup: case.setup.clone(),
        };
        if let Some(expected) = &common_controls {
            if expected != &candidate_common {
                return Err(ValidationError::new(format!(
                    "case {:?}: average-filter texture, integer coordinates, tie, setup, output controls, or geometry differs within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            common_controls = Some(candidate_common);
        }

        let candidate_point = PointControls {
            s_fraction_u5: *s_fraction_u5,
            t_fraction_u5: *t_fraction_u5,
            accumulator_numerator_i64: *accumulator_numerator_i64,
            producer_expected_output_u8: *producer_expected_output_u8,
            producer_expected_framebuffer_rgba32_be: *producer_expected_framebuffer_rgba32_be,
        };
        if let Some(expected) = point_controls.get(tie_position) {
            if expected != &candidate_point {
                return Err(ValidationError::new(format!(
                    "average-filter-tie sweep {sweep_id:?} uses different {tie_position:?} coordinate/accumulator/output controls across cycle modes"
                )));
            }
        } else {
            point_controls.insert(*tie_position, candidate_point);
        }

        let framebuffer_bytes =
            decode_blob("average-filter-tie framebuffer", &framebuffer.contents)?;
        let depth_bytes = decode_blob("average-filter-tie depth", &depth.contents)?;
        let coverage_bytes = decode_blob("average-filter-tie stored coverage", &coverage.contents)?;
        let observed_depth = u16::from_be_bytes(
            depth_bytes
                .try_into()
                .expect("validated 1x1 depth plane is two bytes"),
        );
        let observed_coverage = coverage_bytes[0];
        if observed_depth != *depth_control_u16_be
            || observed_coverage != *stored_coverage_control_u3
        {
            return Err(ValidationError::new(format!(
                "case {:?}: average-filter-tie depth or coverage output changed from its fixed control",
                case.case_id
            )));
        }
        let observed_framebuffer = u32::from_be_bytes(
            framebuffer_bytes
                .try_into()
                .expect("validated 1x1 RGBA32 framebuffer is four bytes"),
        );
        let observation = AverageFilterTieObservation {
            tie_position: *tie_position,
            s_fraction_u5: *s_fraction_u5,
            t_fraction_u5: *t_fraction_u5,
            accumulator_numerator_i64: *accumulator_numerator_i64,
            accumulator_denominator_u64: *accumulator_denominator_u64,
            producer_expected_output_u8: *producer_expected_output_u8,
            producer_expected_framebuffer_rgba32_be: *producer_expected_framebuffer_rgba32_be,
            framebuffer_rgba32_be: observed_framebuffer,
            observed_output_u8: None,
            output_matches_producer_expectation: observed_framebuffer
                == *producer_expected_framebuffer_rgba32_be,
            depth_u16_be: observed_depth,
            stored_coverage_u3: observed_coverage,
        };
        if points
            .insert((*cycle_type, *tie_position), observation)
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate average-filter-tie point for {cycle_type:?} {tie_position:?} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no average-filter-tie capture intent for sweep {sweep_id:?}"
        )));
    }
    let common_controls = common_controls.expect("a matching case establishes controls");
    let positions = [
        AverageFilterTiePosition::Below,
        AverageFilterTiePosition::On,
        AverageFilterTiePosition::Above,
    ];
    let ordered_controls = positions
        .into_iter()
        .map(|position| {
            point_controls.get(&position).copied().ok_or_else(|| {
                ValidationError::new(format!(
                    "average-filter-tie sweep {sweep_id:?} has no declared {position:?} controls"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let tie_numerator = i128::from(common_controls.tie_numerator_i64);
    if i128::from(ordered_controls[0].accumulator_numerator_i64) != tie_numerator - 1
        || i128::from(ordered_controls[1].accumulator_numerator_i64) != tie_numerator
        || i128::from(ordered_controls[2].accumulator_numerator_i64) != tie_numerator + 1
    {
        return Err(ValidationError::new(format!(
            "average-filter-tie sweep {sweep_id:?} must use exact accumulator numerator tie-1/tie/tie+1 inputs"
        )));
    }
    let fraction_pairs = ordered_controls
        .iter()
        .map(|control| (control.s_fraction_u5, control.t_fraction_u5))
        .collect::<BTreeSet<_>>();
    if fraction_pairs.len() != 3 {
        return Err(ValidationError::new(format!(
            "average-filter-tie sweep {sweep_id:?} must use distinct below/on/above fractional coordinate pairs"
        )));
    }

    let mut marker_to_output = BTreeMap::<u32, u8>::new();
    let mut output_to_marker = BTreeMap::<u8, u32>::new();
    for control in &ordered_controls {
        if let Some(output) = marker_to_output.insert(
            control.producer_expected_framebuffer_rgba32_be,
            control.producer_expected_output_u8,
        ) {
            if output != control.producer_expected_output_u8 {
                return Err(ValidationError::new(format!(
                    "average-filter-tie sweep {sweep_id:?} assigns one output marker to different expected channel values"
                )));
            }
        }
        if let Some(marker) = output_to_marker.insert(
            control.producer_expected_output_u8,
            control.producer_expected_framebuffer_rgba32_be,
        ) {
            if marker != control.producer_expected_framebuffer_rgba32_be {
                return Err(ValidationError::new(format!(
                    "average-filter-tie sweep {sweep_id:?} assigns different output markers to one expected channel value"
                )));
            }
        }
    }
    for observation in points.values_mut() {
        observation.observed_output_u8 = marker_to_output
            .get(&observation.framebuffer_rgba32_be)
            .copied();
    }

    let mut cycles = Vec::with_capacity(2);
    for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
        let observations = positions
            .into_iter()
            .map(|position| {
                points
                    .get(&(cycle_type, position))
                    .copied()
                    .ok_or_else(|| {
                        ValidationError::new(format!(
                            "average-filter-tie sweep {sweep_id:?} is missing {cycle_type:?} {position:?} point"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        cycles.push(AverageFilterTieCycleAnalysis {
            cycle_type,
            observations,
        });
    }
    let unexpected_output_count = cycles
        .iter()
        .flat_map(|cycle| &cycle.observations)
        .filter(|observation| !observation.output_matches_producer_expectation)
        .count() as u8;
    let cycle_results_match = cycles[0]
        .observations
        .iter()
        .zip(&cycles[1].observations)
        .all(|(one, two)| {
            one.framebuffer_rgba32_be == two.framebuffer_rgba32_be
                && one.depth_u16_be == two.depth_u16_be
                && one.stored_coverage_u3 == two.stored_coverage_u3
        });

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        texture_address: u32,
        texel_rgba16_be: [u16; 4],
        s_texel_i10: i16,
        t_texel_i10: i16,
        isolated_channel: AverageFilterChannel,
        tie_numerator_i64: i64,
        accumulator_denominator_u64: u64,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        geometry: AverageFilterTieGeometry,
        cycles: &'a [AverageFilterTieCycleAnalysis],
        unexpected_output_count: u8,
        cycle_results_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-average-filter-output-tie-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        texture_address: common_controls.texture_address,
        texel_rgba16_be: common_controls.texel_rgba16_be,
        s_texel_i10: common_controls.s_texel_i10,
        t_texel_i10: common_controls.t_texel_i10,
        isolated_channel: common_controls.isolated_channel,
        tie_numerator_i64: common_controls.tie_numerator_i64,
        accumulator_denominator_u64: common_controls.accumulator_denominator_u64,
        depth_control_u16_be: common_controls.depth_control_u16_be,
        stored_coverage_control_u3: common_controls.stored_coverage_control_u3,
        geometry: common_controls.geometry,
        cycles: &cycles,
        unexpected_output_count,
        cycle_results_match,
    };
    let canonical = serde_json::to_vec(&hash_input).map_err(|error| {
        ValidationError::new(format!("hash average-filter-tie analysis: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-average-filter-output-tie-analysis.v1\0");
    hasher.update(canonical);

    Ok(AverageFilterTieAnalysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256: hex(&hasher.finalize()),
        sweep_id: sweep_id.to_owned(),
        texture_address: common_controls.texture_address,
        texel_rgba16_be: common_controls.texel_rgba16_be,
        s_texel_i10: common_controls.s_texel_i10,
        t_texel_i10: common_controls.t_texel_i10,
        isolated_channel: common_controls.isolated_channel,
        tie_numerator_i64: common_controls.tie_numerator_i64,
        accumulator_denominator_u64: common_controls.accumulator_denominator_u64,
        depth_control_u16_be: common_controls.depth_control_u16_be,
        stored_coverage_control_u3: common_controls.stored_coverage_control_u3,
        geometry: common_controls.geometry,
        cycles,
        unexpected_output_count,
        cycle_results_match,
    })
}

/// Validate and preserve a complete mip/detail/sharpen derivative-boundary
/// matrix in both cycle modes. The exact coordinate deltas are checked, while
/// the LOD metric and expected selections remain producer declarations.
pub fn analyze_texture_lod_boundary_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<TextureLodBoundaryAnalysis, ValidationError> {
    text("texture-LOD sweep_id", sweep_id)?;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct CommonControls {
        lod_boundary_numerator_i64: i64,
        lod_metric_denominator_u64: u64,
        primitive_tile_u3: u8,
        max_mip_level_u3: u8,
        min_lod_u8: u8,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        geometry: TextureLodGeometry,
        setup: Setup,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct InputControls {
        center_s_s10_5_i16: i16,
        center_t_s10_5_i16: i16,
        x_neighbor_s_s10_5_i16: i16,
        x_neighbor_t_s10_5_i16: i16,
        y_neighbor_s_s10_5_i16: i16,
        y_neighbor_t_s10_5_i16: i16,
        dsdx_s10_5_i32: i32,
        dtdx_s10_5_i32: i32,
        dsdy_s10_5_i32: i32,
        dtdy_s10_5_i32: i32,
        lod_metric_numerator_i64: i64,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct ExpectedControls {
        selection: TextureLodExpectedSelection,
        marker: u32,
    }

    let mut common_controls: Option<CommonControls> = None;
    let mut input_controls = BTreeMap::<TextureLodBoundaryPosition, InputControls>::new();
    let mut expected_controls =
        BTreeMap::<(TextureLodMode, TextureLodBoundaryPosition), ExpectedControls>::new();
    let mut points = BTreeMap::<
        (TextureLodMode, ProbeCycleType, TextureLodBoundaryPosition),
        TextureLodObservation,
    >::new();
    let mut matching_cases = 0usize;

    for case in &bundle.bundle.cases {
        let Some(CaptureIntent::TextureLodBoundarySweep {
            sweep_id: case_sweep_id,
            cycle_type,
            lod_mode,
            boundary_position,
            replay_from_reset,
            sample_x,
            sample_y,
            center_s_s10_5_i16,
            center_t_s10_5_i16,
            x_neighbor_s_s10_5_i16,
            x_neighbor_t_s10_5_i16,
            y_neighbor_s_s10_5_i16,
            y_neighbor_t_s10_5_i16,
            dsdx_s10_5_i32,
            dtdx_s10_5_i32,
            dsdy_s10_5_i32,
            dtdy_s10_5_i32,
            lod_metric_numerator_i64,
            lod_metric_denominator_u64,
            lod_boundary_numerator_i64,
            primitive_tile_u3,
            max_mip_level_u3,
            min_lod_u8,
            producer_expected_tile0_u3,
            producer_expected_tile1_u3,
            producer_expected_lod_fraction_s9_8_i16,
            producer_expected_framebuffer_rgba32_be,
            depth_control_u16_be,
            stored_coverage_control_u3,
        }) = &case.capture_intent
        else {
            continue;
        };
        if case_sweep_id != sweep_id {
            continue;
        }
        matching_cases += 1;
        if !replay_from_reset {
            return Err(ValidationError::new(format!(
                "case {:?}: texture-LOD sweep must replay from reset before every point",
                case.case_id
            )));
        }

        let framebuffer = &case.expected.framebuffer;
        let depth = &case.expected.depth;
        let coverage = &case.expected.coverage;
        if framebuffer.width != 1
            || framebuffer.height != 1
            || framebuffer.row_stride_bytes != 4
            || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
            || depth.width != 1
            || depth.height != 1
            || depth.row_stride_bytes != 2
            || coverage.width != 1
            || coverage.height != 1
            || coverage.encoding != CoverageEncoding::StoredCoverageU3
        {
            return Err(ValidationError::new(format!(
                "case {:?}: texture-LOD sweep requires exact 1x1 RGBA32 framebuffer, depth, and stored-coverage planes",
                case.case_id
            )));
        }

        let candidate_common = CommonControls {
            lod_boundary_numerator_i64: *lod_boundary_numerator_i64,
            lod_metric_denominator_u64: *lod_metric_denominator_u64,
            primitive_tile_u3: *primitive_tile_u3,
            max_mip_level_u3: *max_mip_level_u3,
            min_lod_u8: *min_lod_u8,
            depth_control_u16_be: *depth_control_u16_be,
            stored_coverage_control_u3: *stored_coverage_control_u3,
            geometry: TextureLodGeometry {
                framebuffer_address: framebuffer.address,
                depth_address: depth.address,
                coverage_address: coverage.color_image_address,
                sample_x: *sample_x,
                sample_y: *sample_y,
            },
            setup: case.setup.clone(),
        };
        if let Some(expected) = &common_controls {
            if expected != &candidate_common {
                return Err(ValidationError::new(format!(
                    "case {:?}: texture-LOD boundary, tile controls, setup, output controls, or geometry differs within sweep {:?}",
                    case.case_id, sweep_id
                )));
            }
        } else {
            common_controls = Some(candidate_common);
        }

        let candidate_input = InputControls {
            center_s_s10_5_i16: *center_s_s10_5_i16,
            center_t_s10_5_i16: *center_t_s10_5_i16,
            x_neighbor_s_s10_5_i16: *x_neighbor_s_s10_5_i16,
            x_neighbor_t_s10_5_i16: *x_neighbor_t_s10_5_i16,
            y_neighbor_s_s10_5_i16: *y_neighbor_s_s10_5_i16,
            y_neighbor_t_s10_5_i16: *y_neighbor_t_s10_5_i16,
            dsdx_s10_5_i32: *dsdx_s10_5_i32,
            dtdx_s10_5_i32: *dtdx_s10_5_i32,
            dsdy_s10_5_i32: *dsdy_s10_5_i32,
            dtdy_s10_5_i32: *dtdy_s10_5_i32,
            lod_metric_numerator_i64: *lod_metric_numerator_i64,
        };
        if let Some(expected) = input_controls.get(boundary_position) {
            if expected != &candidate_input {
                return Err(ValidationError::new(format!(
                    "texture-LOD sweep {sweep_id:?} uses different {boundary_position:?} coordinates, derivatives, or metric across modes/cycles"
                )));
            }
        } else {
            input_controls.insert(*boundary_position, candidate_input);
        }

        let selection = TextureLodExpectedSelection {
            tile0_u3: *producer_expected_tile0_u3,
            tile1_u3: *producer_expected_tile1_u3,
            lod_fraction_s9_8_i16: *producer_expected_lod_fraction_s9_8_i16,
        };
        let candidate_expected = ExpectedControls {
            selection,
            marker: *producer_expected_framebuffer_rgba32_be,
        };
        let expected_key = (*lod_mode, *boundary_position);
        if let Some(expected) = expected_controls.get(&expected_key) {
            if expected != &candidate_expected {
                return Err(ValidationError::new(format!(
                    "texture-LOD sweep {sweep_id:?} uses different {lod_mode:?} {boundary_position:?} expected selection/output across cycle modes"
                )));
            }
        } else {
            expected_controls.insert(expected_key, candidate_expected);
        }

        let framebuffer_bytes = decode_blob("texture-LOD framebuffer", &framebuffer.contents)?;
        let depth_bytes = decode_blob("texture-LOD depth", &depth.contents)?;
        let coverage_bytes = decode_blob("texture-LOD stored coverage", &coverage.contents)?;
        let observed_depth = u16::from_be_bytes(
            depth_bytes
                .try_into()
                .expect("validated 1x1 depth plane is two bytes"),
        );
        let observed_coverage = coverage_bytes[0];
        let observed_framebuffer = u32::from_be_bytes(
            framebuffer_bytes
                .try_into()
                .expect("validated 1x1 RGBA32 framebuffer is four bytes"),
        );
        let observation = TextureLodObservation {
            boundary_position: *boundary_position,
            center_s_s10_5_i16: *center_s_s10_5_i16,
            center_t_s10_5_i16: *center_t_s10_5_i16,
            x_neighbor_s_s10_5_i16: *x_neighbor_s_s10_5_i16,
            x_neighbor_t_s10_5_i16: *x_neighbor_t_s10_5_i16,
            y_neighbor_s_s10_5_i16: *y_neighbor_s_s10_5_i16,
            y_neighbor_t_s10_5_i16: *y_neighbor_t_s10_5_i16,
            dsdx_s10_5_i32: *dsdx_s10_5_i32,
            dtdx_s10_5_i32: *dtdx_s10_5_i32,
            dsdy_s10_5_i32: *dsdy_s10_5_i32,
            dtdy_s10_5_i32: *dtdy_s10_5_i32,
            lod_metric_numerator_i64: *lod_metric_numerator_i64,
            lod_metric_denominator_u64: *lod_metric_denominator_u64,
            producer_expected_selection: selection,
            producer_expected_framebuffer_rgba32_be: *producer_expected_framebuffer_rgba32_be,
            framebuffer_rgba32_be: observed_framebuffer,
            observed_selection: None,
            output_matches_producer_expectation: observed_framebuffer
                == *producer_expected_framebuffer_rgba32_be,
            depth_u16_be: observed_depth,
            depth_matches_producer_control: observed_depth == *depth_control_u16_be,
            stored_coverage_u3: observed_coverage,
            coverage_matches_producer_control: observed_coverage == *stored_coverage_control_u3,
        };
        if points
            .insert((*lod_mode, *cycle_type, *boundary_position), observation)
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate texture-LOD point for {lod_mode:?} {cycle_type:?} {boundary_position:?} in sweep {sweep_id:?}"
            )));
        }
    }

    if matching_cases == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no texture-LOD capture intent for sweep {sweep_id:?}"
        )));
    }
    let common_controls = common_controls.expect("a matching case establishes controls");
    let positions = [
        TextureLodBoundaryPosition::Below,
        TextureLodBoundaryPosition::On,
        TextureLodBoundaryPosition::Above,
    ];
    let ordered_inputs = positions
        .into_iter()
        .map(|position| {
            input_controls.get(&position).copied().ok_or_else(|| {
                ValidationError::new(format!(
                    "texture-LOD sweep {sweep_id:?} has no declared {position:?} input controls"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let boundary = i128::from(common_controls.lod_boundary_numerator_i64);
    if i128::from(ordered_inputs[0].lod_metric_numerator_i64) != boundary - 1
        || i128::from(ordered_inputs[1].lod_metric_numerator_i64) != boundary
        || i128::from(ordered_inputs[2].lod_metric_numerator_i64) != boundary + 1
    {
        return Err(ValidationError::new(format!(
            "texture-LOD sweep {sweep_id:?} must use exact metric numerator boundary-1/boundary/boundary+1 inputs"
        )));
    }
    let derivative_controls = ordered_inputs
        .iter()
        .map(|input| {
            (
                input.dsdx_s10_5_i32,
                input.dtdx_s10_5_i32,
                input.dsdy_s10_5_i32,
                input.dtdy_s10_5_i32,
            )
        })
        .collect::<BTreeSet<_>>();
    if derivative_controls.len() != 3 {
        return Err(ValidationError::new(format!(
            "texture-LOD sweep {sweep_id:?} must use distinct below/on/above derivative tuples"
        )));
    }

    let mut marker_to_selection = BTreeMap::<u32, TextureLodExpectedSelection>::new();
    let mut selection_to_marker = BTreeMap::<TextureLodExpectedSelection, u32>::new();
    for controls in expected_controls.values() {
        if let Some(selection) = marker_to_selection.insert(controls.marker, controls.selection) {
            if selection != controls.selection {
                return Err(ValidationError::new(format!(
                    "texture-LOD sweep {sweep_id:?} assigns one output marker to different expected selections"
                )));
            }
        }
        if let Some(marker) = selection_to_marker.insert(controls.selection, controls.marker) {
            if marker != controls.marker {
                return Err(ValidationError::new(format!(
                    "texture-LOD sweep {sweep_id:?} assigns different output markers to one expected selection"
                )));
            }
        }
    }
    for observation in points.values_mut() {
        observation.observed_selection = marker_to_selection
            .get(&observation.framebuffer_rgba32_be)
            .copied();
    }

    let modes_order = [
        TextureLodMode::Mip,
        TextureLodMode::Detail,
        TextureLodMode::Sharpen,
    ];
    let mut modes = Vec::with_capacity(3);
    for lod_mode in modes_order {
        let mut cycles = Vec::with_capacity(2);
        for cycle_type in [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle] {
            let observations = positions
                .into_iter()
                .map(|position| {
                    points
                        .get(&(lod_mode, cycle_type, position))
                        .copied()
                        .ok_or_else(|| {
                            ValidationError::new(format!(
                                "texture-LOD sweep {sweep_id:?} is missing {lod_mode:?} {cycle_type:?} {position:?} point"
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            cycles.push(TextureLodCycleAnalysis {
                cycle_type,
                observations,
            });
        }
        let cycle_results_match = cycles[0]
            .observations
            .iter()
            .zip(&cycles[1].observations)
            .all(|(one, two)| {
                one.framebuffer_rgba32_be == two.framebuffer_rgba32_be
                    && one.depth_u16_be == two.depth_u16_be
                    && one.stored_coverage_u3 == two.stored_coverage_u3
            });
        modes.push(TextureLodModeAnalysis {
            lod_mode,
            cycles,
            cycle_results_match,
        });
    }
    let unexpected_output_count = modes
        .iter()
        .flat_map(|mode| &mode.cycles)
        .flat_map(|cycle| &cycle.observations)
        .filter(|observation| !observation.output_matches_producer_expectation)
        .count() as u8;
    let unexpected_depth_count = modes
        .iter()
        .flat_map(|mode| &mode.cycles)
        .flat_map(|cycle| &cycle.observations)
        .filter(|observation| !observation.depth_matches_producer_control)
        .count() as u8;
    let unexpected_coverage_count = modes
        .iter()
        .flat_map(|mode| &mode.cycles)
        .flat_map(|cycle| &cycle.observations)
        .filter(|observation| !observation.coverage_matches_producer_control)
        .count() as u8;
    let all_cycle_results_match = modes.iter().all(|mode| mode.cycle_results_match);

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        lod_boundary_numerator_i64: i64,
        lod_metric_denominator_u64: u64,
        primitive_tile_u3: u8,
        max_mip_level_u3: u8,
        min_lod_u8: u8,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        geometry: TextureLodGeometry,
        modes: &'a [TextureLodModeAnalysis],
        unexpected_output_count: u8,
        unexpected_depth_count: u8,
        unexpected_coverage_count: u8,
        all_cycle_results_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-texture-lod-boundary-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        lod_boundary_numerator_i64: common_controls.lod_boundary_numerator_i64,
        lod_metric_denominator_u64: common_controls.lod_metric_denominator_u64,
        primitive_tile_u3: common_controls.primitive_tile_u3,
        max_mip_level_u3: common_controls.max_mip_level_u3,
        min_lod_u8: common_controls.min_lod_u8,
        depth_control_u16_be: common_controls.depth_control_u16_be,
        stored_coverage_control_u3: common_controls.stored_coverage_control_u3,
        geometry: common_controls.geometry,
        modes: &modes,
        unexpected_output_count,
        unexpected_depth_count,
        unexpected_coverage_count,
        all_cycle_results_match,
    };
    let canonical = serde_json::to_vec(&hash_input)
        .map_err(|error| ValidationError::new(format!("hash texture-LOD analysis: {error}")))?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-texture-lod-boundary-analysis.v1\0");
    hasher.update(canonical);

    Ok(TextureLodBoundaryAnalysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256: hex(&hasher.finalize()),
        sweep_id: sweep_id.to_owned(),
        lod_boundary_numerator_i64: common_controls.lod_boundary_numerator_i64,
        lod_metric_denominator_u64: common_controls.lod_metric_denominator_u64,
        primitive_tile_u3: common_controls.primitive_tile_u3,
        max_mip_level_u3: common_controls.max_mip_level_u3,
        min_lod_u8: common_controls.min_lod_u8,
        depth_control_u16_be: common_controls.depth_control_u16_be,
        stored_coverage_control_u3: common_controls.stored_coverage_control_u3,
        geometry: common_controls.geometry,
        modes,
        unexpected_output_count,
        unexpected_depth_count,
        unexpected_coverage_count,
        all_cycle_results_match,
    })
}

/// Validate and preserve the complete producer-declared blender precision and
/// adjacent-pixel memory-feedback matrix. Numeric inputs and candidate markers
/// remain declarations: this analyzer does not derive a silicon division,
/// rounding, bypass, or memory-timing formula.
pub fn analyze_blender_precision_sweep(
    bundle: &ValidatedBundle,
    sweep_id: &str,
) -> Result<BlenderPrecisionAnalysis, ValidationError> {
    text("blender-precision sweep_id", sweep_id)?;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct PrecisionCommon {
        denominator_boundary_u6: u8,
        pixel_color_rgba32_be: u32,
        memory_color_rgba32_be: u32,
        fog_color_rgba32_be: u32,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        geometry: BlenderPrecisionGeometry,
        setup: Setup,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct PairCandidates {
        cycle_one_handoff_color_rgba32_be: u32,
        prior_memory_color_rgba32_be: u32,
        cycle_one_handoff_coverage_u3: u8,
        prior_memory_coverage_u3: u8,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct PairCommon {
        geometry: BlenderFeedbackGeometry,
        candidates: PairCandidates,
        setup: Setup,
    }

    let mut precision_common: Option<PrecisionCommon> = None;
    let mut pair_common: Option<PairCommon> = None;
    let mut denominators = BTreeMap::<(u8, BlenderDenominatorPosition), u8>::new();
    let mut expected_markers =
        BTreeMap::<(BlenderProbeMode, u8, BlenderDenominatorPosition), u32>::new();
    let mut precision_points = BTreeMap::<
        (
            BlenderProbeMode,
            ProbeCycleType,
            u8,
            BlenderDenominatorPosition,
        ),
        BlenderPrecisionObservation,
    >::new();
    let mut feedback_pairs = BTreeMap::<BlenderProbeMode, BlenderMemoryFeedbackObservation>::new();
    let mut matching_precision = 0usize;
    let mut matching_pairs = 0usize;

    for case in &bundle.bundle.cases {
        match &case.capture_intent {
            Some(CaptureIntent::BlenderPrecisionBoundarySweep {
                sweep_id: case_sweep_id,
                cycle_type,
                mode,
                isolated_alpha_u5,
                denominator_position,
                replay_from_reset,
                sample_x,
                sample_y,
                denominator_boundary_u6,
                producer_declared_denominator_u6,
                pixel_color_rgba32_be,
                memory_color_rgba32_be,
                fog_color_rgba32_be,
                producer_expected_framebuffer_rgba32_be,
                depth_control_u16_be,
                stored_coverage_control_u3,
            }) if case_sweep_id == sweep_id => {
                matching_precision += 1;
                if !replay_from_reset {
                    return Err(ValidationError::new(format!(
                        "case {:?}: blender-precision point must replay from reset",
                        case.case_id
                    )));
                }
                let framebuffer = &case.expected.framebuffer;
                let depth = &case.expected.depth;
                let coverage = &case.expected.coverage;
                if framebuffer.width != 1
                    || framebuffer.height != 1
                    || framebuffer.row_stride_bytes != 4
                    || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
                    || depth.width != 1
                    || depth.height != 1
                    || depth.row_stride_bytes != 2
                    || coverage.width != 1
                    || coverage.height != 1
                    || coverage.encoding != CoverageEncoding::StoredCoverageU3
                {
                    return Err(ValidationError::new(format!(
                        "case {:?}: blender-precision point requires exact 1x1 RGBA32 framebuffer, depth, and stored-coverage planes",
                        case.case_id
                    )));
                }
                let candidate_common = PrecisionCommon {
                    denominator_boundary_u6: *denominator_boundary_u6,
                    pixel_color_rgba32_be: *pixel_color_rgba32_be,
                    memory_color_rgba32_be: *memory_color_rgba32_be,
                    fog_color_rgba32_be: *fog_color_rgba32_be,
                    depth_control_u16_be: *depth_control_u16_be,
                    stored_coverage_control_u3: *stored_coverage_control_u3,
                    geometry: BlenderPrecisionGeometry {
                        framebuffer_address: framebuffer.address,
                        depth_address: depth.address,
                        coverage_address: coverage.color_image_address,
                        sample_x: *sample_x,
                        sample_y: *sample_y,
                    },
                    setup: case.setup.clone(),
                };
                if let Some(expected) = &precision_common {
                    if expected != &candidate_common {
                        return Err(ValidationError::new(format!(
                            "case {:?}: blender-precision boundary, colors, setup, controls, or geometry differs within sweep {:?}",
                            case.case_id, sweep_id
                        )));
                    }
                } else {
                    precision_common = Some(candidate_common);
                }

                let denominator_key = (*isolated_alpha_u5, *denominator_position);
                if let Some(expected) =
                    denominators.insert(denominator_key, *producer_declared_denominator_u6)
                {
                    if expected != *producer_declared_denominator_u6 {
                        return Err(ValidationError::new(format!(
                            "blender-precision sweep {sweep_id:?} changes the declared denominator for alpha {isolated_alpha_u5} {denominator_position:?}"
                        )));
                    }
                }
                let marker_key = (*mode, *isolated_alpha_u5, *denominator_position);
                if let Some(expected) =
                    expected_markers.insert(marker_key, *producer_expected_framebuffer_rgba32_be)
                {
                    if expected != *producer_expected_framebuffer_rgba32_be {
                        return Err(ValidationError::new(format!(
                            "blender-precision sweep {sweep_id:?} changes the producer marker across cycle modes for {mode:?} alpha {isolated_alpha_u5} {denominator_position:?}"
                        )));
                    }
                }

                let framebuffer_bytes =
                    decode_blob("blender-precision framebuffer", &framebuffer.contents)?;
                let depth_bytes = decode_blob("blender-precision depth", &depth.contents)?;
                let coverage_bytes =
                    decode_blob("blender-precision stored coverage", &coverage.contents)?;
                let observed_framebuffer = u32::from_be_bytes(
                    framebuffer_bytes
                        .try_into()
                        .expect("validated 1x1 RGBA32 framebuffer is four bytes"),
                );
                let observed_depth = u16::from_be_bytes(
                    depth_bytes
                        .try_into()
                        .expect("validated 1x1 depth plane is two bytes"),
                );
                let observed_coverage = coverage_bytes[0];
                let observation = BlenderPrecisionObservation {
                    isolated_alpha_u5: *isolated_alpha_u5,
                    denominator_position: *denominator_position,
                    producer_declared_denominator_u6: *producer_declared_denominator_u6,
                    producer_expected_framebuffer_rgba32_be:
                        *producer_expected_framebuffer_rgba32_be,
                    framebuffer_rgba32_be: observed_framebuffer,
                    output_matches_producer_expectation: observed_framebuffer
                        == *producer_expected_framebuffer_rgba32_be,
                    depth_u16_be: observed_depth,
                    depth_matches_producer_control: observed_depth == *depth_control_u16_be,
                    stored_coverage_u3: observed_coverage,
                    coverage_matches_producer_control: observed_coverage
                        == *stored_coverage_control_u3,
                };
                if precision_points
                    .insert(
                        (
                            *mode,
                            *cycle_type,
                            *isolated_alpha_u5,
                            *denominator_position,
                        ),
                        observation,
                    )
                    .is_some()
                {
                    return Err(ValidationError::new(format!(
                        "duplicate blender-precision point for {mode:?} {cycle_type:?} alpha {isolated_alpha_u5} {denominator_position:?} in sweep {sweep_id:?}"
                    )));
                }
            }
            Some(CaptureIntent::BlenderMemoryFeedbackPair {
                sweep_id: case_sweep_id,
                mode,
                cycle_type,
                replay_from_reset,
                first_pixel_x,
                first_pixel_y,
                second_pixel_x,
                second_pixel_y,
                ordered_pair_command_sha256,
                cycle_one_handoff_color_rgba32_be,
                prior_memory_color_rgba32_be,
                cycle_one_handoff_coverage_u3,
                prior_memory_coverage_u3,
            }) if case_sweep_id == sweep_id => {
                matching_pairs += 1;
                if !replay_from_reset {
                    return Err(ValidationError::new(format!(
                        "case {:?}: blender-feedback pair must start from reset",
                        case.case_id
                    )));
                }
                let framebuffer = &case.expected.framebuffer;
                let depth = &case.expected.depth;
                let coverage = &case.expected.coverage;
                if framebuffer.width != 2
                    || framebuffer.height != 1
                    || framebuffer.row_stride_bytes != 8
                    || framebuffer.encoding != FramebufferEncoding::Rgba32BigEndian
                    || depth.width != 2
                    || depth.height != 1
                    || depth.row_stride_bytes != 4
                    || coverage.width != 2
                    || coverage.height != 1
                    || coverage.encoding != CoverageEncoding::StoredCoverageU3
                {
                    return Err(ValidationError::new(format!(
                        "case {:?}: blender-feedback pair requires exact 2x1 RGBA32 framebuffer, depth, and stored-coverage planes",
                        case.case_id
                    )));
                }
                let candidates = PairCandidates {
                    cycle_one_handoff_color_rgba32_be: *cycle_one_handoff_color_rgba32_be,
                    prior_memory_color_rgba32_be: *prior_memory_color_rgba32_be,
                    cycle_one_handoff_coverage_u3: *cycle_one_handoff_coverage_u3,
                    prior_memory_coverage_u3: *prior_memory_coverage_u3,
                };
                let candidate_common = PairCommon {
                    geometry: BlenderFeedbackGeometry {
                        framebuffer_address: framebuffer.address,
                        depth_address: depth.address,
                        coverage_address: coverage.color_image_address,
                        first_pixel_x: *first_pixel_x,
                        first_pixel_y: *first_pixel_y,
                        second_pixel_x: *second_pixel_x,
                        second_pixel_y: *second_pixel_y,
                    },
                    candidates,
                    setup: case.setup.clone(),
                };
                if let Some(expected) = &pair_common {
                    if expected != &candidate_common {
                        return Err(ValidationError::new(format!(
                            "case {:?}: blender-feedback setup, candidate markers, or ordered-pair geometry differs within sweep {:?}",
                            case.case_id, sweep_id
                        )));
                    }
                } else {
                    pair_common = Some(candidate_common);
                }

                let framebuffer_bytes =
                    decode_blob("blender-feedback framebuffer", &framebuffer.contents)?;
                let depth_bytes = decode_blob("blender-feedback depth", &depth.contents)?;
                let coverage_bytes =
                    decode_blob("blender-feedback stored coverage", &coverage.contents)?;
                let framebuffer_rgba32_be = [
                    u32::from_be_bytes(framebuffer_bytes[0..4].try_into().unwrap()),
                    u32::from_be_bytes(framebuffer_bytes[4..8].try_into().unwrap()),
                ];
                let depth_u16_be = [
                    u16::from_be_bytes(depth_bytes[0..2].try_into().unwrap()),
                    u16::from_be_bytes(depth_bytes[2..4].try_into().unwrap()),
                ];
                let stored_coverage_u3 = [coverage_bytes[0], coverage_bytes[1]];
                let observation = BlenderMemoryFeedbackObservation {
                    mode: *mode,
                    cycle_type: *cycle_type,
                    ordered_pair_command_sha256: ordered_pair_command_sha256.clone(),
                    framebuffer_rgba32_be,
                    depth_u16_be,
                    stored_coverage_u3,
                    cycle_one_handoff_color_rgba32_be: candidates.cycle_one_handoff_color_rgba32_be,
                    prior_memory_color_rgba32_be: candidates.prior_memory_color_rgba32_be,
                    cycle_one_handoff_coverage_u3: candidates.cycle_one_handoff_coverage_u3,
                    prior_memory_coverage_u3: candidates.prior_memory_coverage_u3,
                    second_color_matches_cycle_one_handoff: framebuffer_rgba32_be[1]
                        == candidates.cycle_one_handoff_color_rgba32_be,
                    second_color_matches_prior_memory: framebuffer_rgba32_be[1]
                        == candidates.prior_memory_color_rgba32_be,
                    second_coverage_matches_cycle_one_handoff: stored_coverage_u3[1]
                        == candidates.cycle_one_handoff_coverage_u3,
                    second_coverage_matches_prior_memory: stored_coverage_u3[1]
                        == candidates.prior_memory_coverage_u3,
                };
                if feedback_pairs.insert(*mode, observation).is_some() {
                    return Err(ValidationError::new(format!(
                        "duplicate blender-feedback pair for {mode:?} in sweep {sweep_id:?}"
                    )));
                }
            }
            _ => {}
        }
    }

    if matching_precision == 0 && matching_pairs == 0 {
        return Err(ValidationError::new(format!(
            "bundle contains no blender-precision capture intent for sweep {sweep_id:?}"
        )));
    }
    let precision_common = precision_common.ok_or_else(|| {
        ValidationError::new(format!(
            "blender-precision sweep {sweep_id:?} contains no precision points"
        ))
    })?;
    let pair_common = pair_common.ok_or_else(|| {
        ValidationError::new(format!(
            "blender-precision sweep {sweep_id:?} contains no adjacent-pixel feedback pairs"
        ))
    })?;
    if matching_precision != 72 {
        return Err(ValidationError::new(format!(
            "blender-precision sweep {sweep_id:?} requires exactly 72 precision points, found {matching_precision}"
        )));
    }
    if matching_pairs != 3 {
        return Err(ValidationError::new(format!(
            "blender-precision sweep {sweep_id:?} requires exactly three mode-specific feedback pairs, found {matching_pairs}"
        )));
    }

    let modes_order = [
        BlenderProbeMode::Ordinary,
        BlenderProbeMode::ForceBlend,
        BlenderProbeMode::FogPass,
    ];
    let cycle_order = [ProbeCycleType::OneCycle, ProbeCycleType::TwoCycle];
    let alpha_values_u5 = [0, 1, 30, 31];
    let position_order = [
        BlenderDenominatorPosition::Below,
        BlenderDenominatorPosition::On,
        BlenderDenominatorPosition::Above,
    ];
    let mut modes = Vec::with_capacity(3);
    for mode in modes_order {
        let mut cycles = Vec::with_capacity(2);
        for cycle_type in cycle_order {
            let mut observations = Vec::with_capacity(12);
            for alpha in alpha_values_u5 {
                for position in position_order {
                    observations.push(
                        precision_points
                            .get(&(mode, cycle_type, alpha, position))
                            .copied()
                            .ok_or_else(|| {
                                ValidationError::new(format!(
                                    "blender-precision sweep {sweep_id:?} is missing {mode:?} {cycle_type:?} alpha {alpha} {position:?} point"
                                ))
                            })?,
                    );
                }
            }
            cycles.push(BlenderPrecisionCycleAnalysis {
                cycle_type,
                observations,
            });
        }
        let cycle_divergence_count = cycles[0]
            .observations
            .iter()
            .zip(&cycles[1].observations)
            .filter(|(one, two)| {
                one.framebuffer_rgba32_be != two.framebuffer_rgba32_be
                    || one.depth_u16_be != two.depth_u16_be
                    || one.stored_coverage_u3 != two.stored_coverage_u3
            })
            .count() as u8;
        modes.push(BlenderPrecisionModeAnalysis {
            mode,
            cycles,
            cycle_divergence_count,
            cycle_results_match: cycle_divergence_count == 0,
        });
    }
    let feedback_pairs = modes_order
        .into_iter()
        .map(|mode| {
            feedback_pairs.get(&mode).cloned().ok_or_else(|| {
                ValidationError::new(format!(
                    "blender-precision sweep {sweep_id:?} is missing {mode:?} feedback pair"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let observations = modes
        .iter()
        .flat_map(|mode| &mode.cycles)
        .flat_map(|cycle| &cycle.observations)
        .collect::<Vec<_>>();
    let unexpected_output_count = observations
        .iter()
        .filter(|observation| !observation.output_matches_producer_expectation)
        .count() as u8;
    let unexpected_depth_count = observations
        .iter()
        .filter(|observation| !observation.depth_matches_producer_control)
        .count() as u8;
    let unexpected_coverage_count = observations
        .iter()
        .filter(|observation| !observation.coverage_matches_producer_control)
        .count() as u8;
    let total_cycle_divergence_count = modes
        .iter()
        .map(|mode| u16::from(mode.cycle_divergence_count))
        .sum::<u16>() as u8;
    let all_cycle_results_match = total_cycle_divergence_count == 0;

    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        producer_kind: &'a ProducerKind,
        base_matrix_row_closed: bool,
        alpha_values_u5: [u8; 4],
        denominator_boundary_u6: u8,
        pixel_color_rgba32_be: u32,
        memory_color_rgba32_be: u32,
        fog_color_rgba32_be: u32,
        depth_control_u16_be: u16,
        stored_coverage_control_u3: u8,
        precision_geometry: BlenderPrecisionGeometry,
        feedback_geometry: BlenderFeedbackGeometry,
        modes: &'a [BlenderPrecisionModeAnalysis],
        feedback_pairs: &'a [BlenderMemoryFeedbackObservation],
        unexpected_output_count: u8,
        unexpected_depth_count: u8,
        unexpected_coverage_count: u8,
        total_cycle_divergence_count: u8,
        all_cycle_results_match: bool,
    }
    const ANALYSIS_SCHEMA: &str = "fn64.rdp-blender-precision-analysis.v1";
    let hash_input = HashInput {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: &bundle.canonical_sha256,
        sweep_id,
        producer_kind: &bundle.bundle.producer.kind,
        base_matrix_row_closed: false,
        alpha_values_u5,
        denominator_boundary_u6: precision_common.denominator_boundary_u6,
        pixel_color_rgba32_be: precision_common.pixel_color_rgba32_be,
        memory_color_rgba32_be: precision_common.memory_color_rgba32_be,
        fog_color_rgba32_be: precision_common.fog_color_rgba32_be,
        depth_control_u16_be: precision_common.depth_control_u16_be,
        stored_coverage_control_u3: precision_common.stored_coverage_control_u3,
        precision_geometry: precision_common.geometry,
        feedback_geometry: pair_common.geometry,
        modes: &modes,
        feedback_pairs: &feedback_pairs,
        unexpected_output_count,
        unexpected_depth_count,
        unexpected_coverage_count,
        total_cycle_divergence_count,
        all_cycle_results_match,
    };
    let canonical = serde_json::to_vec(&hash_input)
        .map_err(|error| ValidationError::new(format!("hash blender analysis: {error}")))?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.rdp-blender-precision-analysis.v1\0");
    hasher.update(canonical);

    Ok(BlenderPrecisionAnalysis {
        schema: ANALYSIS_SCHEMA,
        bundle_sha256: bundle.canonical_sha256.clone(),
        analysis_sha256: hex(&hasher.finalize()),
        sweep_id: sweep_id.to_owned(),
        producer_kind: bundle.bundle.producer.kind.clone(),
        base_matrix_row_closed: false,
        alpha_values_u5,
        denominator_boundary_u6: precision_common.denominator_boundary_u6,
        pixel_color_rgba32_be: precision_common.pixel_color_rgba32_be,
        memory_color_rgba32_be: precision_common.memory_color_rgba32_be,
        fog_color_rgba32_be: precision_common.fog_color_rgba32_be,
        depth_control_u16_be: precision_common.depth_control_u16_be,
        stored_coverage_control_u3: precision_common.stored_coverage_control_u3,
        precision_geometry: precision_common.geometry,
        feedback_geometry: pair_common.geometry,
        modes,
        feedback_pairs,
        unexpected_output_count,
        unexpected_depth_count,
        unexpected_coverage_count,
        total_cycle_divergence_count,
        all_cycle_results_match,
    })
}

fn validate_framebuffer(plane: &FramebufferPlane) -> Result<(), ValidationError> {
    let min_stride = plane
        .width
        .checked_mul(plane.encoding.bytes_per_pixel())
        .ok_or_else(|| ValidationError::new("framebuffer row size overflows"))?;
    validate_plane(
        "framebuffer",
        plane.address,
        plane.width,
        plane.height,
        plane.row_stride_bytes,
        min_stride,
        &plane.contents,
    )
}

fn validate_depth(plane: &DepthPlane) -> Result<(), ValidationError> {
    let min_stride = plane
        .width
        .checked_mul(2)
        .ok_or_else(|| ValidationError::new("depth row size overflows"))?;
    validate_plane(
        "depth",
        plane.address,
        plane.width,
        plane.height,
        plane.row_stride_bytes,
        min_stride,
        &plane.contents,
    )
}

fn validate_coverage(plane: &CoveragePlane) -> Result<(), ValidationError> {
    let bytes = decode_blob("coverage contents", &plane.contents)?;
    let pixels = plane
        .width
        .checked_mul(plane.height)
        .ok_or_else(|| ValidationError::new("coverage dimensions overflow"))?;
    if plane.width == 0 || plane.height == 0 || u64::from(pixels) != plane.contents.byte_len {
        return Err(ValidationError::new(
            "coverage must contain exactly one normalized byte per pixel",
        ));
    }
    physical_range(plane.color_image_address, 1, "coverage color image")?;
    let maximum = match plane.encoding {
        CoverageEncoding::Rgba16HiddenBitsU2 => 3,
        CoverageEncoding::StoredCoverageU3 => 7,
        CoverageEncoding::CoverageCountU4 => 8,
    };
    if bytes.iter().any(|&value| value > maximum) {
        return Err(ValidationError::new(format!(
            "coverage value exceeds encoding maximum {maximum}"
        )));
    }
    Ok(())
}

fn validate_plane(
    label: &str,
    address: u32,
    width: u32,
    height: u32,
    stride: u32,
    min_stride: u32,
    contents: &Blob,
) -> Result<(), ValidationError> {
    if width == 0 || height == 0 || stride < min_stride {
        return Err(ValidationError::new(format!(
            "{label} has zero dimensions or an undersized row stride"
        )));
    }
    let expected = u64::from(stride)
        .checked_mul(u64::from(height))
        .ok_or_else(|| ValidationError::new(format!("{label} byte length overflows")))?;
    if expected != contents.byte_len {
        return Err(ValidationError::new(format!(
            "{label} geometry does not equal contents.byte_len"
        )));
    }
    physical_range(address, expected, label)?;
    decode_blob(&format!("{label} contents"), contents)?;
    Ok(())
}

fn register(case: &VectorCase, name: RegisterName) -> u32 {
    case.setup
        .registers
        .iter()
        .find(|register| register.name == name)
        .expect("required register checked above")
        .value
}

fn physical_range(address: u32, byte_len: u64, label: &str) -> Result<(u32, u32), ValidationError> {
    if byte_len == 0 {
        return Err(ValidationError::new(format!("{label} must not be empty")));
    }
    let length = u32::try_from(byte_len)
        .map_err(|_| ValidationError::new(format!("{label} is too large")))?;
    let end = address
        .checked_add(length)
        .ok_or_else(|| ValidationError::new(format!("{label} address overflows")))?;
    if end > RDRAM_END {
        return Err(ValidationError::new(format!(
            "{label} lies outside eight MiB physical RDRAM"
        )));
    }
    Ok((address, end))
}

fn decode_blob(label: &str, blob: &Blob) -> Result<Vec<u8>, ValidationError> {
    if blob.byte_len > MAX_BLOB_BYTES as u64 {
        return Err(ValidationError::new(format!(
            "{label} exceeds {MAX_BLOB_BYTES} bytes"
        )));
    }
    sha256(&format!("{label}.sha256"), &blob.sha256)?;
    let expected_hex_len = usize::try_from(blob.byte_len)
        .ok()
        .and_then(|length| length.checked_mul(2))
        .ok_or_else(|| ValidationError::new(format!("{label} length overflows")))?;
    if blob.bytes_hex.len() != expected_hex_len
        || !blob
            .bytes_hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ValidationError::new(format!(
            "{label}.bytes_hex must be exact-length lowercase hexadecimal"
        )));
    }
    let mut bytes = Vec::with_capacity(expected_hex_len / 2);
    for pair in blob.bytes_hex.as_bytes().chunks_exact(2) {
        bytes.push((nibble(pair[0]) << 4) | nibble(pair[1]));
    }
    let digest = hex(&Sha256::digest(&bytes));
    if digest != blob.sha256 {
        return Err(ValidationError::new(format!(
            "{label} SHA-256 mismatch: declared {}, calculated {digest}",
            blob.sha256
        )));
    }
    Ok(bytes)
}

fn text(label: &str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ValidationError::new(format!(
            "{label} must be nonempty canonical text"
        )));
    }
    Ok(())
}

fn sha256(label: &str, value: &str) -> Result<(), ValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ValidationError::new(format!(
            "{label} must contain exactly 64 lowercase hexadecimal digits"
        )));
    }
    Ok(())
}

fn nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("validated lowercase hexadecimal"),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}
