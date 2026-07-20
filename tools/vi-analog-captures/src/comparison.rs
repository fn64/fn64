use super::{
    digest, sha256, text, utc, validate_artifact, validate_hardware_consensus, validate_json,
    AnalogSignal, ArtifactRef, ConsoleRegion, ValidatedCapture, ValidationError, ViField,
    ViFilters, ViRegisters, MAX_MANIFEST_BYTES, MIN_CLOSURE_RUNS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::Path;

pub const PIXEL_COMPARISON_SCHEMA: &str = "fn64.vi-pixel-comparison.v2";
pub const PIXEL_COMPARISON_REPORT_SCHEMA: &str = "fn64.vi-pixel-comparison-report.v2";
pub const MAX_PIXEL_PLANE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelEncoding {
    Rgb8,
    Rgb16BigEndian,
}

impl PixelEncoding {
    fn bytes_per_sample(&self) -> u64 {
        match self {
            Self::Rgb8 => 1,
            Self::Rgb16BigEndian => 2,
        }
    }

    fn sample_max(&self) -> u32 {
        match self {
            Self::Rgb8 => u8::MAX.into(),
            Self::Rgb16BigEndian => u16::MAX.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelPlane {
    pub artifact: ArtifactRef,
    pub encoding: PixelEncoding,
    pub width: u32,
    pub height: u32,
    pub row_stride_bytes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerIdentity {
    pub name: String,
    pub version: String,
    pub binary_sha256: String,
    pub settings_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencePixels {
    pub input_vector_sha256: String,
    pub producer: ProducerIdentity,
    pub plane: PixelPlane,
    pub active_output: PixelRectangle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelRectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSampleWindow {
    pub field_number: u32,
    pub first_line: u32,
    pub line_count: u32,
    pub first_sample: u32,
    pub samples_per_line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedExtraction {
    pub source_window: SourceSampleWindow,
    pub active_output: PixelRectangle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignmentWindow {
    pub reference_x: u32,
    pub reference_y: u32,
    pub observation_x: u32,
    pub observation_y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwarePixels {
    pub run_id: String,
    pub capture_manifest: ArtifactRef,
    pub source_capture_sha256: String,
    pub extractor: ProducerIdentity,
    pub plane: PixelPlane,
    pub extraction: ReviewedExtraction,
    pub alignment: AlignmentWindow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignmentReview {
    pub reviewer: String,
    pub reviewed_at_utc: String,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelComparisonManifest {
    pub schema: String,
    pub analysis_id: String,
    pub content_class: String,
    pub expected_consensus_sha256: String,
    /// Names the common integer sample domain. Its referenced specification
    /// owns any decoding/color transform; this analyzer invents no transform.
    pub sample_domain_id: String,
    pub sample_domain_spec: ArtifactRef,
    pub reference: ReferencePixels,
    pub observations: Vec<HardwarePixels>,
    pub alignment_review: AlignmentReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelErrorMetrics {
    pub channel: &'static str,
    pub signed_error_min: i32,
    pub signed_error_max: i32,
    pub absolute_error_max: u32,
    pub sum_absolute_error: u64,
    pub sum_squared_error: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunPixelComparison {
    pub run_id: String,
    pub repeat_index: u32,
    pub capture_manifest_sha256: String,
    pub source_capture_sha256: String,
    pub observation_plane_sha256: String,
    pub extraction: ReviewedExtraction,
    pub alignment: AlignmentWindow,
    pub compared_pixel_count: u64,
    pub compared_sample_count: u64,
    pub exact_pixel_count: u64,
    pub exact_sample_count: u64,
    pub channels: Vec<ChannelErrorMetrics>,
    /// SHA-256 of signed RGB residuals encoded as row-major i32 big-endian.
    pub delta_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregatePixelMetrics {
    pub compared_pixel_count: u64,
    pub compared_sample_count: u64,
    pub exact_pixel_count: u64,
    pub exact_sample_count: u64,
    pub channels: Vec<ChannelErrorMetrics>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PixelComparisonReport {
    pub schema: &'static str,
    pub analysis_id: String,
    pub analysis_manifest_sha256: String,
    pub hardware_consensus_sha256: String,
    pub suite_id: String,
    pub vector_id: String,
    pub input_vector_sha256: String,
    pub region: ConsoleRegion,
    pub field: ViField,
    pub signal: AnalogSignal,
    pub filters: ViFilters,
    pub vi_registers: ViRegisters,
    pub sample_domain_id: String,
    pub sample_domain_spec_sha256: String,
    pub hardware_extractor: ProducerIdentity,
    pub reference_plane_sha256: String,
    pub reference_active_output: PixelRectangle,
    pub run_count: usize,
    pub sample_encoding: PixelEncoding,
    pub sample_max: u32,
    pub runs: Vec<RunPixelComparison>,
    pub aggregate: AggregatePixelMetrics,
    /// Binds the ordered run IDs and their spatial residual digests.
    pub cohort_delta_sha256: String,
    pub tolerance_applied: bool,
    pub hardware_parity_claimed: bool,
    pub base_matrix_row_closed: bool,
    pub report_sha256: String,
}

#[derive(Clone)]
struct MetricsAccumulator {
    compared_pixels: u64,
    compared_samples: u64,
    exact_pixels: u64,
    exact_samples: u64,
    channels: [ChannelAccumulator; 3],
}

#[derive(Clone, Copy)]
struct ChannelAccumulator {
    minimum: i32,
    maximum: i32,
    absolute_maximum: u32,
    sum_absolute: u64,
    sum_squared: u64,
}

impl MetricsAccumulator {
    fn new() -> Self {
        Self {
            compared_pixels: 0,
            compared_samples: 0,
            exact_pixels: 0,
            exact_samples: 0,
            channels: [ChannelAccumulator {
                minimum: i32::MAX,
                maximum: i32::MIN,
                absolute_maximum: 0,
                sum_absolute: 0,
                sum_squared: 0,
            }; 3],
        }
    }

    fn record_pixel(&mut self, errors: [i32; 3]) -> Result<(), ValidationError> {
        self.compared_pixels = checked_add(self.compared_pixels, 1, "compared pixel count")?;
        self.compared_samples = checked_add(self.compared_samples, 3, "compared sample count")?;
        if errors == [0; 3] {
            self.exact_pixels = checked_add(self.exact_pixels, 1, "exact pixel count")?;
        }
        for (channel, error) in self.channels.iter_mut().zip(errors) {
            channel.minimum = channel.minimum.min(error);
            channel.maximum = channel.maximum.max(error);
            let absolute = error.unsigned_abs();
            channel.absolute_maximum = channel.absolute_maximum.max(absolute);
            channel.sum_absolute = checked_add(
                channel.sum_absolute,
                u64::from(absolute),
                "sum absolute error",
            )?;
            channel.sum_squared = checked_add(
                channel.sum_squared,
                u64::from(absolute) * u64::from(absolute),
                "sum squared error",
            )?;
            if error == 0 {
                self.exact_samples = checked_add(self.exact_samples, 1, "exact sample count")?;
            }
        }
        Ok(())
    }

    fn merge(&mut self, other: &Self) -> Result<(), ValidationError> {
        self.compared_pixels = checked_add(
            self.compared_pixels,
            other.compared_pixels,
            "aggregate compared pixel count",
        )?;
        self.compared_samples = checked_add(
            self.compared_samples,
            other.compared_samples,
            "aggregate compared sample count",
        )?;
        self.exact_pixels = checked_add(
            self.exact_pixels,
            other.exact_pixels,
            "aggregate exact pixel count",
        )?;
        self.exact_samples = checked_add(
            self.exact_samples,
            other.exact_samples,
            "aggregate exact sample count",
        )?;
        for (channel, source) in self.channels.iter_mut().zip(other.channels) {
            channel.minimum = channel.minimum.min(source.minimum);
            channel.maximum = channel.maximum.max(source.maximum);
            channel.absolute_maximum = channel.absolute_maximum.max(source.absolute_maximum);
            channel.sum_absolute = checked_add(
                channel.sum_absolute,
                source.sum_absolute,
                "aggregate sum absolute error",
            )?;
            channel.sum_squared = checked_add(
                channel.sum_squared,
                source.sum_squared,
                "aggregate sum squared error",
            )?;
        }
        Ok(())
    }

    fn finish(&self) -> AggregatePixelMetrics {
        AggregatePixelMetrics {
            compared_pixel_count: self.compared_pixels,
            compared_sample_count: self.compared_samples,
            exact_pixel_count: self.exact_pixels,
            exact_sample_count: self.exact_samples,
            channels: finish_channels(&self.channels),
        }
    }
}

pub fn analyze_pixel_comparison_file(
    path: &Path,
) -> Result<PixelComparisonReport, ValidationError> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        ValidationError::new(format!(
            "read comparison manifest {}: {error}",
            path.display()
        ))
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(ValidationError::new(format!(
            "comparison manifest {} must be a regular non-symlink file",
            path.display()
        )));
    }
    let mut file = fs::File::open(path).map_err(|error| {
        ValidationError::new(format!(
            "read comparison manifest {}: {error}",
            path.display()
        ))
    })?;
    let metadata = file.metadata().map_err(|error| {
        ValidationError::new(format!(
            "read comparison manifest metadata {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(ValidationError::new(format!(
            "comparison manifest must remain a regular file no larger than {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(|error| {
        ValidationError::new(format!(
            "read comparison manifest {}: {error}",
            path.display()
        ))
    })?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    analyze_pixel_comparison_json(&bytes, base)
}

pub fn analyze_pixel_comparison_json(
    bytes: &[u8],
    artifact_base: &Path,
) -> Result<PixelComparisonReport, ValidationError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ValidationError::new(format!(
            "comparison manifest exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    let manifest: PixelComparisonManifest = serde_json::from_slice(bytes)
        .map_err(|error| ValidationError::new(format!("malformed comparison manifest: {error}")))?;
    analyze_pixel_comparison(manifest, artifact_base)
}

pub fn analyze_pixel_comparison(
    manifest: PixelComparisonManifest,
    artifact_base: &Path,
) -> Result<PixelComparisonReport, ValidationError> {
    if manifest.schema != PIXEL_COMPARISON_SCHEMA {
        return Err(ValidationError::new(format!(
            "unsupported comparison schema {:?}; expected {PIXEL_COMPARISON_SCHEMA:?}",
            manifest.schema
        )));
    }
    text("comparison analysis_id", &manifest.analysis_id)?;
    if manifest.content_class != "synthetic_vi_probe" {
        return Err(ValidationError::new(
            "comparison content_class must be `synthetic_vi_probe`",
        ));
    }
    sha256(
        "comparison expected_consensus_sha256",
        &manifest.expected_consensus_sha256,
    )?;
    text("comparison sample_domain_id", &manifest.sample_domain_id)?;
    validate_artifact(
        "comparison sample_domain_spec",
        &manifest.sample_domain_spec,
        artifact_base,
        Some(MAX_MANIFEST_BYTES as u64),
    )?;
    validate_review(&manifest.alignment_review)?;
    validate_producer(
        "comparison reference producer",
        &manifest.reference.producer,
    )?;
    sha256(
        "comparison reference input_vector_sha256",
        &manifest.reference.input_vector_sha256,
    )?;
    let reference_bytes = validate_plane(
        "comparison reference plane",
        &manifest.reference.plane,
        artifact_base,
    )?;
    validate_rectangle(
        "comparison reference active output",
        &manifest.reference.active_output,
        &manifest.reference.plane,
    )?;

    if manifest.observations.len() < MIN_CLOSURE_RUNS {
        return Err(ValidationError::new(format!(
            "pixel comparison requires at least {MIN_CLOSURE_RUNS} hardware observations"
        )));
    }
    let expected_extractor = &manifest.observations[0].extractor;
    let expected_extraction = &manifest.observations[0].extraction;
    let expected_alignment = &manifest.observations[0].alignment;
    validate_producer("comparison hardware extractor", expected_extractor)?;
    let mut run_ids = BTreeSet::new();
    let mut captures = Vec::<ValidatedCapture>::with_capacity(manifest.observations.len());
    for (index, observation) in manifest.observations.iter().enumerate() {
        let run = index + 1;
        text("comparison observation run_id", &observation.run_id)?;
        if !run_ids.insert(&observation.run_id) {
            return Err(ValidationError::new(format!(
                "comparison observation {run} duplicates run_id {:?}",
                observation.run_id
            )));
        }
        sha256(
            "comparison observation source_capture_sha256",
            &observation.source_capture_sha256,
        )?;
        validate_producer("comparison hardware extractor", &observation.extractor)?;
        if observation.extractor != *expected_extractor {
            return Err(ValidationError::new(format!(
                "comparison observation {run} changes the extraction producer or settings"
            )));
        }
        if observation.extraction != *expected_extraction
            || observation.alignment != *expected_alignment
        {
            return Err(ValidationError::new(format!(
                "comparison observation {run} changes the reviewed extraction or alignment"
            )));
        }
        if observation.plane.encoding != manifest.reference.plane.encoding {
            return Err(ValidationError::new(format!(
                "comparison observation {run} pixel encoding differs from the reference"
            )));
        }
        validate_alignment(
            run,
            &observation.alignment,
            &manifest.reference.plane,
            &observation.plane,
        )?;
        validate_rectangle(
            "comparison hardware active output",
            &observation.extraction.active_output,
            &observation.plane,
        )?;
        validate_active_alignment(
            run,
            &manifest.reference.active_output,
            &observation.extraction.active_output,
            &observation.alignment,
        )?;
        let capture_bytes = validate_artifact(
            "comparison capture manifest",
            &observation.capture_manifest,
            artifact_base,
            Some(MAX_MANIFEST_BYTES as u64),
        )?;
        let capture_path = artifact_base.join(&observation.capture_manifest.path);
        let capture_base = capture_path.parent().unwrap_or(artifact_base);
        let capture = validate_json(&capture_bytes, capture_base)?;
        if capture.manifest().run_id != observation.run_id {
            return Err(ValidationError::new(format!(
                "comparison observation {run} run_id does not match its capture manifest"
            )));
        }
        if capture.receipt().output_artifact_sha256 != observation.source_capture_sha256 {
            return Err(ValidationError::new(format!(
                "comparison observation {run} source_capture_sha256 does not match its validated physical capture"
            )));
        }
        validate_source_window(run, &observation.extraction.source_window, &capture)?;
        validate_plane(
            "comparison hardware plane",
            &observation.plane,
            artifact_base,
        )?;
        captures.push(capture);
    }

    let consensus = validate_hardware_consensus(&captures, MIN_CLOSURE_RUNS)?;
    if consensus.consensus_sha256 != manifest.expected_consensus_sha256 {
        return Err(ValidationError::new(format!(
            "comparison expected consensus {} does not match recomputed hardware cohort {}",
            manifest.expected_consensus_sha256, consensus.consensus_sha256
        )));
    }
    if consensus.input_vector_sha256 != manifest.reference.input_vector_sha256 {
        return Err(ValidationError::new(
            "comparison reference input vector does not match the hardware cohort",
        ));
    }

    let mut aggregate = MetricsAccumulator::new();
    let mut runs = Vec::with_capacity(manifest.observations.len());
    for (observation, capture) in manifest.observations.iter().zip(captures.iter()) {
        // Revalidate and retain one observation at a time. The digest keeps
        // the second read bound to the manifest while avoiding a ten-plane
        // cohort allocation at the 64-MiB per-plane ceiling.
        let plane_bytes = validate_plane(
            "comparison hardware plane",
            &observation.plane,
            artifact_base,
        )?;
        let (metrics, delta_sha256) = compare_window(
            &manifest.reference.plane,
            &reference_bytes,
            &observation.plane,
            &plane_bytes,
            &observation.alignment,
        )?;
        aggregate.merge(&metrics)?;
        runs.push(RunPixelComparison {
            run_id: observation.run_id.clone(),
            repeat_index: capture
                .manifest()
                .digital_input
                .reset_and_repeat
                .repeat_index,
            capture_manifest_sha256: capture.receipt().manifest_sha256.clone(),
            source_capture_sha256: observation.source_capture_sha256.clone(),
            observation_plane_sha256: observation.plane.artifact.sha256.clone(),
            extraction: observation.extraction.clone(),
            alignment: observation.alignment.clone(),
            compared_pixel_count: metrics.compared_pixels,
            compared_sample_count: metrics.compared_samples,
            exact_pixel_count: metrics.exact_pixels,
            exact_sample_count: metrics.exact_samples,
            channels: finish_channels(&metrics.channels),
            delta_sha256,
        });
    }
    runs.sort_by(|left, right| {
        left.capture_manifest_sha256
            .cmp(&right.capture_manifest_sha256)
    });
    let mut cohort_hasher = Sha256::new();
    for run in &runs {
        cohort_hasher.update((run.run_id.len() as u64).to_be_bytes());
        cohort_hasher.update(run.run_id.as_bytes());
        cohort_hasher.update(run.delta_sha256.as_bytes());
    }
    let canonical_manifest = serde_json::to_vec(&manifest)
        .map_err(|error| ValidationError::new(format!("encode comparison manifest: {error}")))?;
    let hardware_extractor = expected_extractor.clone();
    let first_capture = captures
        .first()
        .expect("minimum hardware comparison cohort is nonempty")
        .manifest();
    let mut report = PixelComparisonReport {
        schema: PIXEL_COMPARISON_REPORT_SCHEMA,
        analysis_id: manifest.analysis_id,
        analysis_manifest_sha256: digest(&canonical_manifest),
        hardware_consensus_sha256: consensus.consensus_sha256,
        suite_id: first_capture.suite_id.clone(),
        vector_id: first_capture.digital_input.vector_id.clone(),
        input_vector_sha256: consensus.input_vector_sha256,
        region: first_capture.digital_input.region.clone(),
        field: first_capture.digital_input.field.clone(),
        signal: first_capture.analog_output.signal.clone(),
        filters: first_capture.digital_input.filters.clone(),
        vi_registers: first_capture.digital_input.registers.clone(),
        sample_domain_id: manifest.sample_domain_id,
        sample_domain_spec_sha256: manifest.sample_domain_spec.sha256,
        hardware_extractor,
        reference_plane_sha256: manifest.reference.plane.artifact.sha256,
        reference_active_output: manifest.reference.active_output,
        run_count: runs.len(),
        sample_encoding: manifest.reference.plane.encoding,
        sample_max: manifest.reference.plane.encoding.sample_max(),
        runs,
        aggregate: aggregate.finish(),
        cohort_delta_sha256: format!("{:x}", cohort_hasher.finalize()),
        tolerance_applied: false,
        hardware_parity_claimed: false,
        base_matrix_row_closed: false,
        report_sha256: String::new(),
    };
    let canonical_report = serde_json::to_vec(&report)
        .map_err(|error| ValidationError::new(format!("encode comparison report: {error}")))?;
    report.report_sha256 = digest(&canonical_report);
    Ok(report)
}

fn validate_review(review: &AlignmentReview) -> Result<(), ValidationError> {
    text("comparison alignment reviewer", &review.reviewer)?;
    utc(
        "comparison alignment reviewed_at_utc",
        &review.reviewed_at_utc,
    )?;
    text("comparison alignment method", &review.method)
}

fn validate_producer(label: &str, producer: &ProducerIdentity) -> Result<(), ValidationError> {
    text(&format!("{label} name"), &producer.name)?;
    text(&format!("{label} version"), &producer.version)?;
    sha256(&format!("{label} binary_sha256"), &producer.binary_sha256)?;
    sha256(
        &format!("{label} settings_sha256"),
        &producer.settings_sha256,
    )
}

fn validate_plane(
    label: &str,
    plane: &PixelPlane,
    base: &Path,
) -> Result<Vec<u8>, ValidationError> {
    if plane.width == 0 || plane.height == 0 {
        return Err(ValidationError::new(format!(
            "{label} dimensions must be nonzero"
        )));
    }
    let tight_stride = u64::from(plane.width)
        .checked_mul(3)
        .and_then(|value| value.checked_mul(plane.encoding.bytes_per_sample()))
        .ok_or_else(|| ValidationError::new(format!("{label} row size overflow")))?;
    if u64::from(plane.row_stride_bytes) != tight_stride {
        return Err(ValidationError::new(format!(
            "{label} row_stride_bytes must be tightly packed at {tight_stride}"
        )));
    }
    let expected = tight_stride
        .checked_mul(u64::from(plane.height))
        .ok_or_else(|| ValidationError::new(format!("{label} byte length overflow")))?;
    if plane.artifact.byte_len != expected {
        return Err(ValidationError::new(format!(
            "{label} artifact byte_len {} does not match geometry {expected}",
            plane.artifact.byte_len
        )));
    }
    validate_artifact(label, &plane.artifact, base, Some(MAX_PIXEL_PLANE_BYTES))
}

fn validate_alignment(
    run: usize,
    alignment: &AlignmentWindow,
    reference: &PixelPlane,
    observation: &PixelPlane,
) -> Result<(), ValidationError> {
    if alignment.width == 0 || alignment.height == 0 {
        return Err(ValidationError::new(format!(
            "comparison observation {run} alignment window must be nonzero"
        )));
    }
    let reference_end_x = alignment
        .reference_x
        .checked_add(alignment.width)
        .ok_or_else(|| ValidationError::new("comparison reference X alignment overflow"))?;
    let reference_end_y = alignment
        .reference_y
        .checked_add(alignment.height)
        .ok_or_else(|| ValidationError::new("comparison reference Y alignment overflow"))?;
    let observation_end_x = alignment
        .observation_x
        .checked_add(alignment.width)
        .ok_or_else(|| ValidationError::new("comparison observation X alignment overflow"))?;
    let observation_end_y = alignment
        .observation_y
        .checked_add(alignment.height)
        .ok_or_else(|| ValidationError::new("comparison observation Y alignment overflow"))?;
    if reference_end_x > reference.width
        || reference_end_y > reference.height
        || observation_end_x > observation.width
        || observation_end_y > observation.height
    {
        return Err(ValidationError::new(format!(
            "comparison observation {run} alignment window exceeds a pixel plane"
        )));
    }
    Ok(())
}

fn validate_rectangle(
    label: &str,
    rectangle: &PixelRectangle,
    plane: &PixelPlane,
) -> Result<(), ValidationError> {
    if rectangle.width == 0 || rectangle.height == 0 {
        return Err(ValidationError::new(format!("{label} must be nonempty")));
    }
    let end_x = rectangle
        .x
        .checked_add(rectangle.width)
        .ok_or_else(|| ValidationError::new(format!("{label} X extent overflow")))?;
    let end_y = rectangle
        .y
        .checked_add(rectangle.height)
        .ok_or_else(|| ValidationError::new(format!("{label} Y extent overflow")))?;
    if end_x > plane.width || end_y > plane.height {
        return Err(ValidationError::new(format!(
            "{label} exceeds its pixel plane"
        )));
    }
    Ok(())
}

fn validate_active_alignment(
    run: usize,
    reference: &PixelRectangle,
    observation: &PixelRectangle,
    alignment: &AlignmentWindow,
) -> Result<(), ValidationError> {
    let covers_reference = alignment.reference_x == reference.x
        && alignment.reference_y == reference.y
        && alignment.width == reference.width
        && alignment.height == reference.height;
    let covers_observation = alignment.observation_x == observation.x
        && alignment.observation_y == observation.y
        && alignment.width == observation.width
        && alignment.height == observation.height;
    if !covers_reference || !covers_observation {
        return Err(ValidationError::new(format!(
            "comparison observation {run} alignment must exactly cover both declared active outputs"
        )));
    }
    Ok(())
}

fn validate_source_window(
    run: usize,
    window: &SourceSampleWindow,
    capture: &ValidatedCapture,
) -> Result<(), ValidationError> {
    if window.line_count == 0 || window.samples_per_line == 0 {
        return Err(ValidationError::new(format!(
            "comparison observation {run} source sample window must be nonempty"
        )));
    }
    window
        .first_line
        .checked_add(window.line_count)
        .ok_or_else(|| {
            ValidationError::new(format!(
                "comparison observation {run} source line window overflow"
            ))
        })?;
    window
        .first_sample
        .checked_add(window.samples_per_line)
        .ok_or_else(|| {
            ValidationError::new(format!(
                "comparison observation {run} source sample window overflow"
            ))
        })?;

    let analog = &capture.manifest().analog_output;
    let field_end = analog
        .first_field
        .checked_add(analog.field_count)
        .ok_or_else(|| ValidationError::new("analog captured field range overflow"))?;
    if window.field_number < analog.first_field || window.field_number >= field_end {
        return Err(ValidationError::new(format!(
            "comparison observation {run} source field {} is outside captured range {}..{}",
            window.field_number, analog.first_field, field_end
        )));
    }
    let field_matches = match capture.manifest().digital_input.field {
        ViField::Progressive => true,
        ViField::InterlacedEven => window.field_number.is_multiple_of(2),
        ViField::InterlacedOdd => !window.field_number.is_multiple_of(2),
    };
    if !field_matches {
        return Err(ValidationError::new(format!(
            "comparison observation {run} source field parity disagrees with the programmed VI field"
        )));
    }
    Ok(())
}

fn compare_window(
    reference_plane: &PixelPlane,
    reference: &[u8],
    observation_plane: &PixelPlane,
    observation: &[u8],
    alignment: &AlignmentWindow,
) -> Result<(MetricsAccumulator, String), ValidationError> {
    let mut metrics = MetricsAccumulator::new();
    let mut delta = Sha256::new();
    for y in 0..alignment.height {
        for x in 0..alignment.width {
            let reference_offset = pixel_offset(
                reference_plane,
                alignment.reference_x + x,
                alignment.reference_y + y,
            )?;
            let observation_offset = pixel_offset(
                observation_plane,
                alignment.observation_x + x,
                alignment.observation_y + y,
            )?;
            let mut errors = [0i32; 3];
            for (channel, error) in errors.iter_mut().enumerate() {
                let expected = sample(reference_plane, reference, reference_offset, channel)?;
                let actual = sample(observation_plane, observation, observation_offset, channel)?;
                *error = actual as i32 - expected as i32;
                delta.update(error.to_be_bytes());
            }
            metrics.record_pixel(errors)?;
        }
    }
    Ok((metrics, format!("{:x}", delta.finalize())))
}

fn pixel_offset(plane: &PixelPlane, x: u32, y: u32) -> Result<usize, ValidationError> {
    let bytes_per_pixel = 3u64 * plane.encoding.bytes_per_sample();
    let offset = u64::from(y)
        .checked_mul(u64::from(plane.row_stride_bytes))
        .and_then(|value| value.checked_add(u64::from(x) * bytes_per_pixel))
        .ok_or_else(|| ValidationError::new("comparison pixel offset overflow"))?;
    usize::try_from(offset)
        .map_err(|_| ValidationError::new("comparison pixel offset exceeds host usize"))
}

fn sample(
    plane: &PixelPlane,
    bytes: &[u8],
    pixel_offset: usize,
    channel: usize,
) -> Result<u32, ValidationError> {
    match plane.encoding {
        PixelEncoding::Rgb8 => bytes
            .get(pixel_offset + channel)
            .copied()
            .map(u32::from)
            .ok_or_else(|| ValidationError::new("comparison RGB8 sample exceeds plane")),
        PixelEncoding::Rgb16BigEndian => {
            let offset = pixel_offset + channel * 2;
            let pair = bytes
                .get(offset..offset + 2)
                .ok_or_else(|| ValidationError::new("comparison RGB16 sample exceeds plane"))?;
            Ok(u16::from_be_bytes([pair[0], pair[1]]).into())
        }
    }
}

fn finish_channels(channels: &[ChannelAccumulator; 3]) -> Vec<ChannelErrorMetrics> {
    ["r", "g", "b"]
        .into_iter()
        .zip(channels)
        .map(|(channel, value)| ChannelErrorMetrics {
            channel,
            signed_error_min: value.minimum,
            signed_error_max: value.maximum,
            absolute_error_max: value.absolute_maximum,
            sum_absolute_error: value.sum_absolute,
            sum_squared_error: value.sum_squared,
        })
        .collect()
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64, ValidationError> {
    left.checked_add(right)
        .ok_or_else(|| ValidationError::new(format!("comparison {label} overflow")))
}
