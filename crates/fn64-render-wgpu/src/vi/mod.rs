//! Bounded M3.3d VI and capture mechanism.
//!
//! This module deliberately accepts one exact, complete presentation image:
//! the synthetic M3.3a 4x2 RGBA16 target, progressive field, 1:1 scale, and
//! VI replicate mode with every optional filter disabled. It owns neither the
//! live VI register latch nor guest memory. The future crate-root integration
//! must translate one `fn64_render::ViPresentation` into this value while the
//! upstream retrace-scoped memory capability is live.
//!
//! The CPU oracle and padded-row extractor are executable without a GPU. The
//! repository-owned WGSL is retained beside this module, but this isolated
//! slice does not wire or qualify a wgpu pipeline.
//!
//! Provenance: the complete register names and `OSViMode` field split come
//! from the public libultra VI interface and *N64 Programming Manual*, Video
//! Interface chapter. The active pixel/half-line window and U2.10 coordinate
//! fields follow US 6,166,748 Figures 35M/35N. RGBA5551 device-byte packing
//! follows the public *N64 Programming Manual* section 15.5 memory-interface
//! description and the already-reviewed M3.3a device-byte contract. The
//! 256-byte capture pitch is wgpu 30's buffer-copy-row alignment mechanism;
//! it is not an N64 or RT64 behavior claim.

#![allow(dead_code)]

use core::fmt;

use crate::native_contract::DeviceRgba16Bytes;

pub const SOURCE_ORIGIN: u32 = 0x400;
pub const SOURCE_STRIDE_PIXELS: u32 = 4;
pub const OUTPUT_WIDTH: u32 = 4;
pub const OUTPUT_HEIGHT: u32 = 2;
pub const SOURCE_BYTES_PER_PIXEL: u32 = 2;
pub const OUTPUT_BYTES_PER_PIXEL: u32 = 4;
pub const SOURCE_BYTE_LEN: usize = 16;
pub const TIGHT_CAPTURE_ROW_BYTES: u32 = 16;
pub const PADDED_CAPTURE_ROW_BYTES: u32 = 256;
pub const PADDED_CAPTURE_BYTE_LEN: usize = 512;

pub const PIXEL_TYPE_RGBA16: u32 = 2;
pub const AA_MODE_REPLICATE: u32 = 3 << 8;
pub const STATUS_RGBA16_REPLICATE: u32 = PIXEL_TYPE_RGBA16 | AA_MODE_REPLICATE;
pub const SCALE_ONE_U2_10: u32 = 1 << 10;

pub const REGISTER_NAMES: [&str; 14] = [
    "VI_STATUS",
    "VI_ORIGIN",
    "VI_WIDTH",
    "VI_INTR",
    "VI_CURRENT",
    "VI_BURST",
    "VI_V_SYNC",
    "VI_H_SYNC",
    "VI_LEAP",
    "VI_H_START",
    "VI_V_START",
    "VI_V_BURST",
    "VI_X_SCALE",
    "VI_Y_SCALE",
];

/// Exact fourteen-word register authority expected from `ViPresentation`.
///
/// H start/end are `0..4` pixels. V start/end are `0..4` half-lines, which
/// names two progressive output rows. Timing words are zero only because this
/// is a mechanism fixture; accepting another value belongs to a broader VI
/// slice, not to a permissive fallback here.
pub const EXACT_REGISTER_WORDS: [u32; 14] = [
    STATUS_RGBA16_REPLICATE,
    SOURCE_ORIGIN,
    SOURCE_STRIDE_PIXELS,
    0,
    0,
    0,
    0,
    0,
    0,
    OUTPUT_WIDTH,
    OUTPUT_HEIGHT * 2,
    0,
    SCALE_ONE_U2_10,
    SCALE_ONE_U2_10,
];

pub const EXACT_WORKLOAD_IDENTITY: [u8; 32] = [
    0x3d, 0x07, 0x99, 0x07, 0xc2, 0x00, 0x80, 0xa2, 0x77, 0xcc, 0xee, 0x13, 0x44, 0xe6, 0xaf, 0x93,
    0x32, 0x82, 0x8b, 0x3c, 0x52, 0x0d, 0xd1, 0x2d, 0x31, 0xf5, 0x02, 0xbb, 0xf8, 0xd6, 0x3c, 0x2c,
];

pub const EXACT_POST_VI_BGRA8: [u8; 32] = [
    0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0,
    0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff,
];

