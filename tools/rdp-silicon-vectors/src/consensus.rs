use crate::{
    Blob, CoveragePlane, DepthPlane, FramebufferPlane, MemoryRegion, Producer, ProducerKind,
    RegisterValue, Setup, ValidatedBundle, ValidationError, VectorCase,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const CONSENSUS_SCHEMA: &str = "fn64.rdp-silicon-consensus.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsensusRun {
    pub bundle_sha256: String,
    pub producer: Producer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareConsensus {
    pub schema: String,
    pub suite_id: String,
    pub minimum_runs: usize,
    pub run_count: usize,
    pub runs: Vec<ConsensusRun>,
    pub consensus_sha256: String,
}

#[derive(Serialize)]
struct ConsensusDigestPayload<'a> {
    schema: &'static str,
    suite_id: &'a str,
    content_class: &'a str,
    minimum_runs: usize,
    cases: &'a [VectorCase],
    runs: &'a [ConsensusRun],
}

/// Require repeated, byte-identical captures from producers explicitly marked
/// as hardware. The minimum is caller policy; the CLI defaults it to ten.
pub fn validate_hardware_consensus(
    bundles: &[ValidatedBundle],
    minimum_runs: usize,
) -> Result<HardwareConsensus, ValidationError> {
    if minimum_runs == 0 {
        return Err(ValidationError::new(
            "hardware consensus minimum_runs must be explicit and greater than zero",
        ));
    }
    if bundles.len() < minimum_runs {
        return Err(ValidationError::new(format!(
            "hardware consensus requires at least {minimum_runs} runs; received {}",
            bundles.len()
        )));
    }
    let baseline = bundles
        .first()
        .expect("nonzero minimum and run count check guarantee a baseline");
    let mut seen = BTreeSet::new();
    let mut timestamps = BTreeSet::new();
    let mut runs = Vec::with_capacity(bundles.len());

    for (index, validated) in bundles.iter().enumerate() {
        let run = index + 1;
        let bundle = validated.bundle();
        if bundle.producer.kind != ProducerKind::Hardware {
            return Err(ValidationError::new(format!(
                "run {run} producer kind is {:?}; hardware consensus requires `hardware`",
                bundle.producer.kind
            )));
        }
        if !seen.insert(validated.canonical_sha256()) {
            return Err(ValidationError::new(format!(
                "run {run} duplicates an earlier capture bundle digest {}",
                validated.canonical_sha256()
            )));
        }
        if !timestamps.insert(&bundle.producer.recorded_at_utc) {
            return Err(ValidationError::new(format!(
                "run {run} duplicates recorded_at_utc {:?}; controlled captures require distinct run timestamps",
                bundle.producer.recorded_at_utc
            )));
        }
        if index != 0 {
            compare_producer_controls(run, &baseline.bundle().producer, &bundle.producer)?;
            compare_bundle(run, baseline, validated)?;
        }
        runs.push(ConsensusRun {
            bundle_sha256: validated.canonical_sha256().to_owned(),
            producer: bundle.producer.clone(),
        });
    }

    // Caller argument order is not evidence. Sorting also keeps every run's
    // provenance attached to its content-bound bundle identity.
    runs.sort_by(|left, right| left.bundle_sha256.cmp(&right.bundle_sha256));
    let first = baseline.bundle();
    let payload = ConsensusDigestPayload {
        schema: CONSENSUS_SCHEMA,
        suite_id: &first.suite_id,
        content_class: &first.content_class,
        minimum_runs,
        cases: &first.cases,
        runs: &runs,
    };
    let encoded = serde_json::to_vec(&payload)
        .map_err(|error| ValidationError::new(format!("encode consensus: {error}")))?;
    let consensus_sha256 = format!("{:x}", Sha256::digest(encoded));

    Ok(HardwareConsensus {
        schema: CONSENSUS_SCHEMA.to_owned(),
        suite_id: first.suite_id.clone(),
        minimum_runs,
        run_count: bundles.len(),
        runs,
        consensus_sha256,
    })
}

