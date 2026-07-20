//! Strict admission for physical N64 VI capture cohorts.
//!
//! This crate verifies manifests and their referenced files. It cannot prove
//! that a producer actually used the console named in a manifest, and a valid
//! cohort never closes fn64's analog-VI matrix row by itself.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

mod campaign;
mod comparison;
mod corpus;
mod digital_boundary;

pub use campaign::{
    plan_capture_campaign, CampaignCorpus, CampaignEvidenceStatus, CampaignRequirement,
    CampaignVector, CaptureCampaignPlan, PlannedCaptureRun, CAMPAIGN_PLAN_SCHEMA,
};
pub use comparison::{
    analyze_pixel_comparison, analyze_pixel_comparison_file, analyze_pixel_comparison_json,
    AggregatePixelMetrics, AlignmentReview, AlignmentWindow, ChannelErrorMetrics, HardwarePixels,
    PixelComparisonManifest, PixelComparisonReport, PixelEncoding, PixelPlane, PixelRectangle,
    ProducerIdentity, ReferencePixels, ReviewedExtraction, RunPixelComparison, SourceSampleWindow,
    MAX_PIXEL_PLANE_BYTES, PIXEL_COMPARISON_REPORT_SCHEMA, PIXEL_COMPARISON_SCHEMA,
};
pub use corpus::{
    generate_digital_vector_corpus, CorpusObjective, DigitalCorpusIndex, DigitalCorpusVector,
    FetchFootprint, GeneratedDigitalCorpus, GeneratedDigitalVector, SourceSpan,
    DIGITAL_CORPUS_SCHEMA, NTSC_SYNTHETIC_CORPUS_ID,
};
pub use digital_boundary::{
    analyze_digital_boundary_file, analyze_digital_boundary_json, DigitalBorderSide,
    DigitalBoundaryAnalysis, DigitalBoundaryAxis, DigitalBoundaryCaptureBundle,
    DigitalBoundaryCase, DigitalBoundaryControls, DigitalBoundaryEdge,
    DigitalBoundaryEvidenceStatus, DigitalBoundaryObservation, DigitalBoundaryPointIntent,
    DigitalBoundaryPosition, DigitalBoundaryProducer, DigitalBoundaryProducerKind,
    DigitalBoundarySourceGeometry, DigitalBoundaryTimingProvenance, DigitalBoundaryViProfile,
    DigitalInterlacedLineSpan, DigitalPostViEncoding, DigitalPostViGeometry, DigitalPostViPlane,
    DIGITAL_BOUNDARY_ANALYSIS_SCHEMA, DIGITAL_BOUNDARY_CAPTURE_SCHEMA,
};