/// Repository-owned compute mechanism retained for the later GPU owner.
pub const REPLICATE_RGBA16_WGSL: &str = include_str!("replicate_rgba16.wgsl");

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViRegisterImage {
    pub words: [u32; 14],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViManagerControls {
    pub blanked: bool,
    pub fade_u10: Option<u16>,
    pub repeat_line: bool,
    pub noise_seed: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PresentationIdentity {
    pub workload_sha256: [u8; 32],
    pub native_target_origin: u32,
    pub native_target_generation: u64,
    pub retrace_cycle: u64,
    pub present_ordinal: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct M3dPresentationSpec {
    pub registers: ViRegisterImage,
    pub controls: ViManagerControls,
    pub identity: PresentationIdentity,
}

impl M3dPresentationSpec {
    pub const fn exact_fixture() -> Self {
        Self {
            registers: ViRegisterImage {
                words: EXACT_REGISTER_WORDS,
            },
            controls: ViManagerControls {
                blanked: false,
                fade_u10: None,
                repeat_line: false,
                noise_seed: 0,
            },
            identity: PresentationIdentity {
                workload_sha256: EXACT_WORKLOAD_IDENTITY,
                native_target_origin: SOURCE_ORIGIN,
                native_target_generation: 1,
                retrace_cycle: 0,
                present_ordinal: 1,
            },
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViPixelType {
    Rgba16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViFilterMode {
    ReplicateNoOptionalFilters,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViFieldIdentity {
    Progressive,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViExtent {
    pub width: u32,
    pub height: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViScale {
    pub x_step_u2_10: u16,
    pub x_offset_u2_10: u16,
    pub y_step_u2_10: u16,
    pub y_offset_u2_10: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViReadPlan {
    identity: PresentationIdentity,
    origin: u32,
    stride_pixels: u32,
    extent: ViExtent,
    pixel_type: ViPixelType,
    row_bytes: u32,
    byte_len: usize,
}

impl ViReadPlan {
    pub const fn identity(self) -> PresentationIdentity {
        self.identity
    }

    pub const fn origin(self) -> u32 {
        self.origin
    }

    pub const fn stride_pixels(self) -> u32 {
        self.stride_pixels
    }

    pub const fn extent(self) -> ViExtent {
        self.extent
    }

    pub const fn pixel_type(self) -> ViPixelType {
        self.pixel_type
    }

    pub const fn row_bytes(self) -> u32 {
        self.row_bytes
    }

    pub const fn byte_len(self) -> usize {
        self.byte_len
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViOutputPlan {
    identity: PresentationIdentity,
    extent: ViExtent,
    format: CaptureFormat,
    tight_row_bytes: u32,
    byte_len: usize,
}

impl ViOutputPlan {
    pub const fn identity(self) -> PresentationIdentity {
        self.identity
    }

    pub const fn extent(self) -> ViExtent {
        self.extent
    }

    pub const fn format(self) -> CaptureFormat {
        self.format
    }

    pub const fn tight_row_bytes(self) -> u32 {
        self.tight_row_bytes
    }

    pub const fn byte_len(self) -> usize {
        self.byte_len
    }
}

/// Resources used by VI conversion only. Capture staging is not hidden here.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViResourcePlan {
    source: ViReadPlan,
    output: ViOutputPlan,
}

impl ViResourcePlan {
    pub const fn source(self) -> ViReadPlan {
        self.source
    }

    pub const fn output(self) -> ViOutputPlan {
        self.output
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CaptureFormat {
    Bgra8Unorm,
    Rgba8Unorm,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CaptureIdentity {
    pub presentation: PresentationIdentity,
    pub capture_ordinal: u64,
}

/// Copy/readback resources are a separate plan so ordinary presentation can
/// never acquire capture work implicitly.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CaptureResourcePlan {
    identity: CaptureIdentity,
    source: ViOutputPlan,
    padded_row_bytes: u32,
    padded_byte_len: usize,
}

impl CaptureResourcePlan {
    pub const fn identity(self) -> CaptureIdentity {
        self.identity
    }

    pub const fn source(self) -> ViOutputPlan {
        self.source
    }

    pub const fn padded_row_bytes(self) -> u32 {
        self.padded_row_bytes
    }

    pub const fn padded_byte_len(self) -> usize {
        self.padded_byte_len
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct M3dResourcePlans {
    vi: ViResourcePlan,
    capture: CaptureResourcePlan,
}

impl M3dResourcePlans {
    pub const fn vi(self) -> ViResourcePlan {
        self.vi
    }

    pub const fn capture(self) -> CaptureResourcePlan {
        self.capture
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ValidatedM3dPresentation {
    identity: PresentationIdentity,
    origin: u32,
    stride_pixels: u32,
    extent: ViExtent,
    pixel_type: ViPixelType,
    scale: ViScale,
    filter: ViFilterMode,
    field: ViFieldIdentity,
    plans: M3dResourcePlans,
}

impl ValidatedM3dPresentation {
    pub const fn identity(self) -> PresentationIdentity {
        self.identity
    }

    pub const fn origin(self) -> u32 {
        self.origin
    }

    pub const fn stride_pixels(self) -> u32 {
        self.stride_pixels
    }

    pub const fn extent(self) -> ViExtent {
        self.extent
    }

    pub const fn pixel_type(self) -> ViPixelType {
        self.pixel_type
    }

    pub const fn scale(self) -> ViScale {
        self.scale
    }

    pub const fn filter(self) -> ViFilterMode {
        self.filter
    }

    pub const fn field(self) -> ViFieldIdentity {
        self.field
    }

    pub const fn plans(self) -> M3dResourcePlans {
        self.plans
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViValidationError {
    RegisterMismatch {
        register: &'static str,
        expected: u32,
        actual: u32,
    },
    ControlMismatch {
        control: &'static str,
    },
    IdentityMismatch {
        field: &'static str,
    },
    ResourceArithmeticOverflow {
        field: &'static str,
    },
}

impl fmt::Display for ViValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegisterMismatch {
                register,
                expected,
                actual,
            } => write!(
                formatter,
                "M3.3d {register} mismatch: expected {expected:#010x}, got {actual:#010x}"
            ),
            Self::ControlMismatch { control } => {
                write!(formatter, "M3.3d unsupported VI control: {control}")
            }
            Self::IdentityMismatch { field } => {
                write!(formatter, "M3.3d presentation identity mismatch: {field}")
            }
            Self::ResourceArithmeticOverflow { field } => {
                write!(formatter, "M3.3d resource arithmetic overflow: {field}")
            }
        }
    }
}

impl std::error::Error for ViValidationError {}

pub fn validate_exact_presentation(
    spec: M3dPresentationSpec,
) -> Result<ValidatedM3dPresentation, ViValidationError> {
    for (index, (&actual, &expected)) in spec
        .registers
        .words
        .iter()
        .zip(EXACT_REGISTER_WORDS.iter())
        .enumerate()
    {
        if actual != expected {
            return Err(ViValidationError::RegisterMismatch {
                register: REGISTER_NAMES[index],
                expected,
                actual,
            });
        }
    }

    for (mismatch, control) in [
        (spec.controls.blanked, "blanking must be disabled"),
        (spec.controls.fade_u10.is_some(), "fade must be disabled"),
        (spec.controls.repeat_line, "repeat-line must be disabled"),
        (spec.controls.noise_seed != 0, "noise seed must be zero"),
    ] {
        if mismatch {
            return Err(ViValidationError::ControlMismatch { control });
        }
    }

    let expected_identity = M3dPresentationSpec::exact_fixture().identity;
    for (mismatch, field) in [
        (
            spec.identity.workload_sha256 != expected_identity.workload_sha256,
            "workload_sha256",
        ),
        (
            spec.identity.native_target_origin != expected_identity.native_target_origin,
            "native_target_origin",
        ),
        (
            spec.identity.native_target_generation != expected_identity.native_target_generation,
            "native_target_generation",
        ),
        (
            spec.identity.retrace_cycle != expected_identity.retrace_cycle,
            "retrace_cycle",
        ),
        (
            spec.identity.present_ordinal != expected_identity.present_ordinal,
            "present_ordinal",
        ),
    ] {
        if mismatch {
            return Err(ViValidationError::IdentityMismatch { field });
        }
    }

    let source_row_bytes = SOURCE_STRIDE_PIXELS
        .checked_mul(SOURCE_BYTES_PER_PIXEL)
        .ok_or(ViValidationError::ResourceArithmeticOverflow {
            field: "source_row_bytes",
        })?;
    let source_byte_len = usize::try_from(source_row_bytes)
        .ok()
        .and_then(|row| row.checked_mul(OUTPUT_HEIGHT as usize))
        .ok_or(ViValidationError::ResourceArithmeticOverflow {
            field: "source_byte_len",
        })?;
    let tight_row_bytes = OUTPUT_WIDTH.checked_mul(OUTPUT_BYTES_PER_PIXEL).ok_or(
        ViValidationError::ResourceArithmeticOverflow {
            field: "tight_capture_row_bytes",
        },
    )?;
    let output_byte_len = usize::try_from(tight_row_bytes)
        .ok()
        .and_then(|row| row.checked_mul(OUTPUT_HEIGHT as usize))
        .ok_or(ViValidationError::ResourceArithmeticOverflow {
            field: "output_byte_len",
        })?;
    let padded_byte_len = usize::try_from(PADDED_CAPTURE_ROW_BYTES)
        .ok()
        .and_then(|row| row.checked_mul(OUTPUT_HEIGHT as usize))
        .ok_or(ViValidationError::ResourceArithmeticOverflow {
            field: "padded_capture_byte_len",
        })?;

    let extent = ViExtent {
        width: OUTPUT_WIDTH,
        height: OUTPUT_HEIGHT,
    };
    let output = ViOutputPlan {
        identity: spec.identity,
        extent,
        format: CaptureFormat::Bgra8Unorm,
        tight_row_bytes,
        byte_len: output_byte_len,
    };
    let plans = M3dResourcePlans {
        vi: ViResourcePlan {
            source: ViReadPlan {
                identity: spec.identity,
                origin: SOURCE_ORIGIN,
                stride_pixels: SOURCE_STRIDE_PIXELS,
                extent,
                pixel_type: ViPixelType::Rgba16,
                row_bytes: source_row_bytes,
                byte_len: source_byte_len,
            },
            output,
        },
        capture: CaptureResourcePlan {
            identity: CaptureIdentity {
                presentation: spec.identity,
                capture_ordinal: 1,
            },
            source: output,
            padded_row_bytes: PADDED_CAPTURE_ROW_BYTES,
            padded_byte_len,
        },
    };

    Ok(ValidatedM3dPresentation {
        identity: spec.identity,
        origin: SOURCE_ORIGIN,
        stride_pixels: SOURCE_STRIDE_PIXELS,
        extent,
        pixel_type: ViPixelType::Rgba16,
        scale: ViScale {
            x_step_u2_10: SCALE_ONE_U2_10 as u16,
            x_offset_u2_10: 0,
            y_step_u2_10: SCALE_ONE_U2_10 as u16,
            y_offset_u2_10: 0,
        },
        filter: ViFilterMode::ReplicateNoOptionalFilters,
        field: ViFieldIdentity::Progressive,
        plans,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViExecutionError {
    SourceLengthMismatch { expected: usize, actual: usize },
}

impl fmt::Display for ViExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceLengthMismatch { expected, actual } => write!(
                formatter,
                "M3.3d RGBA16 source has {actual} bytes; exact plan requires {expected}"
            ),
        }
    }
}

impl std::error::Error for ViExecutionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuViOutput {
    identity: PresentationIdentity,
    bgra8: Box<[u8]>,
}

impl CpuViOutput {
    pub const fn identity(&self) -> PresentationIdentity {
        self.identity
    }

    pub fn bgra8(&self) -> &[u8] {
        &self.bgra8
    }
}

const fn expand_five_to_eight(value: u8) -> u8 {
    (value << 3) | (value >> 2)
}

/// Execute the exact progressive/replicate conversion as a CPU oracle.
pub fn execute_cpu_oracle(
    presentation: ValidatedM3dPresentation,
    source_device_rgba16: &DeviceRgba16Bytes,
) -> Result<CpuViOutput, ViExecutionError> {
    let source_device_rgba16 = source_device_rgba16.device_bytes();
    let expected = presentation.plans.vi.source.byte_len;
    if source_device_rgba16.len() != expected {
        return Err(ViExecutionError::SourceLengthMismatch {
            expected,
            actual: source_device_rgba16.len(),
        });
    }

    let mut output = Vec::with_capacity(presentation.plans.vi.output.byte_len);
    for pixel in source_device_rgba16.chunks_exact(2) {
        let rgba16 = u16::from_be_bytes([pixel[0], pixel[1]]);
        let red = expand_five_to_eight(((rgba16 >> 11) & 0x1f) as u8);
        let green = expand_five_to_eight(((rgba16 >> 6) & 0x1f) as u8);
        let blue = expand_five_to_eight(((rgba16 >> 1) & 0x1f) as u8);
        let alpha = if rgba16 & 1 == 0 { 0 } else { 0xff };
        output.extend_from_slice(&[blue, green, red, alpha]);
    }
    Ok(CpuViOutput {
        identity: presentation.identity,
        bgra8: output.into_boxed_slice(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaddedCapture {
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    Identity,
    Format,
    Extent,
    RowBytes { expected: u32, actual: u32 },
    ByteLength { expected: usize, actual: usize },
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity => formatter.write_str("M3.3d capture identity mismatch"),
            Self::Format => formatter.write_str("M3.3d capture format mismatch"),
            Self::Extent => formatter.write_str("M3.3d capture extent mismatch"),
            Self::RowBytes { expected, actual } => write!(
                formatter,
                "M3.3d capture row pitch is {actual}; exact plan requires {expected}"
            ),
            Self::ByteLength { expected, actual } => write!(
                formatter,
                "M3.3d padded capture has {actual} bytes; exact plan requires {expected}"
            ),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Strip wgpu's padded copy rows without admitting the padding as pixels.
pub fn extract_tightly_packed_bgra8(
    plan: CaptureResourcePlan,
    capture: &PaddedCapture,
) -> Result<Vec<u8>, CaptureError> {
    if capture.identity != plan.identity {
        return Err(CaptureError::Identity);
    }
    if capture.format != plan.source.format {
        return Err(CaptureError::Format);
    }
    if (capture.width, capture.height) != (plan.source.extent.width, plan.source.extent.height) {
        return Err(CaptureError::Extent);
    }
    if capture.row_bytes != plan.padded_row_bytes {
        return Err(CaptureError::RowBytes {
            expected: plan.padded_row_bytes,
            actual: capture.row_bytes,
        });
    }
    if capture.bytes.len() != plan.padded_byte_len {
        return Err(CaptureError::ByteLength {
            expected: plan.padded_byte_len,
            actual: capture.bytes.len(),
        });
    }

    let padded_row = plan.padded_row_bytes as usize;
    let tight_row = plan.source.tight_row_bytes as usize;
    let mut tight = Vec::with_capacity(plan.source.byte_len);
    for row in capture.bytes.chunks_exact(padded_row) {
        tight.extend_from_slice(&row[..tight_row]);
    }
    Ok(tight)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE_RGBA16_RED: [u8; SOURCE_BYTE_LEN] = [
        0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8,
        0x01,
    ];

    fn validated() -> ValidatedM3dPresentation {
        validate_exact_presentation(M3dPresentationSpec::exact_fixture()).unwrap()
    }

    fn device_rgba16(bytes: &[u8]) -> DeviceRgba16Bytes {
        DeviceRgba16Bytes::from_device_bytes(bytes.to_vec())
    }

    fn padded_capture(bytes: &[u8]) -> PaddedCapture {
        let plan = validated().plans().capture;
        let mut padded = vec![0xa5; plan.padded_byte_len];
        let tight_row = plan.source.tight_row_bytes as usize;
        for (row_index, row) in bytes.chunks_exact(tight_row).enumerate() {
            let start = row_index * plan.padded_row_bytes as usize;
            padded[start..start + tight_row].copy_from_slice(row);
        }
        PaddedCapture {
            identity: plan.identity,
            format: plan.source.format,
            width: plan.source.extent.width,
            height: plan.source.extent.height,
            row_bytes: plan.padded_row_bytes,
            bytes: padded,
        }
    }

    #[test]
    fn exact_fixture_retains_every_typed_vi_dimension() {
        let presentation = validated();
        assert_eq!(presentation.origin(), SOURCE_ORIGIN);
        assert_eq!(presentation.stride_pixels(), SOURCE_STRIDE_PIXELS);
        assert_eq!(
            presentation.extent(),
            ViExtent {
                width: OUTPUT_WIDTH,
                height: OUTPUT_HEIGHT
            }
        );
        assert_eq!(presentation.pixel_type(), ViPixelType::Rgba16);
        assert_eq!(
            presentation.scale(),
            ViScale {
                x_step_u2_10: 1024,
                x_offset_u2_10: 0,
                y_step_u2_10: 1024,
                y_offset_u2_10: 0,
            }
        );
        assert_eq!(
            presentation.filter(),
            ViFilterMode::ReplicateNoOptionalFilters
        );
        assert_eq!(presentation.field(), ViFieldIdentity::Progressive);
    }

    #[test]
    fn vi_and_capture_resources_are_separate_exact_plans() {
        let plans = validated().plans();
        assert_eq!(plans.vi.source.byte_len, SOURCE_BYTE_LEN);
        assert_eq!(plans.vi.source.row_bytes, 8);
        assert_eq!(plans.vi.output.byte_len, EXACT_POST_VI_BGRA8.len());
        assert_eq!(plans.vi.output.tight_row_bytes, TIGHT_CAPTURE_ROW_BYTES);
        assert_eq!(plans.capture.source, plans.vi.output);
        assert_eq!(plans.capture.padded_row_bytes, PADDED_CAPTURE_ROW_BYTES);
        assert_eq!(plans.capture.padded_byte_len, PADDED_CAPTURE_BYTE_LEN);
        assert_ne!(
            plans.capture.padded_byte_len, plans.vi.output.byte_len,
            "capture padding must not be collapsed into VI output storage"
        );
    }

    #[test]
    fn cpu_oracle_matches_the_frozen_post_vi_bgra8_fixture() {
        let source = device_rgba16(&DEVICE_RGBA16_RED);
        let output = execute_cpu_oracle(validated(), &source).unwrap();
        assert_eq!(
            output.identity(),
            M3dPresentationSpec::exact_fixture().identity
        );
        assert_eq!(output.bgra8(), EXACT_POST_VI_BGRA8);
    }

    #[test]
    fn cpu_oracle_preserves_channel_and_alpha_bit_identity() {
        let source = [
            0xf8, 0x01, // red, opaque
            0x07, 0xc1, // green, opaque
            0x00, 0x3f, // blue, opaque
            0xff, 0xff, // white, opaque
            0x00, 0x00, // black, transparent
            0x84, 0x21, // midpoint channels, opaque
            0xf8, 0x00, // red, transparent
            0x00, 0x01, // black, opaque
        ];
        let source = device_rgba16(&source);
        let output = execute_cpu_oracle(validated(), &source).unwrap();
        assert_eq!(
            output.bgra8(),
            [
                0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 0,
                132, 132, 132, 255, 0, 0, 255, 0, 0, 0, 0, 255,
            ]
        );
    }

    #[test]
    fn every_fixed_register_word_is_rejected_when_mutated() {
        for (index, &register_name) in REGISTER_NAMES.iter().enumerate() {
            let mut spec = M3dPresentationSpec::exact_fixture();
            spec.registers.words[index] ^= 1;
            assert!(matches!(
                validate_exact_presentation(spec),
                Err(ViValidationError::RegisterMismatch { register, .. })
                    if register == register_name
            ));
        }
    }

    #[test]
    fn every_fixed_manager_control_is_rejected_when_mutated() {
        let mut blanked = M3dPresentationSpec::exact_fixture();
        blanked.controls.blanked = true;
        let mut fade = M3dPresentationSpec::exact_fixture();
        fade.controls.fade_u10 = Some(0);
        let mut repeat = M3dPresentationSpec::exact_fixture();
        repeat.controls.repeat_line = true;
        let mut noise = M3dPresentationSpec::exact_fixture();
        noise.controls.noise_seed = 1;

        for spec in [blanked, fade, repeat, noise] {
            assert!(matches!(
                validate_exact_presentation(spec),
                Err(ViValidationError::ControlMismatch { .. })
            ));
        }
    }

    #[test]
    fn every_presentation_identity_component_is_rejected_when_mutated() {
        let mut workload = M3dPresentationSpec::exact_fixture();
        workload.identity.workload_sha256[0] ^= 1;
        let mut origin = M3dPresentationSpec::exact_fixture();
        origin.identity.native_target_origin ^= 1;
        let mut generation = M3dPresentationSpec::exact_fixture();
        generation.identity.native_target_generation += 1;
        let mut retrace = M3dPresentationSpec::exact_fixture();
        retrace.identity.retrace_cycle += 1;
        let mut ordinal = M3dPresentationSpec::exact_fixture();
        ordinal.identity.present_ordinal += 1;

        for (spec, expected) in [
            (workload, "workload_sha256"),
            (origin, "native_target_origin"),
            (generation, "native_target_generation"),
            (retrace, "retrace_cycle"),
            (ordinal, "present_ordinal"),
        ] {
            assert!(matches!(
                validate_exact_presentation(spec),
                Err(ViValidationError::IdentityMismatch { field }) if field == expected
            ));
        }
    }

    #[test]
    fn source_length_mismatch_is_loud() {
        for actual in [0, SOURCE_BYTE_LEN - 1, SOURCE_BYTE_LEN + 1] {
            let source = device_rgba16(&vec![0; actual]);
            assert_eq!(
                execute_cpu_oracle(validated(), &source).unwrap_err(),
                ViExecutionError::SourceLengthMismatch {
                    expected: SOURCE_BYTE_LEN,
                    actual,
                }
            );
        }
    }

    #[test]
    fn cpu_oracle_api_requires_the_m3_3a_device_byte_domain() {
        let _typed_api: fn(
            ValidatedM3dPresentation,
            &DeviceRgba16Bytes,
        ) -> Result<CpuViOutput, ViExecutionError> = execute_cpu_oracle;
    }

    #[test]
    fn padded_rows_extract_tightly_without_leaking_padding() {
        let plan = validated().plans().capture;
        let capture = padded_capture(&EXACT_POST_VI_BGRA8);
        assert!(
            capture.bytes[TIGHT_CAPTURE_ROW_BYTES as usize..PADDED_CAPTURE_ROW_BYTES as usize]
                .iter()
                .all(|byte| *byte == 0xa5)
        );
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &capture).unwrap(),
            EXACT_POST_VI_BGRA8
        );

        let mut changed_padding = capture.clone();
        changed_padding.bytes[TIGHT_CAPTURE_ROW_BYTES as usize] ^= 0xff;
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &changed_padding).unwrap(),
            EXACT_POST_VI_BGRA8
        );

        let mut changed_pixel = capture;
        changed_pixel.bytes[0] ^= 1;
        let tight = extract_tightly_packed_bgra8(plan, &changed_pixel).unwrap();
        assert_eq!(tight[0], 1);
    }

    #[test]
    fn capture_layout_and_identity_mutations_fail_closed() {
        let plan = validated().plans().capture;
        let exact = padded_capture(&EXACT_POST_VI_BGRA8);

        let mut identity = exact.clone();
        identity.identity.capture_ordinal += 1;
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &identity).unwrap_err(),
            CaptureError::Identity
        );

        let mut presentation_identity = exact.clone();
        presentation_identity.identity.presentation.present_ordinal += 1;
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &presentation_identity).unwrap_err(),
            CaptureError::Identity
        );

        let mut format = exact.clone();
        format.format = CaptureFormat::Rgba8Unorm;
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &format).unwrap_err(),
            CaptureError::Format
        );

        let mut width = exact.clone();
        width.width += 1;
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &width).unwrap_err(),
            CaptureError::Extent
        );

        let mut height = exact.clone();
        height.height += 1;
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &height).unwrap_err(),
            CaptureError::Extent
        );

        let mut row_bytes = exact.clone();
        row_bytes.row_bytes -= 1;
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &row_bytes).unwrap_err(),
            CaptureError::RowBytes {
                expected: PADDED_CAPTURE_ROW_BYTES,
                actual: PADDED_CAPTURE_ROW_BYTES - 1,
            }
        );

        let mut short = exact.clone();
        short.bytes.pop();
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &short).unwrap_err(),
            CaptureError::ByteLength {
                expected: PADDED_CAPTURE_BYTE_LEN,
                actual: PADDED_CAPTURE_BYTE_LEN - 1,
            }
        );

        let mut long = exact;
        long.bytes.push(0);
        assert_eq!(
            extract_tightly_packed_bgra8(plan, &long).unwrap_err(),
            CaptureError::ByteLength {
                expected: PADDED_CAPTURE_BYTE_LEN,
                actual: PADDED_CAPTURE_BYTE_LEN + 1,
            }
        );
    }

    #[test]
    fn retained_wgsl_names_only_the_bounded_compute_mechanism() {
        assert!(REPLICATE_RGBA16_WGSL.contains("@compute"));
        assert!(REPLICATE_RGBA16_WGSL.contains("replicate_rgba16"));
        assert!(!REPLICATE_RGBA16_WGSL.contains("textureSample"));
    }
}