fn compare_producer_controls(
    run: usize,
    expected: &Producer,
    actual: &Producer,
) -> Result<(), ValidationError> {
    same(run, "producer.name", &expected.name, &actual.name)?;
    same(run, "producer.version", &expected.version, &actual.version)?;
    same(
        run,
        "producer.platform",
        &expected.platform,
        &actual.platform,
    )?;
    same(run, "producer.adapter", &expected.adapter, &actual.adapter)?;
    same(
        run,
        "producer.adapter_version",
        &expected.adapter_version,
        &actual.adapter_version,
    )?;
    same(
        run,
        "producer.producer_binary_sha256",
        &expected.producer_binary_sha256,
        &actual.producer_binary_sha256,
    )?;
    same(
        run,
        "producer.settings_sha256",
        &expected.settings_sha256,
        &actual.settings_sha256,
    )?;
    same(
        run,
        "producer.capture_method",
        &expected.capture_method,
        &actual.capture_method,
    )
}

fn compare_bundle(
    run: usize,
    baseline: &ValidatedBundle,
    candidate: &ValidatedBundle,
) -> Result<(), ValidationError> {
    let expected = baseline.bundle();
    let actual = candidate.bundle();
    same(run, "schema", &expected.schema, &actual.schema)?;
    same(run, "suite_id", &expected.suite_id, &actual.suite_id)?;
    same(
        run,
        "content_class",
        &expected.content_class,
        &actual.content_class,
    )?;
    if expected.cases.len() != actual.cases.len() {
        return mismatch(run, "case count", expected.cases.len(), actual.cases.len());
    }
    for (case_index, (left, right)) in expected.cases.iter().zip(&actual.cases).enumerate() {
        let path = format!("case[{case_index}]");
        same(
            run,
            &format!("{path}.case_id"),
            &left.case_id,
            &right.case_id,
        )?;
        same(
            run,
            &format!("{path}.description"),
            &left.description,
            &right.description,
        )?;
        same(
            run,
            &format!("{path}.capture_intent"),
            &left.capture_intent,
            &right.capture_intent,
        )?;
        same(
            run,
            &format!("{path}.rdp_completion_counters"),
            &left.rdp_completion_counters,
            &right.rdp_completion_counters,
        )?;
        blob(
            run,
            &format!("{path}.command_bytes"),
            &left.command_bytes,
            &right.command_bytes,
        )?;
        setup(run, &path, &left.setup, &right.setup)?;
        framebuffer_geometry(
            run,
            &path,
            &left.expected.framebuffer,
            &right.expected.framebuffer,
        )?;
        depth_geometry(run, &path, &left.expected.depth, &right.expected.depth)?;
        coverage_geometry(
            run,
            &path,
            &left.expected.coverage,
            &right.expected.coverage,
        )?;
        blob(
            run,
            &format!("{path}.expected.framebuffer"),
            &left.expected.framebuffer.contents,
            &right.expected.framebuffer.contents,
        )?;
        blob(
            run,
            &format!("{path}.expected.depth"),
            &left.expected.depth.contents,
            &right.expected.depth.contents,
        )?;
        blob(
            run,
            &format!("{path}.expected.coverage"),
            &left.expected.coverage.contents,
            &right.expected.coverage.contents,
        )?;
    }
    Ok(())
}

fn setup(run: usize, path: &str, left: &Setup, right: &Setup) -> Result<(), ValidationError> {
    sequence(
        run,
        &format!("{path}.setup.registers"),
        &left.registers,
        &right.registers,
        register,
    )?;
    sequence(
        run,
        &format!("{path}.setup.initial_memory"),
        &left.initial_memory,
        &right.initial_memory,
        memory,
    )
}

fn register(
    run: usize,
    path: &str,
    left: &RegisterValue,
    right: &RegisterValue,
) -> Result<(), ValidationError> {
    same(run, &format!("{path}.name"), &left.name, &right.name)?;
    same(run, &format!("{path}.value"), &left.value, &right.value)
}

fn memory(
    run: usize,
    path: &str,
    left: &MemoryRegion,
    right: &MemoryRegion,
) -> Result<(), ValidationError> {
    same(
        run,
        &format!("{path}.region_id"),
        &left.region_id,
        &right.region_id,
    )?;
    same(run, &format!("{path}.role"), &left.role, &right.role)?;
    same(
        run,
        &format!("{path}.address"),
        &left.address,
        &right.address,
    )?;
    blob(
        run,
        &format!("{path}.contents"),
        &left.contents,
        &right.contents,
    )
}

