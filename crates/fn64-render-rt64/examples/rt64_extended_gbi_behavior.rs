#![allow(clippy::identity_op)]

//! Local-only, fail-closed RT64 Extended-GBI behavior fixture.
//!
//! The private manifest is validated by `tools/private_input_admission.py`
//! before either artifact is opened. Diagnostics intentionally contain no
//! private path, name, length, hash, or bytes.

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fn64_render::{
    AspectTarget, FrameStatus, OsTask, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderFiltering, RenderGraphicsApi, RenderRuntimeSettings, ViFilterControl, ViPixelType,
    ViPresentation, M_GFXTASK,
};
use fn64_render_rt64::{
    extended_gbi::{
        AspectMode, Availability, Component, MatrixGroup, MatrixMode, Origin, Policy,
        RectAlignment, Version1,
    },
    Rt64Backend, Rt64ExtendedAspectMode, Rt64ExtendedGbiEvidence, Rt64ExtendedPresentedPixels,
    Rt64SourceProvenance, Rt64VertexZMarkerKind,
};
use fn64_runtime::{RspMemAddr, RspMemory, RspMemoryBank};
use serde_json::Value;
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const UCODE_TEXT: usize = 0x0001_0000;
const UCODE_DATA: usize = 0x0001_2000;
const UCODE_DATA_CAPACITY: usize = 0x0001_0000;
const DISPLAY_LIST: usize = 0x0004_0000;
const DISPLAY_LIST_CAPACITY: usize = 0x0000_4000;
const VERSION_WORD: usize = 0x0004_8000;
const TARGET: usize = 0x0040_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const TARGET_BYTES: usize = WIDTH as usize * HEIGHT as usize * 2;
const GUARD: u32 = 0xa5c3_7e19;
const RED: u16 = 0xf801;

type WordPair = (u32, u32);

#[derive(Clone)]
struct PrivateInputs {
    text: Vec<u8>,
    data: Vec<u8>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Case {
    HookControl,
    DisabledNegativeControl,
    Activation,
    Widescreen,
    Interpolation,
    VertexZ,
}

const CASES: [Case; 6] = [
    Case::HookControl,
    Case::DisabledNegativeControl,
    Case::Activation,
    Case::Widescreen,
    Case::Interpolation,
    Case::VertexZ,
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct CaseResult {
    case: Case,
    negotiated_version: Option<Version1>,
    evidence: Option<Rt64ExtendedGbiEvidence>,
    extended_presents: Vec<Rt64ExtendedPresentedPixels>,
    presented_sha256: [u8; 32],
    policy_sha256: [u8; 32],
}

fn private_error(message: &'static str) -> io::Error {
    io::Error::other(message)
}

fn manifest_arg() -> Result<PathBuf, Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--manifest")) {
        return Err(private_error("expected --manifest with one absolute local path").into());
    }
    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| private_error("missing private-input manifest"))?,
    );
    if args.next().is_some() || !path.is_absolute() {
        return Err(private_error("invalid private-input manifest arguments").into());
    }
    Ok(path)
}

fn artifact_path<'a>(manifest: &'a Value, role: &str) -> Result<&'a Path, Box<dyn Error>> {
    manifest["artifacts"][role]["path"]
        .as_str()
        .map(Path::new)
        .ok_or_else(|| private_error("admitted manifest artifact shape changed").into())
}

fn load_private_inputs(path: &Path) -> Result<PrivateInputs, Box<dyn Error>> {
    let validator =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/private_input_admission.py");
    let report = PathBuf::from(format!(
        "/private/tmp/fn64-extended-gbi-readiness-{}.json",
        std::process::id()
    ));
    if report.exists() {
        return Err(private_error("private-input readiness slot already exists").into());
    }
    let status = Command::new("python3")
        .arg(validator)
        .arg("--manifest")
        .arg(path)
        .arg("--report")
        .arg(&report)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| private_error("private-input validator could not run"))?;
    if !status.success() {
        return Err(private_error("private-input admission failed").into());
    }
    let manifest_bytes =
        fs::read(path).map_err(|_| private_error("admitted manifest could not be reopened"))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| private_error("admitted manifest could not be decoded"))?;
    let text = fs::read(artifact_path(&manifest, "microcode_text")?)
        .map_err(|_| private_error("admitted microcode text could not be read"))?;
    let data = fs::read(artifact_path(&manifest, "microcode_data")?)
        .map_err(|_| private_error("admitted microcode data could not be read"))?;
    fs::remove_file(&report)
        .map_err(|_| private_error("private-input readiness cleanup failed"))?;
    if text.len() != fn64_runtime::RSP_MEMORY_BANK_SIZE || data.len() > UCODE_DATA_CAPACITY {
        return Err(private_error("admitted microcode does not fit the fixture layout").into());
    }
    Ok(PrivateInputs { text, data })
}

