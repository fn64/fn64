use super::{
    decode_blob, digest, sha256, text, validate_framebuffer_rdram_bounds, validate_vector, Blob,
    ConsoleRegion, DigitalFramebuffer, DigitalVector, FramebufferEncoding, ResetKind,
    ValidationError, VectorFramebuffer, ViField, ViFilters, ViPixelType, ViRegisters,
    DIGITAL_VECTOR_SCHEMA, MAX_INPUT_VECTOR_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const DIGITAL_BOUNDARY_CAPTURE_SCHEMA: &str = "fn64.vi-digital-boundary-capture.v1";
pub const DIGITAL_BOUNDARY_ANALYSIS_SCHEMA: &str = "fn64.vi-digital-boundary-analysis.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalBoundaryProducerKind {
    SyntheticFixture,
    BlackBoxObservation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryProducer {
    pub kind: DigitalBoundaryProducerKind,
    pub name: String,
    pub version: String,
    pub platform: String,
    pub producer_binary_sha256: String,
    pub settings_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalBoundaryAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalBoundaryEdge {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalBoundaryPosition {
    Before,
    On,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalBorderSide {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalInterlacedLineSpan {
    OneLine,
    TwoLines,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DigitalBoundaryPointIntent {
    ActiveWindowBoundary {
        axis: DigitalBoundaryAxis,
        edge: DigitalBoundaryEdge,
        position: DigitalBoundaryPosition,
        boundary_coordinate_i32: i32,
        sample_coordinate_i32: i32,
    },
    BorderFetchBoundary {
        side: DigitalBorderSide,
        position: DigitalBoundaryPosition,
        boundary_coordinate_i32: i32,
        sample_coordinate_i32: i32,
    },
    InsufficientThreeSampleNeighborhood {
        axis: DigitalBoundaryAxis,
        edge: DigitalBoundaryEdge,
        available_samples_u8: u8,
    },
    PartialCoverageAaCentroidCandidate {
        candidate_sample_u3: u8,
        candidate_x_q2_i16: i16,
        candidate_y_q2_i16: i16,
        coverage_mask_u8: u8,
        coverage_count_u4: u8,
    },
    InterlacedLinePhase {
        field: ViField,
        line_span: DigitalInterlacedLineSpan,
        phase_origin_line_i32: i32,
        sample_line_i32: i32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryViProfile {
    pub profile_id: String,
    pub registers: ViRegisters,
    pub filters: ViFilters,
    pub region: ConsoleRegion,
    pub field: ViField,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundarySourceGeometry {
    pub encoding: FramebufferEncoding,
    pub width: u32,
    pub height: u32,
    pub row_stride_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalPostViEncoding {
    Rgb8,
    Rgba8,
}

impl DigitalPostViEncoding {
    fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalPostViGeometry {
    pub encoding: DigitalPostViEncoding,
    pub width: u32,
    pub height: u32,
    pub row_stride_bytes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryControls {
    pub reset_sequence_id: String,
    pub retrace_sequence_id: String,
    pub progressive_profile_id: String,
    pub interlaced_even_profile_id: String,
    pub interlaced_odd_profile_id: String,
    pub profiles: Vec<DigitalBoundaryViProfile>,
    pub source_geometry: DigitalBoundarySourceGeometry,
    pub post_vi_geometry: DigitalPostViGeometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryTimingProvenance {
    pub replay_from_reset: bool,
    pub reset_kind: ResetKind,
    pub reset_event_id_sha256: String,
    pub repeat_index: u32,
    pub retrace_event_id_sha256: String,
    pub retrace_index: u32,
    pub observed_field: ViField,
    pub observed_current: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryCase {
    pub case_id: String,
    pub description: String,
    pub profile_id: String,
    pub intent: DigitalBoundaryPointIntent,
    pub timing: DigitalBoundaryTimingProvenance,
    pub source_framebuffer_contents: Blob,
    pub source_coverage_counts: Blob,
    pub post_vi_output_contents: Blob,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryCaptureBundle {
    pub schema: String,
    pub sweep_id: String,
    pub content_class: String,
    pub producer: DigitalBoundaryProducer,
    pub controls: DigitalBoundaryControls,
    pub cases: Vec<DigitalBoundaryCase>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalPostViPlane {
    pub geometry: DigitalPostViGeometry,
    pub contents: Blob,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryObservation {
    pub case_id: String,
    pub profile_id: String,
    pub intent: DigitalBoundaryPointIntent,
    pub timing: DigitalBoundaryTimingProvenance,
    pub source_framebuffer: VectorFramebuffer,
    pub post_vi_output: DigitalPostViPlane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitalBoundaryEvidenceStatus {
    NonParityCaptureEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalBoundaryAnalysis {
    pub schema: &'static str,
    pub bundle_sha256: String,
    pub analysis_sha256: String,
    pub sweep_id: String,
    pub producer: DigitalBoundaryProducer,
    pub controls: DigitalBoundaryControls,
    pub observations: Vec<DigitalBoundaryObservation>,
    pub evidence_status: DigitalBoundaryEvidenceStatus,
    pub parity_claimed: bool,
    pub base_matrix_row_closed: bool,
}

pub fn analyze_digital_boundary_file(
    path: &std::path::Path,
) -> Result<DigitalBoundaryAnalysis, ValidationError> {
    let bytes = std::fs::read(path).map_err(|error| {
        ValidationError::new(format!(
            "read digital boundary bundle {}: {error}",
            path.display()
        ))
    })?;
    analyze_digital_boundary_json(&bytes)
}

pub fn analyze_digital_boundary_json(
    bytes: &[u8],
) -> Result<DigitalBoundaryAnalysis, ValidationError> {
    if bytes.len() > MAX_INPUT_VECTOR_BYTES {
        return Err(ValidationError::new(format!(
            "digital boundary bundle exceeds {MAX_INPUT_VECTOR_BYTES} bytes"
        )));
    }
    let bundle: DigitalBoundaryCaptureBundle = serde_json::from_slice(bytes).map_err(|error| {
        ValidationError::new(format!("malformed digital boundary bundle: {error}"))
    })?;
    validate_and_analyze(bundle)
}

fn validate_and_analyze(
    bundle: DigitalBoundaryCaptureBundle,
) -> Result<DigitalBoundaryAnalysis, ValidationError> {
    if bundle.schema != DIGITAL_BOUNDARY_CAPTURE_SCHEMA {
        return Err(ValidationError::new(format!(
            "unsupported digital boundary schema {:?}; expected {DIGITAL_BOUNDARY_CAPTURE_SCHEMA:?}",
            bundle.schema
        )));
    }
    text("digital boundary sweep_id", &bundle.sweep_id)?;
    if bundle.content_class != "synthetic_vi_probe" {
        return Err(ValidationError::new(
            "digital boundary content_class must be `synthetic_vi_probe`",
        ));
    }
    for (label, value) in [
        ("digital boundary producer name", &bundle.producer.name),
        (
            "digital boundary producer version",
            &bundle.producer.version,
        ),
        (
            "digital boundary producer platform",
            &bundle.producer.platform,
        ),
        (
            "digital boundary reset_sequence_id",
            &bundle.controls.reset_sequence_id,
        ),
        (
            "digital boundary retrace_sequence_id",
            &bundle.controls.retrace_sequence_id,
        ),
    ] {
        text(label, value)?;
    }
    sha256(
        "digital boundary producer_binary_sha256",
        &bundle.producer.producer_binary_sha256,
    )?;
    sha256(
        "digital boundary settings_sha256",
        &bundle.producer.settings_sha256,
    )?;
    validate_geometries(&bundle.controls)?;

    let profiles = validate_profiles(&bundle.controls)?;
    let progressive_id = &bundle.controls.progressive_profile_id;
    let even_id = &bundle.controls.interlaced_even_profile_id;
    let odd_id = &bundle.controls.interlaced_odd_profile_id;
    let mut observations = BTreeMap::<String, DigitalBoundaryObservation>::new();
    let mut case_ids = BTreeSet::new();
    let mut reset_ids = BTreeSet::new();
    let mut retrace_ids = BTreeSet::new();
    let mut repeat_indices = BTreeSet::new();
    let mut active_boundaries = BTreeMap::new();
    let mut border_boundaries = BTreeMap::new();
    let mut centroid_coordinates = BTreeSet::new();
    let mut interlace_origins = BTreeMap::new();

    for case in &bundle.cases {
        text("digital boundary case_id", &case.case_id)?;
        text("digital boundary case description", &case.description)?;
        if !case_ids.insert(case.case_id.clone()) {
            return Err(ValidationError::new(format!(
                "duplicate digital boundary case_id {:?}",
                case.case_id
            )));
        }
        let profile = profiles.get(&case.profile_id).ok_or_else(|| {
            ValidationError::new(format!(
                "case {:?}: unknown VI profile {:?}",
                case.case_id, case.profile_id
            ))
        })?;
        validate_timing(
            case,
            profile,
            &mut reset_ids,
            &mut retrace_ids,
            &mut repeat_indices,
        )?;
        let key = validate_intent(
            case,
            profile,
            progressive_id,
            even_id,
            odd_id,
            &mut active_boundaries,
            &mut border_boundaries,
            &mut centroid_coordinates,
            &mut interlace_origins,
        )?;
        let source = source_framebuffer(&bundle.controls, case);
        let vector = DigitalVector {
            schema: DIGITAL_VECTOR_SCHEMA.to_owned(),
            vector_id: case.case_id.clone(),
            content_class: "synthetic_vi_probe".to_owned(),
            framebuffer: source.clone(),
            registers: profile.registers.clone(),
            filters: profile.filters.clone(),
            region: profile.region.clone(),
            field: profile.field.clone(),
        };
        validate_vector(&vector)
            .map_err(|error| ValidationError::new(format!("case {:?}: {error}", case.case_id)))?;
        let digital = DigitalFramebuffer {
            encoding: bundle.controls.source_geometry.encoding.clone(),
            width: bundle.controls.source_geometry.width,
            height: bundle.controls.source_geometry.height,
            row_stride_bytes: bundle.controls.source_geometry.row_stride_bytes,
            framebuffer_sha256: case.source_framebuffer_contents.sha256.clone(),
        };
        validate_framebuffer_rdram_bounds(&digital, profile.registers.origin)?;
        validate_output(
            &bundle.controls.post_vi_geometry,
            &case.post_vi_output_contents,
        )
        .map_err(|error| ValidationError::new(format!("case {:?}: {error}", case.case_id)))?;
        let observation = DigitalBoundaryObservation {
            case_id: case.case_id.clone(),
            profile_id: case.profile_id.clone(),
            intent: case.intent.clone(),
            timing: case.timing.clone(),
            source_framebuffer: source,
            post_vi_output: DigitalPostViPlane {
                geometry: bundle.controls.post_vi_geometry.clone(),
                contents: case.post_vi_output_contents.clone(),
            },
        };
        if observations.insert(key.clone(), observation).is_some() {
            return Err(ValidationError::new(format!(
                "duplicate digital boundary matrix point {key}"
            )));
        }
    }

    let expected = expected_keys();
    for key in &expected {
        if !observations.contains_key(key) {
            return Err(ValidationError::new(format!(
                "digital boundary sweep {:?} is missing matrix point {key}",
                bundle.sweep_id
            )));
        }
    }
    if observations.len() != expected.len() {
        return Err(ValidationError::new(format!(
            "digital boundary sweep {:?} contains a point outside the required matrix",
            bundle.sweep_id
        )));
    }
    let expected_repeat_indices = (0..u32::try_from(expected.len())
        .expect("fixed digital boundary matrix fits u32"))
        .collect::<BTreeSet<_>>();
    if repeat_indices != expected_repeat_indices {
        return Err(ValidationError::new(format!(
            "digital boundary repeat_index values must exactly cover 0..{}",
            expected.len()
        )));
    }
    let observations = expected
        .iter()
        .map(|key| {
            observations
                .remove(key)
                .expect("completeness checked every required key")
        })
        .collect::<Vec<_>>();

    let canonical_bundle = serde_json::to_vec(&bundle).map_err(|error| {
        ValidationError::new(format!("canonicalize digital boundary bundle: {error}"))
    })?;
    let bundle_sha256 = digest(&canonical_bundle);
    #[derive(Serialize)]
    struct HashInput<'a> {
        schema: &'static str,
        bundle_sha256: &'a str,
        sweep_id: &'a str,
        producer: &'a DigitalBoundaryProducer,
        controls: &'a DigitalBoundaryControls,
        observations: &'a [DigitalBoundaryObservation],
        evidence_status: DigitalBoundaryEvidenceStatus,
        parity_claimed: bool,
        base_matrix_row_closed: bool,
    }
    let hash_input = HashInput {
        schema: DIGITAL_BOUNDARY_ANALYSIS_SCHEMA,
        bundle_sha256: &bundle_sha256,
        sweep_id: &bundle.sweep_id,
        producer: &bundle.producer,
        controls: &bundle.controls,
        observations: &observations,
        evidence_status: DigitalBoundaryEvidenceStatus::NonParityCaptureEnvelope,
        parity_claimed: false,
        base_matrix_row_closed: false,
    };
    let canonical_analysis = serde_json::to_vec(&hash_input).map_err(|error| {
        ValidationError::new(format!("canonicalize digital boundary analysis: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.vi-digital-boundary-analysis.v1\0");
    hasher.update(canonical_analysis);
    let analysis_sha256 = format!("{:x}", hasher.finalize());

    Ok(DigitalBoundaryAnalysis {
        schema: DIGITAL_BOUNDARY_ANALYSIS_SCHEMA,
        bundle_sha256,
        analysis_sha256,
        sweep_id: bundle.sweep_id,
        producer: bundle.producer,
        controls: bundle.controls,
        observations,
        evidence_status: DigitalBoundaryEvidenceStatus::NonParityCaptureEnvelope,
        parity_claimed: false,
        base_matrix_row_closed: false,
    })
}

fn validate_geometries(controls: &DigitalBoundaryControls) -> Result<(), ValidationError> {
    let source = &controls.source_geometry;
    if source.width == 0 || source.height == 0 {
        return Err(ValidationError::new(
            "digital boundary source geometry must be nonzero",
        ));
    }
    let source_minimum = source
        .width
        .checked_mul(source.encoding.bytes_per_pixel())
        .ok_or_else(|| ValidationError::new("digital boundary source row size overflow"))?;
    if source.row_stride_bytes < source_minimum {
        return Err(ValidationError::new(
            "digital boundary source row stride is too small",
        ));
    }
    let output = &controls.post_vi_geometry;
    if output.width == 0 || output.height == 0 {
        return Err(ValidationError::new(
            "digital boundary post-VI geometry must be nonzero",
        ));
    }
    let output_minimum = output
        .width
        .checked_mul(output.encoding.bytes_per_pixel())
        .ok_or_else(|| ValidationError::new("digital boundary post-VI row size overflow"))?;
    if output.row_stride_bytes < output_minimum {
        return Err(ValidationError::new(
            "digital boundary post-VI row stride is too small",
        ));
    }
    Ok(())
}

fn validate_profiles(
    controls: &DigitalBoundaryControls,
) -> Result<BTreeMap<String, &DigitalBoundaryViProfile>, ValidationError> {
    let expected_ids = [
        &controls.progressive_profile_id,
        &controls.interlaced_even_profile_id,
        &controls.interlaced_odd_profile_id,
    ];
    if expected_ids
        .iter()
        .any(|id| text("VI profile_id", id).is_err())
    {
        return Err(ValidationError::new(
            "digital boundary VI profile identifiers must be nonempty printable text",
        ));
    }
    if expected_ids.into_iter().collect::<BTreeSet<_>>().len() != 3 {
        return Err(ValidationError::new(
            "digital boundary progressive/even/odd profile identifiers must be distinct",
        ));
    }
    let mut profiles = BTreeMap::new();
    for profile in &controls.profiles {
        text("digital boundary VI profile_id", &profile.profile_id)?;
        validate_profile(profile, &controls.source_geometry)?;
        if profiles
            .insert(profile.profile_id.clone(), profile)
            .is_some()
        {
            return Err(ValidationError::new(format!(
                "duplicate digital boundary VI profile {:?}",
                profile.profile_id
            )));
        }
    }
    if profiles.len() != 3
        || profiles.keys().collect::<BTreeSet<_>>() != expected_ids.into_iter().collect()
    {
        return Err(ValidationError::new(
            "digital boundary controls must contain exactly the declared progressive/even/odd profiles",
        ));
    }
    if profiles[&controls.progressive_profile_id].field != ViField::Progressive
        || profiles[&controls.interlaced_even_profile_id].field != ViField::InterlacedEven
        || profiles[&controls.interlaced_odd_profile_id].field != ViField::InterlacedOdd
    {
        return Err(ValidationError::new(
            "digital boundary profile field identities do not match their declared roles",
        ));
    }
    Ok(profiles)
}

fn validate_profile(
    profile: &DigitalBoundaryViProfile,
    geometry: &DigitalBoundarySourceGeometry,
) -> Result<(), ValidationError> {
    if profile.registers.width == 0 || profile.registers.width != geometry.width {
        return Err(ValidationError::new(format!(
            "VI profile {:?} width must match source geometry",
            profile.profile_id
        )));
    }
    let status = profile.registers.status;
    let pixel_type = match status & 3 {
        0 => ViPixelType::Blank,
        1 => {
            return Err(ValidationError::new(format!(
                "VI profile {:?} selects reserved pixel type 1",
                profile.profile_id
            )))
        }
        2 => ViPixelType::Rgba16,
        3 => ViPixelType::Rgba32,
        _ => unreachable!(),
    };
    let encoding_matches = matches!(
        (&pixel_type, &geometry.encoding),
        (ViPixelType::Blank, _)
            | (ViPixelType::Rgba16, FramebufferEncoding::Rgba16BigEndian)
            | (ViPixelType::Rgba32, FramebufferEncoding::Rgba32BigEndian)
    );
    if !encoding_matches
        || profile.filters.pixel_type != pixel_type
        || profile.filters.gamma != (status & (1 << 3) != 0)
        || profile.filters.gamma_dither != (status & (1 << 2) != 0)
        || profile.filters.divot != (status & (1 << 4) != 0)
        || profile.filters.dither_filter != (status & (1 << 16) != 0)
    {
        return Err(ValidationError::new(format!(
            "VI profile {:?} encoding/filter controls disagree with STATUS",
            profile.profile_id
        )));
    }
    let serrate = status & (1 << 6) != 0;
    match profile.field {
        ViField::Progressive if !serrate => {}
        ViField::InterlacedEven if serrate && profile.registers.current & 1 == 0 => {}
        ViField::InterlacedOdd if serrate && profile.registers.current & 1 == 1 => {}
        _ => {
            return Err(ValidationError::new(format!(
                "VI profile {:?} field disagrees with STATUS/CURRENT",
                profile.profile_id
            )))
        }
    }
    Ok(())
}

fn validate_timing(
    case: &DigitalBoundaryCase,
    profile: &DigitalBoundaryViProfile,
    reset_ids: &mut BTreeSet<String>,
    retrace_ids: &mut BTreeSet<String>,
    repeat_indices: &mut BTreeSet<u32>,
) -> Result<(), ValidationError> {
    let timing = &case.timing;
    if !timing.replay_from_reset || timing.reset_kind != ResetKind::PowerCycle {
        return Err(ValidationError::new(format!(
            "case {:?}: digital boundary point must replay from power_cycle",
            case.case_id
        )));
    }
    sha256(
        "digital boundary reset_event_id_sha256",
        &timing.reset_event_id_sha256,
    )?;
    sha256(
        "digital boundary retrace_event_id_sha256",
        &timing.retrace_event_id_sha256,
    )?;
    if !reset_ids.insert(timing.reset_event_id_sha256.clone()) {
        return Err(ValidationError::new(format!(
            "case {:?}: duplicate reset event identity",
            case.case_id
        )));
    }
    if !retrace_ids.insert(timing.retrace_event_id_sha256.clone()) {
        return Err(ValidationError::new(format!(
            "case {:?}: duplicate retrace event identity",
            case.case_id
        )));
    }
    if !repeat_indices.insert(timing.repeat_index) {
        return Err(ValidationError::new(format!(
            "case {:?}: duplicate repeat_index {}",
            case.case_id, timing.repeat_index
        )));
    }
    if timing.retrace_index == 0 {
        return Err(ValidationError::new(format!(
            "case {:?}: retrace_index must be nonzero",
            case.case_id
        )));
    }
    if timing.observed_field != profile.field
        || timing.observed_current != profile.registers.current
    {
        return Err(ValidationError::new(format!(
            "case {:?}: observed field/CURRENT provenance differs from VI profile",
            case.case_id
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_intent(
    case: &DigitalBoundaryCase,
    profile: &DigitalBoundaryViProfile,
    progressive_id: &str,
    even_id: &str,
    odd_id: &str,
    active_boundaries: &mut BTreeMap<(DigitalBoundaryAxis, DigitalBoundaryEdge), i32>,
    border_boundaries: &mut BTreeMap<DigitalBorderSide, i32>,
    centroid_coordinates: &mut BTreeSet<(i16, i16)>,
    interlace_origins: &mut BTreeMap<DigitalInterlacedLineSpan, i32>,
) -> Result<String, ValidationError> {
    let require_progressive = |case: &DigitalBoundaryCase| {
        if case.profile_id != progressive_id || profile.field != ViField::Progressive {
            Err(ValidationError::new(format!(
                "case {:?}: non-interlaced boundary points must use the declared progressive profile",
                case.case_id
            )))
        } else {
            Ok(())
        }
    };
    match &case.intent {
        DigitalBoundaryPointIntent::ActiveWindowBoundary {
            axis,
            edge,
            position,
            boundary_coordinate_i32,
            sample_coordinate_i32,
        } => {
            require_progressive(case)?;
            validate_adjacent_coordinate(
                "active-window",
                *position,
                *boundary_coordinate_i32,
                *sample_coordinate_i32,
            )?;
            if active_boundaries
                .insert((*axis, *edge), *boundary_coordinate_i32)
                .is_some_and(|prior| prior != *boundary_coordinate_i32)
            {
                return Err(ValidationError::new(format!(
                    "case {:?}: active-window boundary coordinate drifts within its group",
                    case.case_id
                )));
            }
            Ok(format!("active:{axis:?}:{edge:?}:{position:?}"))
        }
        DigitalBoundaryPointIntent::BorderFetchBoundary {
            side,
            position,
            boundary_coordinate_i32,
            sample_coordinate_i32,
        } => {
            require_progressive(case)?;
            validate_adjacent_coordinate(
                "border-fetch",
                *position,
                *boundary_coordinate_i32,
                *sample_coordinate_i32,
            )?;
            if border_boundaries
                .insert(*side, *boundary_coordinate_i32)
                .is_some_and(|prior| prior != *boundary_coordinate_i32)
            {
                return Err(ValidationError::new(format!(
                    "case {:?}: border-fetch boundary coordinate drifts within its group",
                    case.case_id
                )));
            }
            Ok(format!("border:{side:?}:{position:?}"))
        }
        DigitalBoundaryPointIntent::InsufficientThreeSampleNeighborhood {
            axis,
            edge,
            available_samples_u8,
        } => {
            require_progressive(case)?;
            if !(1..=2).contains(available_samples_u8) {
                return Err(ValidationError::new(format!(
                    "case {:?}: insufficient three-sample neighborhood must declare one or two available samples",
                    case.case_id
                )));
            }
            Ok(format!(
                "neighborhood:{axis:?}:{edge:?}:{available_samples_u8}"
            ))
        }
        DigitalBoundaryPointIntent::PartialCoverageAaCentroidCandidate {
            candidate_sample_u3,
            candidate_x_q2_i16,
            candidate_y_q2_i16,
            coverage_mask_u8,
            coverage_count_u4,
        } => {
            require_progressive(case)?;
            if *candidate_sample_u3 > 7 {
                return Err(ValidationError::new(format!(
                    "case {:?}: centroid candidate sample must be in 0..=7",
                    case.case_id
                )));
            }
            if *coverage_mask_u8 == 0 || *coverage_mask_u8 == u8::MAX {
                return Err(ValidationError::new(format!(
                    "case {:?}: centroid candidate requires a partial nonzero coverage mask",
                    case.case_id
                )));
            }
            let count = coverage_mask_u8.count_ones() as u8;
            if *coverage_count_u4 != count {
                return Err(ValidationError::new(format!(
                    "case {:?}: centroid mask count is {count}, not declared {coverage_count_u4}",
                    case.case_id
                )));
            }
            if !centroid_coordinates.insert((*candidate_x_q2_i16, *candidate_y_q2_i16)) {
                return Err(ValidationError::new(format!(
                    "case {:?}: centroid candidate coordinates are not unique",
                    case.case_id
                )));
            }
            Ok(format!("centroid:{candidate_sample_u3}"))
        }
        DigitalBoundaryPointIntent::InterlacedLinePhase {
            field,
            line_span,
            phase_origin_line_i32,
            sample_line_i32,
        } => {
            let expected_profile = match field {
                ViField::InterlacedEven => even_id,
                ViField::InterlacedOdd => odd_id,
                ViField::Progressive => {
                    return Err(ValidationError::new(format!(
                        "case {:?}: interlaced phase cannot declare progressive field",
                        case.case_id
                    )))
                }
            };
            if case.profile_id != expected_profile || profile.field != *field {
                return Err(ValidationError::new(format!(
                    "case {:?}: interlaced phase uses the wrong field profile",
                    case.case_id
                )));
            }
            let offset = match line_span {
                DigitalInterlacedLineSpan::OneLine => 1,
                DigitalInterlacedLineSpan::TwoLines => 2,
            };
            let expected_line = phase_origin_line_i32.checked_add(offset).ok_or_else(|| {
                ValidationError::new(format!(
                    "case {:?}: interlaced phase line overflows",
                    case.case_id
                ))
            })?;
            if *sample_line_i32 != expected_line {
                return Err(ValidationError::new(format!(
                    "case {:?}: interlaced {line_span:?} phase sample must be line {expected_line}",
                    case.case_id
                )));
            }
            if interlace_origins
                .insert(*line_span, *phase_origin_line_i32)
                .is_some_and(|prior| prior != *phase_origin_line_i32)
            {
                return Err(ValidationError::new(format!(
                    "case {:?}: interlaced phase origin drifts across fields",
                    case.case_id
                )));
            }
            Ok(format!("interlace:{field:?}:{line_span:?}"))
        }
    }
}

fn validate_adjacent_coordinate(
    label: &str,
    position: DigitalBoundaryPosition,
    boundary: i32,
    sample: i32,
) -> Result<(), ValidationError> {
    let expected = match position {
        DigitalBoundaryPosition::Before => boundary.checked_sub(1),
        DigitalBoundaryPosition::On => Some(boundary),
        DigitalBoundaryPosition::After => boundary.checked_add(1),
    }
    .ok_or_else(|| ValidationError::new(format!("{label} boundary neighbor overflows")))?;
    if sample != expected {
        return Err(ValidationError::new(format!(
            "{label} {position:?} sample must be exactly {expected}"
        )));
    }
    Ok(())
}

fn source_framebuffer(
    controls: &DigitalBoundaryControls,
    case: &DigitalBoundaryCase,
) -> VectorFramebuffer {
    VectorFramebuffer {
        encoding: controls.source_geometry.encoding.clone(),
        width: controls.source_geometry.width,
        height: controls.source_geometry.height,
        row_stride_bytes: controls.source_geometry.row_stride_bytes,
        contents: case.source_framebuffer_contents.clone(),
        coverage_counts: case.source_coverage_counts.clone(),
    }
}

fn validate_output(
    geometry: &DigitalPostViGeometry,
    contents: &Blob,
) -> Result<(), ValidationError> {
    let bytes = decode_blob("digital boundary post-VI output", contents)?;
    let expected = u64::from(geometry.row_stride_bytes)
        .checked_mul(u64::from(geometry.height))
        .ok_or_else(|| ValidationError::new("digital boundary post-VI length overflow"))?;
    if bytes.len() as u64 != expected {
        return Err(ValidationError::new(format!(
            "digital boundary post-VI byte length {} does not match geometry {expected}",
            bytes.len()
        )));
    }
    Ok(())
}

fn expected_keys() -> Vec<String> {
    let mut keys = Vec::with_capacity(44);
    for axis in [
        DigitalBoundaryAxis::Horizontal,
        DigitalBoundaryAxis::Vertical,
    ] {
        for edge in [DigitalBoundaryEdge::Start, DigitalBoundaryEdge::End] {
            for position in [
                DigitalBoundaryPosition::Before,
                DigitalBoundaryPosition::On,
                DigitalBoundaryPosition::After,
            ] {
                keys.push(format!("active:{axis:?}:{edge:?}:{position:?}"));
            }
        }
    }
    for side in [
        DigitalBorderSide::Left,
        DigitalBorderSide::Right,
        DigitalBorderSide::Top,
        DigitalBorderSide::Bottom,
    ] {
        for position in [
            DigitalBoundaryPosition::Before,
            DigitalBoundaryPosition::On,
            DigitalBoundaryPosition::After,
        ] {
            keys.push(format!("border:{side:?}:{position:?}"));
        }
    }
    for axis in [
        DigitalBoundaryAxis::Horizontal,
        DigitalBoundaryAxis::Vertical,
    ] {
        for edge in [DigitalBoundaryEdge::Start, DigitalBoundaryEdge::End] {
            for available in 1..=2 {
                keys.push(format!("neighborhood:{axis:?}:{edge:?}:{available}"));
            }
        }
    }
    for candidate in 0..8 {
        keys.push(format!("centroid:{candidate}"));
    }
    for field in [ViField::InterlacedEven, ViField::InterlacedOdd] {
        for line_span in [
            DigitalInterlacedLineSpan::OneLine,
            DigitalInterlacedLineSpan::TwoLines,
        ] {
            keys.push(format!("interlace:{field:?}:{line_span:?}"));
        }
    }
    keys
}