fn framebuffer_geometry(
    run: usize,
    path: &str,
    left: &FramebufferPlane,
    right: &FramebufferPlane,
) -> Result<(), ValidationError> {
    let path = format!("{path}.expected.framebuffer");
    same(
        run,
        &format!("{path}.address"),
        &left.address,
        &right.address,
    )?;
    same(run, &format!("{path}.width"), &left.width, &right.width)?;
    same(run, &format!("{path}.height"), &left.height, &right.height)?;
    same(
        run,
        &format!("{path}.row_stride_bytes"),
        &left.row_stride_bytes,
        &right.row_stride_bytes,
    )?;
    same(
        run,
        &format!("{path}.encoding"),
        &left.encoding,
        &right.encoding,
    )
}

fn depth_geometry(
    run: usize,
    path: &str,
    left: &DepthPlane,
    right: &DepthPlane,
) -> Result<(), ValidationError> {
    let path = format!("{path}.expected.depth");
    same(
        run,
        &format!("{path}.address"),
        &left.address,
        &right.address,
    )?;
    same(run, &format!("{path}.width"), &left.width, &right.width)?;
    same(run, &format!("{path}.height"), &left.height, &right.height)?;
    same(
        run,
        &format!("{path}.row_stride_bytes"),
        &left.row_stride_bytes,
        &right.row_stride_bytes,
    )
}

fn coverage_geometry(
    run: usize,
    path: &str,
    left: &CoveragePlane,
    right: &CoveragePlane,
) -> Result<(), ValidationError> {
    let path = format!("{path}.expected.coverage");
    same(
        run,
        &format!("{path}.color_image_address"),
        &left.color_image_address,
        &right.color_image_address,
    )?;
    same(run, &format!("{path}.width"), &left.width, &right.width)?;
    same(run, &format!("{path}.height"), &left.height, &right.height)?;
    same(
        run,
        &format!("{path}.encoding"),
        &left.encoding,
        &right.encoding,
    )
}

fn sequence<T, F>(
    run: usize,
    path: &str,
    left: &[T],
    right: &[T],
    compare: F,
) -> Result<(), ValidationError>
where
    F: Fn(usize, &str, &T, &T) -> Result<(), ValidationError>,
{
    if left.len() != right.len() {
        return mismatch(run, &format!("{path}.length"), left.len(), right.len());
    }
    for (index, (left, right)) in left.iter().zip(right).enumerate() {
        compare(run, &format!("{path}[{index}]"), left, right)?;
    }
    Ok(())
}

fn blob(run: usize, path: &str, left: &Blob, right: &Blob) -> Result<(), ValidationError> {
    if left.byte_len != right.byte_len {
        return mismatch(
            run,
            &format!("{path}.byte_len"),
            left.byte_len,
            right.byte_len,
        );
    }
    if left.bytes_hex != right.bytes_hex {
        let offset = left
            .bytes_hex
            .as_bytes()
            .chunks_exact(2)
            .zip(right.bytes_hex.as_bytes().chunks_exact(2))
            .position(|(left, right)| left != right)
            .expect("different equal-length validated blobs have a differing byte");
        let start = offset * 2;
        return Err(ValidationError::new(format!(
            "run {run} mismatch at {path}.byte[{offset}]: expected 0x{}, found 0x{}",
            &left.bytes_hex[start..start + 2],
            &right.bytes_hex[start..start + 2]
        )));
    }
    // A validated blob's digest is a function of its bytes, but retaining this
    // comparison makes the evidence-envelope invariant explicit.
    same(run, &format!("{path}.sha256"), &left.sha256, &right.sha256)
}

fn same<T: std::fmt::Debug + PartialEq>(
    run: usize,
    path: &str,
    left: &T,
    right: &T,
) -> Result<(), ValidationError> {
    if left == right {
        Ok(())
    } else {
        mismatch(run, path, left, right)
    }
}

fn mismatch<T: std::fmt::Debug>(
    run: usize,
    path: &str,
    expected: T,
    found: T,
) -> Result<(), ValidationError> {
    Err(ValidationError::new(format!(
        "run {run} mismatch at {path}: expected {expected:?}, found {found:?}"
    )))
}