fn wr_u32(rdram: &mut [u8], offset: usize, value: u32) {
    rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn rd_u32(rdram: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(rdram[offset..offset + 4].try_into().expect("four bytes"))
}

fn ordinary_fill() -> [WordPair; 6] {
    let lr = (((WIDTH - 9) * 4) << 12) | ((HEIGHT - 9) * 4);
    [
        (0xef30_00f0, 0),
        (0xff10_0000 | (WIDTH - 1), TARGET as u32),
        (0xf700_0000, u32::from(RED) * 0x1_0001),
        (0xf600_0000 | lr, (8 * 4) << 12 | (8 * 4)),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ]
}

fn commands(case: Case, version: Option<Version1>) -> Result<Vec<WordPair>, Box<dyn Error>> {
    if case == Case::HookControl {
        let probe = Policy::Required
            .probe(VERSION_WORD as u32)?
            .expect("required policy always creates a probe");
        return Ok(vec![probe.command().words(), (0xdf00_0000, 0)]);
    }
    let version =
        version.ok_or_else(|| private_error("Extended-GBI case lacks a negotiated v1 session"))?;
    let mut words = Vec::new();
    match case {
        Case::HookControl => unreachable!(),
        Case::DisabledNegativeControl => {
            words.push(version.set_rect_aspect(AspectMode::Adjust).words())
        }
        Case::Activation | Case::Widescreen => {
            words.push(version.enable_command().words());
            words.push(version.set_rect_aspect(AspectMode::Adjust).words());
            words.extend(
                version
                    .set_rect_align(RectAlignment {
                        left_origin: Origin::Left,
                        right_origin: Origin::Right,
                        left_offset: 16,
                        right_offset: 16,
                        ..RectAlignment::default()
                    })
                    .map(|command| command.words()),
            );
        }
        Case::Interpolation => {
            words.push(version.enable_command().words());
            words.push(version.set_refresh_rate(120)?.words());
            words.extend(
                version
                    .matrix_group(MatrixGroup {
                        id: 7,
                        mode: MatrixMode::Decompose,
                        position: Component::Interpolate,
                        rotation: Component::Interpolate,
                        editable: true,
                        aspect: AspectMode::Adjust,
                        ..MatrixGroup::default()
                    })
                    .map(|command| command.words()),
            );
        }
        Case::VertexZ => {
            words.push(version.enable_command().words());
            words.push(version.begin_vertex_z_test(3).words());
            words.push(version.end_vertex_z_test().words());
        }
    }
    words.extend(ordinary_fill());
    Ok(words)
}

fn guard(rdram: &mut [u8], start: usize, len: usize) {
    wr_u32(rdram, start - 4, GUARD);
    wr_u32(rdram, start + len, GUARD);
}

fn require_guards(rdram: &[u8], regions: &[(usize, usize)]) -> Result<(), Box<dyn Error>> {
    if regions.iter().any(|&(start, len)| {
        rd_u32(rdram, start - 4) != GUARD || rd_u32(rdram, start + len) != GUARD
    }) {
        return Err(io::Error::other("Extended-GBI fixture RDRAM guard changed").into());
    }
    Ok(())
}

fn validate_semantics(
    case: Case,
    evidence: &Rt64ExtendedGbiEvidence,
) -> Result<(), Box<dyn Error>> {
    let no_commands = evidence.command_counts.iter().all(|count| *count == 0);
    match case {
        Case::HookControl => unreachable!("hook control is not armed"),
        Case::DisabledNegativeControl
            if evidence.enabled_opcode.is_none()
                && evidence.hook_enable_count == 0
                && no_commands
                && evidence.rects.is_empty()
                && evidence.groups.is_empty()
                && evidence.vertex_z.is_empty() => {}
        Case::Activation | Case::Widescreen
            if evidence.enabled_opcode == Some(0x64)
                && evidence.hook_enable_count == 1
                && evidence.command_counts[0x33] == 1
                && evidence.command_counts[0x06] == 1
                && evidence.rects.len() == 1
                && evidence.rects[0].aspect_mode == Rt64ExtendedAspectMode::Adjust
                && evidence.rects[0].left_offset == 16
                && evidence.rects[0].right_offset == 16 => {}
        Case::Interpolation
            if evidence.enabled_opcode == Some(0x64)
                && evidence.command_counts[0x09] == 1
                && evidence.command_counts[0x0c] == 1
                && evidence.refresh_rate == Some(120)
                && evidence.groups.len() == 1
                && evidence.groups[0].group_id == 7
                && evidence.generated_presents.len() >= 2 => {}
        Case::VertexZ
            if evidence.enabled_opcode == Some(0x64)
                && evidence.command_counts[0x0a] == 1
                && evidence.command_counts[0x0b] == 1
                && evidence.vertex_z.len() == 2
                && evidence.vertex_z[0].marker_kind == Rt64VertexZMarkerKind::Begin
                && evidence.vertex_z[1].marker_kind == Rt64VertexZMarkerKind::End => {}
        _ => return Err(io::Error::other("Extended-GBI semantic evidence mismatch").into()),
    }
    Ok(())
}

fn validate_extended_presents(
    evidence: &Rt64ExtendedGbiEvidence,
    captures: &[Rt64ExtendedPresentedPixels],
) -> Result<(), Box<dyn Error>> {
    let expected_count = evidence.generated_presents.len().max(1);
    if captures.len() != expected_count {
        return Err(io::Error::other("Extended present history count mismatch").into());
    }
    for (index, capture) in captures.iter().enumerate() {
        if capture.capture_ordinal != index as u32
            || capture.workload_id != evidence.workload_id
            || capture.present_id != evidence.present_id
        {
            return Err(io::Error::other("Extended present history association mismatch").into());
        }
        if let Some(generated) = evidence.generated_presents.get(index) {
            if capture.generated_ordinal != Some(generated.presentation_ordinal)
                || capture.interpolation_numerator != generated.interpolation_numerator
                || capture.interpolation_denominator != generated.interpolation_denominator
            {
                return Err(io::Error::other("Extended generated-frame fraction mismatch").into());
            }
        } else if capture.generated_ordinal.is_some() {
            return Err(io::Error::other("ordinary endpoint was labeled generated").into());
        }
    }
    Ok(())
}

fn run_case(
    inputs: &PrivateInputs,
    case: Case,
    version: Option<Version1>,
) -> Result<CaseResult, Box<dyn Error>> {
    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new()
        .with_f3dex2_ucode_text(&inputs.text)
        .with_runtime_settings(runtime.clone());
    backend.create(&RenderConfig::new(WIDTH, HEIGHT))?;
    if case != Case::HookControl {
        backend.enable_present_capture()?;
        backend.enable_extended_gbi_evidence()?;
    }

    let words = commands(case, version)?;
    let dl_len = words.len() * 8;
    if dl_len > DISPLAY_LIST_CAPACITY {
        return Err(io::Error::other("Extended-GBI command fixture overflow").into());
    }
    let mut rdram = vec![0; RDRAM_LEN];
    rdram[UCODE_TEXT..UCODE_TEXT + inputs.text.len()].copy_from_slice(&inputs.text);
    rdram[UCODE_DATA..UCODE_DATA + inputs.data.len()].copy_from_slice(&inputs.data);
    for (index, (word0, word1)) in words.into_iter().enumerate() {
        wr_u32(&mut rdram, DISPLAY_LIST + index * 8, word0);
        wr_u32(&mut rdram, DISPLAY_LIST + index * 8 + 4, word1);
    }
    wr_u32(
        &mut rdram,
        VERSION_WORD,
        if case == Case::HookControl {
            fn64_render_rt64::extended_gbi::Probe::RETURN_WORD_INITIALIZER
        } else {
            0xfeed_face
        },
    );
    let regions = [
        (UCODE_TEXT, inputs.text.len()),
        (UCODE_DATA, inputs.data.len()),
        (DISPLAY_LIST, dl_len),
        (VERSION_WORD, 4),
        (TARGET, TARGET_BYTES),
    ];
    for &(start, len) in &regions {
        guard(&mut rdram, start, len);
    }

    let mut rsp = RspMemory::new();
    rsp.write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &inputs.text)?;
    let task = OsTask {
        task_type: M_GFXTASK,
        ucode: UCODE_TEXT as u32,
        ucode_size: inputs.text.len() as u32,
        ucode_data: UCODE_DATA as u32,
        ucode_data_size: inputs.data.len() as u32,
        data_ptr: DISPLAY_LIST as u32,
        data_size: dl_len as u32,
        ..OsTask::default()
    };
    if backend.process_task(&mut rdram, &mut rsp, &task, TARGET as u32)? != FrameStatus::Complete {
        return Err(io::Error::other("Extended-GBI task did not complete").into());
    }
    if case == Case::HookControl {
        let probe = Policy::Required
            .probe(VERSION_WORD as u32)?
            .expect("required policy always creates a probe");
        let negotiated_version = match probe.resolve(rd_u32(&rdram, VERSION_WORD))? {
            Availability::Version1(version) => version,
            Availability::Unavailable => {
                return Err(
                    io::Error::other("required Extended-GBI probe resolved as unavailable").into(),
                )
            }
        };
        require_guards(&rdram, &regions)?;
        return Ok(CaseResult {
            case,
            negotiated_version: Some(negotiated_version),
            evidence: None,
            extended_presents: Vec::new(),
            presented_sha256: [0; 32],
            policy_sha256: backend
                .active_runtime_policy()
                .ok_or_else(|| io::Error::other("RT64 active policy missing"))?
                .sha256(),
        });
    }
    backend.present(ViPresentation {
        noise_seed: 0x6412_0000 | case as u64,
        filters: ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        },
        ..ViPresentation::default()
    })?;
    require_guards(&rdram, &regions)?;
    let capture = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    if capture.present_id == 0
        || selection.present_id != capture.present_id
        || selection.target_address != TARGET as u32
        || capture.bytes.len() != (capture.row_bytes * capture.height) as usize
    {
        return Err(io::Error::other("Extended-GBI present association mismatch").into());
    }
    let evidence = backend.extended_gbi_evidence()?;
    validate_semantics(case, &evidence)?;
    let extended_presents = backend.extended_presented_pixels()?;
    validate_extended_presents(&evidence, &extended_presents)?;
    Ok(CaseResult {
        case,
        negotiated_version: None,
        evidence: Some(evidence),
        extended_presents,
        presented_sha256: Sha256::digest(&capture.bytes).into(),
        policy_sha256: backend
            .active_runtime_policy()
            .ok_or_else(|| io::Error::other("RT64 active policy missing"))?
            .sha256(),
    })
}

