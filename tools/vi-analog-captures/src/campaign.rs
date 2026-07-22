use super::{
    digest, generate_digital_vector_corpus, text, validate_artifact, AnalogSignal, ArtifactRef,
    ConsoleRegion, CorpusObjective, ValidationError, MIN_CLOSURE_RUNS,
};
use serde::Serialize;
use std::path::Path;

pub const CAMPAIGN_PLAN_SCHEMA: &str = "fn64.vi-analog-campaign-plan.v2";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignEvidenceStatus {
    PlannedNotCaptured,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignRequirement {
    ConsoleClass,
    ConsoleUnitIdSha256,
    MotherboardRevision,
    VideoEncoderRevision,
    ModificationState,
    Operator,
    RecordedAtUtc,
    ResetEventIdSha256,
    CaptureDeviceManufacturer,
    CaptureDeviceModel,
    CaptureDeviceUnitIdSha256,
    CaptureDeviceFirmware,
    Cable,
    TerminationOhms,
    SampleRateHz,
    CaptureEncoding,
    CaptureToolName,
    CaptureToolVersion,
    CaptureToolBinarySha256,
    CaptureSettingsSha256,
    FirstField,
    FieldCount,
    CaptureArtifactPath,
    CaptureArtifactByteLen,
    CaptureArtifactSha256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignCorpus {
    pub corpus_id: String,
    pub region: ConsoleRegion,
    pub index_artifact: ArtifactRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignVector {
    pub vector_id: String,
    pub vector_artifact: ArtifactRef,
    pub objectives: Vec<CorpusObjective>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedCaptureRun {
    pub run_index: u32,
    pub repeat_index: u32,
    pub run_id: String,
    pub signal: AnalogSignal,
    pub manifest_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureCampaignPlan {
    pub schema: &'static str,
    pub evidence_status: CampaignEvidenceStatus,
    pub capture_manifests_emitted: bool,
    pub campaign_id: String,
    pub suite_id: String,
    pub content_class: &'static str,
    pub corpus: CampaignCorpus,
    pub selected_vector: CampaignVector,
    pub signal: AnalogSignal,
    pub reset_kind: &'static str,
    pub reset_sequence_id: String,
    pub run_count: u32,
    pub runs: Vec<PlannedCaptureRun>,
    pub required_hardware_provenance: Vec<CampaignRequirement>,
    pub required_capture_chain: Vec<CampaignRequirement>,
    pub required_per_run_observation: Vec<CampaignRequirement>,
    pub plan_sha256: String,
}

/// Builds a non-evidence operator handoff from the exact generated NTSC corpus.
///
/// The returned document deliberately contains no capture artifact identity,
/// hardware unit identity, timestamp, or capture-chain value. Those values do
/// not exist until the operator performs and records a physical capture.
pub fn plan_capture_campaign(
    corpus_dir: &Path,
    campaign_id: &str,
    vector_id: &str,
    signal: AnalogSignal,
    run_count: usize,
) -> Result<CaptureCampaignPlan, ValidationError> {
    text("campaign_id", campaign_id)?;
    text("vector_id", vector_id)?;
    if run_count < MIN_CLOSURE_RUNS {
        return Err(ValidationError::new(format!(
            "capture campaign requires at least {MIN_CLOSURE_RUNS} planned runs"
        )));
    }
    let run_count = u32::try_from(run_count)
        .map_err(|_| ValidationError::new("capture campaign run count exceeds u32"))?;

    let generated = generate_digital_vector_corpus(ConsoleRegion::Ntsc)?;
    let index_artifact = ArtifactRef {
        path: "corpus.json".to_owned(),
        byte_len: generated.index_bytes.len() as u64,
        sha256: digest(&generated.index_bytes),
    };
    let index_bytes = validate_artifact(
        "campaign corpus index",
        &index_artifact,
        corpus_dir,
        Some(super::MAX_MANIFEST_BYTES as u64),
    )?;
    if index_bytes != generated.index_bytes {
        return Err(ValidationError::new(
            "campaign corpus index is not the canonical generated NTSC corpus",
        ));
    }

    let selected = generated
        .vectors
        .iter()
        .find(|item| item.artifact.vector_id == vector_id)
        .ok_or_else(|| {
            ValidationError::new(format!(
                "campaign vector {vector_id:?} is absent from corpus {:?}",
                generated.index.corpus_id
            ))
        })?;
    let vector_artifact = ArtifactRef {
        path: selected.artifact.path.clone(),
        byte_len: selected.artifact.byte_len,
        sha256: selected.artifact.sha256.clone(),
    };
    let vector_bytes = validate_artifact(
        "campaign selected vector",
        &vector_artifact,
        corpus_dir,
        Some(super::MAX_INPUT_VECTOR_BYTES as u64),
    )?;
    if vector_bytes != selected.bytes {
        return Err(ValidationError::new(
            "campaign selected vector is not the canonical generated artifact",
        ));
    }

    let signal_name = match signal {
        AnalogSignal::Composite => "composite",
        AnalogSignal::SVideo => "s-video",
    };
    let suite_id = format!("{campaign_id}.{vector_id}.{signal_name}");
    let reset_sequence_id = format!("{suite_id}.power-cycle");
    let runs = (0..run_count)
        .map(|run_index| PlannedCaptureRun {
            run_index,
            repeat_index: run_index,
            run_id: format!("{suite_id}.run-{run_index:02}"),
            signal: signal.clone(),
            manifest_path: format!("runs/run-{run_index:02}/manifest.json"),
        })
        .collect::<Vec<_>>();

    let mut plan = CaptureCampaignPlan {
        schema: CAMPAIGN_PLAN_SCHEMA,
        evidence_status: CampaignEvidenceStatus::PlannedNotCaptured,
        capture_manifests_emitted: false,
        campaign_id: campaign_id.to_owned(),
        suite_id,
        content_class: "synthetic_vi_probe",
        corpus: CampaignCorpus {
            corpus_id: generated.index.corpus_id,
            region: ConsoleRegion::Ntsc,
            index_artifact,
        },
        selected_vector: CampaignVector {
            vector_id: selected.artifact.vector_id.clone(),
            vector_artifact,
            objectives: selected.artifact.objectives.clone(),
        },
        signal,
        reset_kind: "power_cycle",
        reset_sequence_id,
        run_count,
        runs,
        required_hardware_provenance: vec![
            CampaignRequirement::ConsoleClass,
            CampaignRequirement::ConsoleUnitIdSha256,
            CampaignRequirement::MotherboardRevision,
            CampaignRequirement::VideoEncoderRevision,
            CampaignRequirement::ModificationState,
            CampaignRequirement::Operator,
        ],
        required_capture_chain: vec![
            CampaignRequirement::CaptureDeviceManufacturer,
            CampaignRequirement::CaptureDeviceModel,
            CampaignRequirement::CaptureDeviceUnitIdSha256,
            CampaignRequirement::CaptureDeviceFirmware,
            CampaignRequirement::Cable,
            CampaignRequirement::TerminationOhms,
            CampaignRequirement::SampleRateHz,
            CampaignRequirement::CaptureEncoding,
            CampaignRequirement::CaptureToolName,
            CampaignRequirement::CaptureToolVersion,
            CampaignRequirement::CaptureToolBinarySha256,
            CampaignRequirement::CaptureSettingsSha256,
        ],
        required_per_run_observation: vec![
            CampaignRequirement::ResetEventIdSha256,
            CampaignRequirement::RecordedAtUtc,
            CampaignRequirement::FirstField,
            CampaignRequirement::FieldCount,
            CampaignRequirement::CaptureArtifactPath,
            CampaignRequirement::CaptureArtifactByteLen,
            CampaignRequirement::CaptureArtifactSha256,
        ],
        plan_sha256: String::new(),
    };
    let canonical = serde_json::to_vec(&plan)
        .map_err(|error| ValidationError::new(format!("encode campaign plan: {error}")))?;
    plan.plan_sha256 = digest(&canonical);
    Ok(plan)
}