pub const SCHEMA: &str = "fn64.vi-analog-capture.v2";
pub const DIGITAL_VECTOR_SCHEMA: &str = "fn64.vi-digital-input.v2";
pub const CONSENSUS_SCHEMA: &str = "fn64.vi-analog-consensus.v2";
pub const MIN_CLOSURE_RUNS: usize = 10;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_INPUT_VECTOR_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub path: String,
    pub byte_len: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleRegion {
    Ntsc,
    Pal,
    Mpal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViField {
    Progressive,
    InterlacedEven,
    InterlacedOdd,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViPixelType {
    Blank,
    Rgba16,
    Rgba32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViFilters {
    pub pixel_type: ViPixelType,
    pub gamma: bool,
    pub gamma_dither: bool,
    pub divot: bool,
    pub dither_filter: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViRegisters {
    pub status: u32,
    pub origin: u32,
    pub width: u32,
    pub intr: u32,
    pub current: u32,
    pub burst: u32,
    pub v_sync: u32,
    pub h_sync: u32,
    pub leap: u32,
    pub h_start: u32,
    pub v_start: u32,
    pub v_burst: u32,
    pub x_scale: u32,
    pub y_scale: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramebufferEncoding {
    Rgba16BigEndian,
    Rgba32BigEndian,
}

impl FramebufferEncoding {
    fn bytes_per_pixel(&self) -> u32 {
        match self {
            Self::Rgba16BigEndian => 2,
            Self::Rgba32BigEndian => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalFramebuffer {
    pub encoding: FramebufferEncoding,
    pub width: u32,
    pub height: u32,
    pub row_stride_bytes: u32,
    pub framebuffer_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blob {
    pub byte_len: u64,
    pub sha256: String,
    pub bytes_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorFramebuffer {
    pub encoding: FramebufferEncoding,
    pub width: u32,
    pub height: u32,
    pub row_stride_bytes: u32,
    pub contents: Blob,
    /// One complete resident RDP coverage count (1..=8) per active pixel.
    /// For RGBA16 this binds the two hidden bits omitted by framebuffer bytes;
    /// the validator also proves the visible coverage bit is coherent.
    pub coverage_counts: Blob,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalVector {
    pub schema: String,
    pub vector_id: String,
    pub content_class: String,
    pub framebuffer: VectorFramebuffer,
    pub registers: ViRegisters,
    pub filters: ViFilters,
    pub region: ConsoleRegion,
    pub field: ViField,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetKind {
    PowerCycle,
    ResetButton,
    WarmBoot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetAndRepeat {
    pub kind: ResetKind,
    pub sequence_id: String,
    /// Privacy-preserving identity of the observed reset event for this run.
    /// Consensus requires a distinct value for every power-cycle repeat.
    pub reset_event_id_sha256: String,
    pub repeat_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalInput {
    pub vector_id: String,
    /// Canonical public vector containing synthetic framebuffer bytes and the
    /// register-write program. Its file digest is the input-vector identity.
    pub vector_artifact: ArtifactRef,
    pub framebuffer: DigitalFramebuffer,
    pub registers: ViRegisters,
    pub filters: ViFilters,
    pub region: ConsoleRegion,
    pub field: ViField,
    pub reset_and_repeat: ResetAndRepeat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalogSignal {
    Composite,
    SVideo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureEncoding {
    RawAdc,
    V210,
    Ffv1Matroska,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureDevice {
    pub manufacturer: String,
    pub model: String,
    pub unit_id_sha256: String,
    pub firmware: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureChain {
    pub device: CaptureDevice,
    pub cable: String,
    pub termination_ohms: u16,
    pub sample_rate_hz: u64,
    pub encoding: CaptureEncoding,
    pub tool_name: String,
    pub tool_version: String,
    pub tool_binary_sha256: String,
    pub settings_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalogOutput {
    pub signal: AnalogSignal,
    pub chain: CaptureChain,
    pub first_field: u32,
    pub field_count: u32,
    /// Lossless physical-video capture. This is distinct from the digital
    /// vector and its framebuffer digest.
    pub capture_artifact: ArtifactRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleClass {
    RetailNintendo64,
    DevelopmentNintendo64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Provenance {
    Hardware {
        console_class: ConsoleClass,
        console_unit_id_sha256: String,
        motherboard_revision: String,
        video_encoder_revision: String,
        modification_state: String,
        operator: String,
        recorded_at_utc: String,
    },
    SyntheticFixture {
        reason: String,
        recorded_at_utc: String,
    },
}

impl Provenance {
    fn recorded_at_utc(&self) -> &str {
        match self {
            Self::Hardware {
                recorded_at_utc, ..
            }
            | Self::SyntheticFixture {
                recorded_at_utc, ..
            } => recorded_at_utc,
        }
    }

    fn is_hardware(&self) -> bool {
        matches!(self, Self::Hardware { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureManifest {
    pub schema: String,
    pub suite_id: String,
    pub run_id: String,
    pub content_class: String,
    pub provenance: Provenance,
    pub digital_input: DigitalInput,
    pub analog_output: AnalogOutput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationReceipt {
    pub schema: &'static str,
    pub manifest_sha256: String,
    pub input_vector_sha256: String,
    pub output_artifact_sha256: String,
    pub hardware_provenance: bool,
    pub closure_eligible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedCapture {
    manifest: CaptureManifest,
    receipt: ValidationReceipt,
}

impl ValidatedCapture {
    pub fn manifest(&self) -> &CaptureManifest {
        &self.manifest
    }

    pub fn receipt(&self) -> &ValidationReceipt {
        &self.receipt
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsensusRun {
    pub run_id: String,
    pub repeat_index: u32,
    pub reset_event_id_sha256: String,
    pub recorded_at_utc: String,
    pub manifest_sha256: String,
    pub output_artifact_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareConsensus {
    pub schema: &'static str,
    pub minimum_runs: usize,
    pub run_count: usize,
    pub input_vector_sha256: String,
    pub exact_output_sha256: Option<String>,
    pub distinct_output_count: usize,
    pub runs: Vec<ConsensusRun>,
    pub consensus_sha256: String,
    /// A controlled hardware cohort is evidence input. It does not implement
    /// the missing analog pipeline or perform a fn64-versus-capture analysis.
    pub base_matrix_row_closed: bool,
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

pub fn validate_manifest_file(path: &Path) -> Result<ValidatedCapture, ValidationError> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        ValidationError::new(format!("read manifest {}: {error}", path.display()))
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(ValidationError::new(format!(
            "manifest {} must be a regular non-symlink file",
            path.display()
        )));
    }
    // Keep one handle across metadata and parsing: otherwise a rename between
    // the metadata lookup and a second path read validates file A but admits B.
    let mut file = fs::File::open(path).map_err(|error| {
        ValidationError::new(format!("read manifest {}: {error}", path.display()))
    })?;
    let metadata = file.metadata().map_err(|error| {
        ValidationError::new(format!(
            "read manifest metadata {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(ValidationError::new(format!(
            "manifest {} must remain a regular file while open",
            path.display()
        )));
    }
    if metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(ValidationError::new(format!(
            "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(|error| {
        ValidationError::new(format!("read manifest {}: {error}", path.display()))
    })?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    validate_json(&bytes, base)
}

pub fn validate_json(
    bytes: &[u8],
    artifact_base: &Path,
) -> Result<ValidatedCapture, ValidationError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ValidationError::new(format!(
            "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    let manifest: CaptureManifest = serde_json::from_slice(bytes)
        .map_err(|error| ValidationError::new(format!("malformed manifest: {error}")))?;
    validate_manifest(manifest, artifact_base)
}

pub fn validate_manifest(
    manifest: CaptureManifest,
    artifact_base: &Path,
) -> Result<ValidatedCapture, ValidationError> {
    if manifest.schema != SCHEMA {
        return Err(ValidationError::new(format!(
            "unsupported schema {:?}; expected {SCHEMA:?}",
            manifest.schema
        )));
    }
    text("suite_id", &manifest.suite_id)?;
    text("run_id", &manifest.run_id)?;
    if manifest.content_class != "synthetic_vi_probe" {
        return Err(ValidationError::new(
            "content_class must be `synthetic_vi_probe`; ROM/game-derived input is forbidden",
        ));
    }
    validate_provenance(&manifest.provenance)?;
    validate_digital(&manifest.digital_input, artifact_base)?;
    validate_analog(&manifest.analog_output, artifact_base)?;
    if manifest.digital_input.vector_artifact.path == manifest.analog_output.capture_artifact.path
        || manifest.digital_input.vector_artifact.sha256
            == manifest.analog_output.capture_artifact.sha256
    {
        return Err(ValidationError::new(
            "digital input vector and analog output capture must be distinct artifacts",
        ));
    }

    let canonical = serde_json::to_vec(&manifest)
        .map_err(|error| ValidationError::new(format!("canonicalize manifest: {error}")))?;
    let hardware = manifest.provenance.is_hardware();
    let receipt = ValidationReceipt {
        schema: SCHEMA,
        manifest_sha256: digest(&canonical),
        input_vector_sha256: manifest.digital_input.vector_artifact.sha256.clone(),
        output_artifact_sha256: manifest.analog_output.capture_artifact.sha256.clone(),
        hardware_provenance: hardware,
        // One run never satisfies the repeated-capture admission bar.
        closure_eligible: false,
    };
    Ok(ValidatedCapture { manifest, receipt })
}

fn validate_provenance(provenance: &Provenance) -> Result<(), ValidationError> {
    utc("provenance.recorded_at_utc", provenance.recorded_at_utc())?;
    match provenance {
        Provenance::Hardware {
            console_unit_id_sha256,
            motherboard_revision,
            video_encoder_revision,
            modification_state,
            operator,
            ..
        } => {
            sha256("provenance.console_unit_id_sha256", console_unit_id_sha256)?;
            for (label, value) in [
                ("provenance.motherboard_revision", motherboard_revision),
                ("provenance.video_encoder_revision", video_encoder_revision),
                ("provenance.modification_state", modification_state),
                ("provenance.operator", operator),
            ] {
                text(label, value)?;
            }
        }
        Provenance::SyntheticFixture { reason, .. } => text("provenance.reason", reason)?,
    }
    Ok(())
}

fn validate_digital(digital: &DigitalInput, base: &Path) -> Result<(), ValidationError> {
    text("digital_input.vector_id", &digital.vector_id)?;
    let vector_bytes = validate_artifact(
        "digital_input.vector_artifact",
        &digital.vector_artifact,
        base,
        Some(MAX_INPUT_VECTOR_BYTES as u64),
    )?;
    let vector: DigitalVector = serde_json::from_slice(&vector_bytes).map_err(|error| {
        ValidationError::new(format!("malformed digital input vector: {error}"))
    })?;
    validate_vector(&vector)?;
    sha256(
        "digital_input.framebuffer.framebuffer_sha256",
        &digital.framebuffer.framebuffer_sha256,
    )?;
    if digital.framebuffer.width == 0 || digital.framebuffer.height == 0 {
        return Err(ValidationError::new(
            "digital framebuffer dimensions must be nonzero",
        ));
    }
    let minimum_stride = digital
        .framebuffer
        .width
        .checked_mul(digital.framebuffer.encoding.bytes_per_pixel())
        .ok_or_else(|| ValidationError::new("digital framebuffer row size overflow"))?;
    if digital.framebuffer.row_stride_bytes < minimum_stride {
        return Err(ValidationError::new(format!(
            "digital framebuffer row_stride_bytes {} is below minimum {minimum_stride}",
            digital.framebuffer.row_stride_bytes
        )));
    }
    text(
        "digital_input.reset_and_repeat.sequence_id",
        &digital.reset_and_repeat.sequence_id,
    )?;
    sha256(
        "digital_input.reset_and_repeat.reset_event_id_sha256",
        &digital.reset_and_repeat.reset_event_id_sha256,
    )?;
    if digital.registers.origin >= 0x0080_0000 {
        return Err(ValidationError::new(
            "VI origin is outside the eight-MiB physical RDRAM aperture",
        ));
    }
    validate_framebuffer_rdram_bounds(&digital.framebuffer, digital.registers.origin)?;
    if digital.registers.width == 0 || digital.registers.width != digital.framebuffer.width {
        return Err(ValidationError::new(
            "VI width must be nonzero and match the digital framebuffer width",
        ));
    }
    let status = digital.registers.status;
    let expected_pixel = match status & 3 {
        0 => ViPixelType::Blank,
        1 => {
            return Err(ValidationError::new(
                "VI STATUS selects reserved pixel type 1",
            ))
        }
        2 => ViPixelType::Rgba16,
        3 => ViPixelType::Rgba32,
        _ => unreachable!(),
    };
    let encoding_matches = matches!(
        (&expected_pixel, &digital.framebuffer.encoding),
        (ViPixelType::Blank, _)
            | (ViPixelType::Rgba16, FramebufferEncoding::Rgba16BigEndian)
            | (ViPixelType::Rgba32, FramebufferEncoding::Rgba32BigEndian)
    );
    if !encoding_matches {
        return Err(ValidationError::new(
            "digital framebuffer encoding does not match VI STATUS pixel type",
        ));
    }
    if digital.filters.pixel_type != expected_pixel
        || digital.filters.gamma != (status & (1 << 3) != 0)
        || digital.filters.gamma_dither != (status & (1 << 2) != 0)
        || digital.filters.divot != (status & (1 << 4) != 0)
        || digital.filters.dither_filter != (status & (1 << 16) != 0)
    {
        return Err(ValidationError::new(
            "typed VI filters do not match VI STATUS bits",
        ));
    }
    let interlaced = status & (1 << 6) != 0;
    if interlaced == matches!(digital.field, ViField::Progressive) {
        return Err(ValidationError::new(
            "field identity does not match VI STATUS serrate bit",
        ));
    }
    match digital.field {
        ViField::Progressive => {}
        ViField::InterlacedEven if digital.registers.current & 1 == 0 => {}
        ViField::InterlacedOdd if digital.registers.current & 1 == 1 => {}
        ViField::InterlacedEven | ViField::InterlacedOdd => {
            return Err(ValidationError::new(
                "interlaced field identity does not match VI CURRENT parity",
            ));
        }
    }
    if vector.vector_id != digital.vector_id
        || vector.registers != digital.registers
        || vector.filters != digital.filters
        || vector.region != digital.region
        || vector.field != digital.field
        || vector.framebuffer.encoding != digital.framebuffer.encoding
        || vector.framebuffer.width != digital.framebuffer.width
        || vector.framebuffer.height != digital.framebuffer.height
        || vector.framebuffer.row_stride_bytes != digital.framebuffer.row_stride_bytes
        || vector.framebuffer.contents.sha256 != digital.framebuffer.framebuffer_sha256
    {
        return Err(ValidationError::new(
            "digital input manifest metadata does not match its vector artifact",
        ));
    }
    Ok(())
}

fn validate_analog(analog: &AnalogOutput, base: &Path) -> Result<(), ValidationError> {
    for (label, value) in [
        (
            "capture device manufacturer",
            &analog.chain.device.manufacturer,
        ),
        ("capture device model", &analog.chain.device.model),
        ("capture device firmware", &analog.chain.device.firmware),
        ("capture cable", &analog.chain.cable),
        ("capture tool name", &analog.chain.tool_name),
        ("capture tool version", &analog.chain.tool_version),
    ] {
        text(label, value)?;
    }
    sha256(
        "capture device unit_id_sha256",
        &analog.chain.device.unit_id_sha256,
    )?;
    sha256(
        "capture tool_binary_sha256",
        &analog.chain.tool_binary_sha256,
    )?;
    sha256("capture settings_sha256", &analog.chain.settings_sha256)?;
    if analog.chain.termination_ohms == 0 || analog.chain.sample_rate_hz == 0 {
        return Err(ValidationError::new(
            "capture termination and sample rate must be nonzero",
        ));
    }
    if analog.field_count == 0 {
        return Err(ValidationError::new("analog field_count must be nonzero"));
    }
    validate_artifact(
        "analog_output.capture_artifact",
        &analog.capture_artifact,
        base,
        None,
    )
    .map(|_| ())
}

fn validate_artifact(
    label: &str,
    artifact: &ArtifactRef,
    base: &Path,
    maximum_bytes: Option<u64>,
) -> Result<Vec<u8>, ValidationError> {
    if artifact.byte_len == 0 {
        return Err(ValidationError::new(format!("{label} must be nonempty")));
    }
    sha256(&format!("{label}.sha256"), &artifact.sha256)?;
    let relative = Path::new(&artifact.path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ValidationError::new(format!(
            "{label}.path must be a contained relative path"
        )));
    }
    if relative.as_os_str().is_empty() {
        return Err(ValidationError::new(format!("{label}.path is empty")));
    }
    let canonical_base = fs::canonicalize(base).map_err(|error| {
        ValidationError::new(format!(
            "{label} artifact base {} cannot be canonicalized: {error}",
            base.display()
        ))
    })?;
    let path: PathBuf = base.join(relative);
    let path_metadata = fs::symlink_metadata(&path).map_err(|error| {
        ValidationError::new(format!(
            "{label} missing artifact {}: {error}",
            path.display()
        ))
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(ValidationError::new(format!(
            "{label} artifact {} must be a regular non-symlink file",
            path.display()
        )));
    }
    let canonical_path = fs::canonicalize(&path).map_err(|error| {
        ValidationError::new(format!(
            "{label} artifact {} cannot be canonicalized: {error}",
            path.display()
        ))
    })?;
    if !canonical_path.starts_with(&canonical_base) {
        return Err(ValidationError::new(format!(
            "{label} artifact {} escapes canonical artifact base {}",
            path.display(),
            canonical_base.display()
        )));
    }
    // Open the canonical target once, then take all length and digest evidence
    // from that handle. A path lookup must not validate one object and hash a
    // replacement selected by a later lookup.
    let mut file = fs::File::open(&canonical_path).map_err(|error| {
        ValidationError::new(format!(
            "read {label} artifact {}: {error}",
            canonical_path.display()
        ))
    })?;
    let metadata = file.metadata().map_err(|error| {
        ValidationError::new(format!(
            "read {label} artifact metadata {}: {error}",
            canonical_path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(ValidationError::new(format!(
            "{label} artifact {} must remain a regular file while open",
            canonical_path.display()
        )));
    }
    if metadata.len() != artifact.byte_len {
        return Err(ValidationError::new(format!(
            "{label} byte_len mismatch: manifest={}, file={}",
            artifact.byte_len,
            metadata.len()
        )));
    }
    if maximum_bytes.is_some_and(|maximum| metadata.len() > maximum) {
        return Err(ValidationError::new(format!(
            "{label} exceeds maximum byte length {}",
            maximum_bytes.expect("checked Some maximum")
        )));
    }
    let mut hasher = Sha256::new();
    let mut retained = maximum_bytes.map(|_| Vec::with_capacity(metadata.len() as usize));
    let mut buffer = [0u8; 64 * 1024];
    let mut observed_len = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            ValidationError::new(format!("read {label} artifact {}: {error}", path.display()))
        })?;
        if count == 0 {
            break;
        }
        observed_len = observed_len
            .checked_add(count as u64)
            .ok_or_else(|| ValidationError::new(format!("{label} streamed length overflow")))?;
        hasher.update(&buffer[..count]);
        if let Some(bytes) = &mut retained {
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    if observed_len != artifact.byte_len {
        return Err(ValidationError::new(format!(
            "{label} changed length while hashing: manifest={}, observed={observed_len}",
            artifact.byte_len
        )));
    }
    let observed = format!("{:x}", hasher.finalize());
    if observed != artifact.sha256 {
        return Err(ValidationError::new(format!(
            "{label} SHA-256 mismatch: manifest={}, file={observed}",
            artifact.sha256
        )));
    }
    Ok(retained.unwrap_or_default())
}

fn validate_framebuffer_rdram_bounds(
    framebuffer: &DigitalFramebuffer,
    origin: u32,
) -> Result<(), ValidationError> {
    let active_row_bytes = framebuffer
        .width
        .checked_mul(framebuffer.encoding.bytes_per_pixel())
        .ok_or_else(|| ValidationError::new("digital framebuffer active row size overflow"))?;
    let last_row_offset = framebuffer
        .row_stride_bytes
        .checked_mul(framebuffer.height.saturating_sub(1))
        .ok_or_else(|| ValidationError::new("digital framebuffer last-row offset overflow"))?;
    let end = origin
        .checked_add(last_row_offset)
        .and_then(|last_row| last_row.checked_add(active_row_bytes))
        .ok_or_else(|| ValidationError::new("digital framebuffer RDRAM end overflow"))?;
    if end > 0x0080_0000 {
        return Err(ValidationError::new(format!(
            "digital framebuffer range ending at {end:#010x} exceeds the eight-MiB physical RDRAM aperture"
        )));
    }
    Ok(())
}

fn validate_vector(vector: &DigitalVector) -> Result<(), ValidationError> {
    if vector.schema != DIGITAL_VECTOR_SCHEMA {
        return Err(ValidationError::new(format!(
            "unsupported digital vector schema {:?}; expected {DIGITAL_VECTOR_SCHEMA:?}",
            vector.schema
        )));
    }
    text("digital vector_id", &vector.vector_id)?;
    if vector.content_class != "synthetic_vi_probe" {
        return Err(ValidationError::new(
            "digital vector content_class must be `synthetic_vi_probe`",
        ));
    }
    let bytes = decode_blob("digital vector framebuffer", &vector.framebuffer.contents)?;
    if vector.framebuffer.width == 0 || vector.framebuffer.height == 0 {
        return Err(ValidationError::new(
            "digital vector framebuffer dimensions must be nonzero",
        ));
    }
    let minimum_stride = vector
        .framebuffer
        .width
        .checked_mul(vector.framebuffer.encoding.bytes_per_pixel())
        .ok_or_else(|| ValidationError::new("digital vector row size overflow"))?;
    if vector.framebuffer.row_stride_bytes < minimum_stride {
        return Err(ValidationError::new(
            "digital vector framebuffer row stride is too small",
        ));
    }
    let expected = u64::from(vector.framebuffer.row_stride_bytes)
        .checked_mul(u64::from(vector.framebuffer.height))
        .ok_or_else(|| ValidationError::new("digital vector framebuffer length overflow"))?;
    if bytes.len() as u64 != expected {
        return Err(ValidationError::new(format!(
            "digital vector framebuffer byte length {} does not match geometry {expected}",
            bytes.len()
        )));
    }
    let coverage = decode_blob(
        "digital vector framebuffer coverage counts",
        &vector.framebuffer.coverage_counts,
    )?;
    let pixel_count = u64::from(vector.framebuffer.width)
        .checked_mul(u64::from(vector.framebuffer.height))
        .ok_or_else(|| ValidationError::new("digital vector pixel count overflow"))?;
    if coverage.len() as u64 != pixel_count {
        return Err(ValidationError::new(format!(
            "digital vector coverage byte length {} does not match pixel count {pixel_count}",
            coverage.len()
        )));
    }
    for (pixel, &count) in coverage.iter().enumerate() {
        if !(1..=8).contains(&count) {
            return Err(ValidationError::new(format!(
                "digital vector coverage count at pixel {pixel} is {count}; expected 1..=8"
            )));
        }
        let row = pixel / vector.framebuffer.width as usize;
        let column = pixel % vector.framebuffer.width as usize;
        let byte = row * vector.framebuffer.row_stride_bytes as usize
            + column * vector.framebuffer.encoding.bytes_per_pixel() as usize;
        let stored = count - 1;
        let coherent = match vector.framebuffer.encoding {
            FramebufferEncoding::Rgba16BigEndian => {
                let visible_coverage = bytes[byte + 1] & 1;
                visible_coverage == (stored >> 2) & 1
            }
            FramebufferEncoding::Rgba32BigEndian => bytes[byte + 3] >> 5 == stored,
        };
        if !coherent {
            return Err(ValidationError::new(format!(
                "digital vector framebuffer coverage encoding disagrees with coverage count at pixel {pixel}"
            )));
        }
    }
    Ok(())
}

/// Validates one standalone public digital-vector artifact without admitting
/// an analog capture or assigning hardware provenance.
pub fn validate_digital_vector_json(bytes: &[u8]) -> Result<DigitalVector, ValidationError> {
    if bytes.len() > MAX_INPUT_VECTOR_BYTES {
        return Err(ValidationError::new(format!(
            "digital input vector exceeds {MAX_INPUT_VECTOR_BYTES} bytes"
        )));
    }
    let vector: DigitalVector = serde_json::from_slice(bytes).map_err(|error| {
        ValidationError::new(format!("malformed digital input vector: {error}"))
    })?;
    validate_vector(&vector)?;
    Ok(vector)
}

fn decode_blob(label: &str, blob: &Blob) -> Result<Vec<u8>, ValidationError> {
    sha256(&format!("{label}.sha256"), &blob.sha256)?;
    if !blob.bytes_hex.len().is_multiple_of(2) {
        return Err(ValidationError::new(format!(
            "{label}.bytes_hex has odd length"
        )));
    }
    if !blob
        .bytes_hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ValidationError::new(format!(
            "{label}.bytes_hex is not lowercase hexadecimal"
        )));
    }
    let bytes = blob
        .bytes_hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("hex pair is ASCII-sized");
            u8::from_str_radix(text, 16).map_err(|_| {
                ValidationError::new(format!("{label}.bytes_hex is not lowercase hexadecimal"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if bytes.len() as u64 != blob.byte_len {
        return Err(ValidationError::new(format!(
            "{label}.byte_len mismatch: manifest={}, decoded={}",
            blob.byte_len,
            bytes.len()
        )));
    }
    let observed = digest(&bytes);
    if observed != blob.sha256 {
        return Err(ValidationError::new(format!(
            "{label} SHA-256 mismatch: manifest={}, decoded={observed}",
            blob.sha256
        )));
    }
    Ok(bytes)
}

pub fn validate_hardware_consensus(
    captures: &[ValidatedCapture],
    minimum_runs: usize,
) -> Result<HardwareConsensus, ValidationError> {
    if minimum_runs < MIN_CLOSURE_RUNS {
        return Err(ValidationError::new(format!(
            "hardware consensus minimum_runs must be at least {MIN_CLOSURE_RUNS}"
        )));
    }
    if captures.len() < minimum_runs {
        return Err(ValidationError::new(format!(
            "hardware consensus requires at least {minimum_runs} runs; received {}",
            captures.len()
        )));
    }
    let first = &captures[0];
    if !first.manifest.provenance.is_hardware() {
        return Err(ValidationError::new(
            "run 1 lacks hardware provenance; synthetic validation is non-certifying",
        ));
    }
    if !matches!(
        first.manifest.digital_input.reset_and_repeat.kind,
        ResetKind::PowerCycle
    ) {
        return Err(ValidationError::new(
            "hardware consensus requires every run to replay from power_cycle",
        ));
    }
    let mut manifest_digests = BTreeSet::new();
    let mut run_ids = BTreeSet::new();
    let mut timestamps = BTreeSet::new();
    let mut repeat_indices = BTreeSet::new();
    let mut reset_event_ids = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    let mut runs = Vec::with_capacity(captures.len());
    for (index, capture) in captures.iter().enumerate() {
        let run = index + 1;
        if !capture.manifest.provenance.is_hardware() {
            return Err(ValidationError::new(format!(
                "run {run} lacks hardware provenance; synthetic validation is non-certifying"
            )));
        }
        if !matches!(
            capture.manifest.digital_input.reset_and_repeat.kind,
            ResetKind::PowerCycle
        ) {
            return Err(ValidationError::new(format!(
                "run {run} did not replay from power_cycle"
            )));
        }
        same_control(first, capture, run)?;
        if !manifest_digests.insert(&capture.receipt.manifest_sha256) {
            return Err(ValidationError::new(format!(
                "run {run} duplicates manifest digest {}",
                capture.receipt.manifest_sha256
            )));
        }
        if !run_ids.insert(&capture.manifest.run_id) {
            return Err(ValidationError::new(format!(
                "run {run} duplicates run_id {:?}",
                capture.manifest.run_id
            )));
        }
        let timestamp = capture.manifest.provenance.recorded_at_utc();
        if !timestamps.insert(timestamp) {
            return Err(ValidationError::new(format!(
                "run {run} duplicates recorded_at_utc {timestamp:?}"
            )));
        }
        let repeat = capture.manifest.digital_input.reset_and_repeat.repeat_index;
        if !repeat_indices.insert(repeat) {
            return Err(ValidationError::new(format!(
                "run {run} duplicates repeat_index {repeat}"
            )));
        }
        let reset_event_id = capture
            .manifest
            .digital_input
            .reset_and_repeat
            .reset_event_id_sha256
            .clone();
        if !reset_event_ids.insert(reset_event_id.clone()) {
            return Err(ValidationError::new(format!(
                "run {run} duplicates reset_event_id_sha256 {reset_event_id}"
            )));
        }
        outputs.insert(capture.receipt.output_artifact_sha256.clone());
        runs.push(ConsensusRun {
            run_id: capture.manifest.run_id.clone(),
            repeat_index: repeat,
            reset_event_id_sha256: reset_event_id,
            recorded_at_utc: timestamp.to_owned(),
            manifest_sha256: capture.receipt.manifest_sha256.clone(),
            output_artifact_sha256: capture.receipt.output_artifact_sha256.clone(),
        });
    }
    let run_count = u32::try_from(captures.len())
        .map_err(|_| ValidationError::new("hardware consensus run count exceeds u32"))?;
    let expected_repeat_indices = (0..run_count).collect::<BTreeSet<_>>();
    if repeat_indices != expected_repeat_indices {
        return Err(ValidationError::new(format!(
            "hardware consensus repeat_index values must exactly cover 0..{run_count}"
        )));
    }
    runs.sort_by(|left, right| left.manifest_sha256.cmp(&right.manifest_sha256));
    let exact_output_sha256 = if outputs.len() == 1 {
        outputs.first().cloned()
    } else {
        None
    };
    let mut result = HardwareConsensus {
        schema: CONSENSUS_SCHEMA,
        minimum_runs,
        run_count: captures.len(),
        input_vector_sha256: first.receipt.input_vector_sha256.clone(),
        exact_output_sha256,
        distinct_output_count: outputs.len(),
        runs,
        consensus_sha256: String::new(),
        base_matrix_row_closed: false,
    };
    let canonical = serde_json::to_vec(&result)
        .map_err(|error| ValidationError::new(format!("encode consensus: {error}")))?;
    result.consensus_sha256 = digest(&canonical);
    Ok(result)
}

fn same_control(
    expected: &ValidatedCapture,
    actual: &ValidatedCapture,
    run: usize,
) -> Result<(), ValidationError> {
    macro_rules! same {
        ($path:literal, $left:expr, $right:expr) => {
            if $left != $right {
                return Err(ValidationError::new(format!(
                    "run {run} mismatch at {}",
                    $path
                )));
            }
        };
    }
    same!(
        "suite_id",
        &expected.manifest.suite_id,
        &actual.manifest.suite_id
    );
    same!(
        "content_class",
        &expected.manifest.content_class,
        &actual.manifest.content_class
    );
    let left = &expected.manifest.digital_input;
    let right = &actual.manifest.digital_input;
    same!("digital_input.vector_id", &left.vector_id, &right.vector_id);
    same!(
        "digital_input.vector_artifact.byte_len",
        left.vector_artifact.byte_len,
        right.vector_artifact.byte_len
    );
    same!(
        "digital_input.vector_artifact.sha256",
        &left.vector_artifact.sha256,
        &right.vector_artifact.sha256
    );
    same!(
        "digital_input.framebuffer",
        &left.framebuffer,
        &right.framebuffer
    );
    same!("digital_input.registers", &left.registers, &right.registers);
    same!("digital_input.filters", &left.filters, &right.filters);
    same!("digital_input.region", &left.region, &right.region);
    same!("digital_input.field", &left.field, &right.field);
    same!(
        "digital_input.reset_and_repeat.kind",
        &left.reset_and_repeat.kind,
        &right.reset_and_repeat.kind
    );
    same!(
        "digital_input.reset_and_repeat.sequence_id",
        &left.reset_and_repeat.sequence_id,
        &right.reset_and_repeat.sequence_id
    );
    same!(
        "analog_output.signal",
        &expected.manifest.analog_output.signal,
        &actual.manifest.analog_output.signal
    );
    same!(
        "analog_output.chain",
        &expected.manifest.analog_output.chain,
        &actual.manifest.analog_output.chain
    );
    same!(
        "analog_output.first_field",
        expected.manifest.analog_output.first_field,
        actual.manifest.analog_output.first_field
    );
    same!(
        "analog_output.field_count",
        expected.manifest.analog_output.field_count,
        actual.manifest.analog_output.field_count
    );
    match (&expected.manifest.provenance, &actual.manifest.provenance) {
        (
            Provenance::Hardware {
                console_class: left_class,
                console_unit_id_sha256: left_unit,
                motherboard_revision: left_board,
                video_encoder_revision: left_encoder,
                modification_state: left_modification,
                operator: left_operator,
                ..
            },
            Provenance::Hardware {
                console_class: right_class,
                console_unit_id_sha256: right_unit,
                motherboard_revision: right_board,
                video_encoder_revision: right_encoder,
                modification_state: right_modification,
                operator: right_operator,
                ..
            },
        ) => {
            same!("provenance.console_class", left_class, right_class);
            same!("provenance.console_unit_id_sha256", left_unit, right_unit);
            same!("provenance.motherboard_revision", left_board, right_board);
            same!(
                "provenance.video_encoder_revision",
                left_encoder,
                right_encoder
            );
            same!(
                "provenance.modification_state",
                left_modification,
                right_modification
            );
            same!("provenance.operator", left_operator, right_operator);
        }
        _ => unreachable!("hardware provenance checked before controlled comparison"),
    }
    Ok(())
}

fn text(label: &str, value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(ValidationError::new(format!("{label} is empty or invalid")));
    }
    Ok(())
}

fn utc(label: &str, value: &str) -> Result<(), ValidationError> {
    text(label, value)?;
    let bytes = value.as_bytes();
    let canonical_shape = bytes.len() == 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        });
    if !canonical_shape {
        return Err(ValidationError::new(format!(
            "{label} must be canonical UTC `YYYY-MM-DDTHH:MM:SSZ`"
        )));
    }
    let number = |start: usize, end: usize| {
        std::str::from_utf8(&bytes[start..end])
            .expect("canonical UTC fields are ASCII")
            .parse::<u32>()
            .expect("canonical UTC fields contain only digits")
    };
    let year = number(0, 4);
    let month = number(5, 7);
    let day = number(8, 10);
    let hour = number(11, 13);
    let minute = number(14, 16);
    let second = number(17, 19);
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => 0,
    };
    if year == 0 || day == 0 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return Err(ValidationError::new(format!(
            "{label} is not a valid canonical UTC calendar timestamp"
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
            "{label} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framebuffer(width: u32, height: u32, row_stride_bytes: u32) -> DigitalFramebuffer {
        DigitalFramebuffer {
            encoding: FramebufferEncoding::Rgba16BigEndian,
            width,
            height,
            row_stride_bytes,
            framebuffer_sha256: "0".repeat(64),
        }
    }

    #[test]
    fn framebuffer_rdram_arithmetic_rejects_offset_and_end_overflow() {
        let offset =
            validate_framebuffer_rdram_bounds(&framebuffer(1, u32::MAX, u32::MAX), 0).unwrap_err();
        assert!(offset.to_string().contains("last-row offset overflow"));

        let end = validate_framebuffer_rdram_bounds(&framebuffer(1, 65_536, 65_536), 0x0010_0000)
            .unwrap_err();
        assert!(end.to_string().contains("RDRAM end overflow"));
    }

    #[test]
    fn utc_requires_a_valid_canonical_calendar_timestamp() {
        assert!(utc("timestamp", "2024-02-29T23:59:59Z").is_ok());
        for invalid in [
            "TZ",
            "0000-01-01T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2024-04-31T00:00:00Z",
            "2024-13-01T00:00:00Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01T00:60:00Z",
            "2024-01-01T00:00:60Z",
            "2024-01-01T00:00:00.0Z",
            "2024-01-01T00:00:00+00:00",
        ] {
            assert!(utc("timestamp", invalid).is_err(), "accepted {invalid}");
        }
    }
}