fn run_suite(inputs: &PrivateInputs) -> Result<Vec<CaseResult>, Box<dyn Error>> {
    let hook = run_case(inputs, Case::HookControl, None)?;
    let version = hook
        .negotiated_version
        .ok_or_else(|| private_error("hook control did not retain the negotiated version"))?;
    let mut results = vec![hook];
    for &case in &CASES[1..] {
        results.push(run_case(inputs, case, Some(version))?);
    }
    Ok(results)
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other("fixture requires clean pinned Metal RT64").into());
    }
    let inputs = load_private_inputs(&manifest_arg()?)?;
    let expected = run_suite(&inputs)?;
    for _ in 1..10 {
        if run_suite(&inputs)? != expected {
            return Err(io::Error::other("Extended-GBI ten-run evidence drifted").into());
        }
    }
    println!("RT64 Extended-GBI six-case fixture passed 10 consecutive runs");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_cases_have_isolated_display_lists() {
        let version = Policy::Required
            .probe(VERSION_WORD as u32)
            .unwrap()
            .unwrap()
            .resolve(1)
            .unwrap()
            .require_v1()
            .unwrap();
        assert_eq!(CASES.len(), 6);
        for case in CASES {
            let words = commands(case, (case != Case::HookControl).then_some(version)).unwrap();
            assert_eq!(words.last(), Some(&(0xdf00_0000, 0)));
            assert!(words.len() * 8 <= DISPLAY_LIST_CAPACITY);
        }
    }
}
