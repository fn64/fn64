//! Local-only F3DZEX2 native characterization transport fixture.
//!
//! A v7 admission manifest and its canonical v6 readiness report are
//! revalidated before either private raw window is opened. Diagnostics and
//! stdout intentionally contain no private path, artifact name, hash, bytes,
//! or native result identity.

use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fn64_certification::f3dzex2_point_light::{
    CharacterizationSuite, InstalledVector, RequiredCase, VectorCase, DISPLAY_LIST_ADDRESS, HEIGHT,
    OUTPUT_ADDRESS, RDRAM_BYTES, WIDTH,
};
use fn64_render::{
    ActiveRenderGraphicsApi, FrameStatus, OsTask, RenderBackend, RenderConfig, RenderFiltering,
    RenderGraphicsApi, RenderRuntimeSettings, ViFilterControl, ViPixelType, ViPresentation,
    M_GFXTASK,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64PresentedPixels, Rt64SourceProvenance,
};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut, RspMemAddr, RspMemory, RspMemoryBank};
use serde_json::Value;
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const CHARACTERIZATION_SUITE: &str = "fn64.f3dzex2-point-light.v1";
const SYSTEM_PYTHON3: &str = "/usr/bin/python3";
const PYTHON_EMBEDDED_SCRIPT_BOOTSTRAP: &str = "import sys\nscript_path = sys.argv.pop(1)\nsource = sys.stdin.buffer.read()\nexec(compile(source, script_path, 'exec'), {'__name__': '__main__', '__file__': script_path})";
const PRIVATE_INPUT_ADMISSION_SCRIPT: &[u8] =
    include_bytes!("../../../tools/private_input_admission.py");
const RAW_TEXT_BYTES: usize = 0x18d0;
const RAW_DATA_BYTES: usize = 0x0fc0;
const LOGICAL_TEXT_BYTES: usize = fn64_runtime::RSP_MEMORY_BANK_SIZE;
const TEXT_ADDRESS: usize = 0x0001_0000;
const DATA_ADDRESS: usize = 0x0002_0000;
const GUARD: u32 = 0xa31f_7c59;

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    manifest: PathBuf,
    readiness: PathBuf,
}

#[derive(Clone)]
struct PrivateRawWindows {
    text: Vec<u8>,
    data: Vec<u8>,
}

struct StagedTask {
    rdram: Vec<u8>,
    rsp_memory: RspMemory,
    task: OsTask,
    vector: InstalledVector,
}

struct PrivateStage {
    directory: PathBuf,
    manifest: PathBuf,
    report: PathBuf,
    armed: bool,
}

impl PrivateStage {
    fn create(manifest_bytes: &[u8]) -> Result<Self, Box<dyn Error>> {
        for ordinal in 0..128u32 {
            let directory = std::env::temp_dir().join(format!(
                "fn64-f3dzex2-characterization-{}-{ordinal}",
                std::process::id()
            ));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    let manifest = directory.join("manifest.json");
                    let report = directory.join("readiness.json");
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&manifest)
                        .map_err(|_| {
                            private_error("private manifest stage could not be created")
                        })?;
                    file.write_all(manifest_bytes).map_err(|_| {
                        private_error("private manifest stage could not be written")
                    })?;
                    file.sync_all()
                        .map_err(|_| private_error("private manifest stage could not be synced"))?;
                    return Ok(Self {
                        directory,
                        manifest,
                        report,
                        armed: true,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    return Err(
                        private_error("private staging directory could not be created").into(),
                    )
                }
            }
        }
        Err(private_error("private staging directory attempts were exhausted").into())
    }

    fn cleanup(mut self) -> Result<(), Box<dyn Error>> {
        fs::remove_file(&self.manifest)
            .map_err(|_| private_error("private manifest-stage cleanup failed"))?;
        if self.report.exists() {
            fs::remove_file(&self.report)
                .map_err(|_| private_error("private readiness cleanup failed"))?;
        }
        fs::remove_dir(&self.directory)
            .map_err(|_| private_error("private staging-directory cleanup failed"))?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for PrivateStage {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.manifest);
            let _ = fs::remove_file(&self.report);
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn private_error(message: &'static str) -> io::Error {
    io::Error::other(message)
}

fn parse_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<Arguments, Box<dyn Error>> {
    if arguments.next().as_deref() != Some(OsStr::new("--manifest")) {
        return Err(private_error("expected private manifest/readiness arguments").into());
    }
    let manifest = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| private_error("missing private manifest argument"))?,
    );
    if arguments.next().as_deref() != Some(OsStr::new("--readiness")) {
        return Err(private_error("expected private manifest/readiness arguments").into());
    }
    let readiness = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| private_error("missing private readiness argument"))?,
    );
    if arguments.next().is_some() || !manifest.is_absolute() || !readiness.is_absolute() {
        return Err(private_error("invalid private manifest/readiness arguments").into());
    }
    Ok(Arguments {
        manifest,
        readiness,
    })
}

struct ArtifactDescriptor {
    path: PathBuf,
    length: usize,
    sha256: [u8; 32],
}

fn decode_sha256(value: &str) -> Result<[u8; 32], Box<dyn Error>> {
    if value.len() != 64 {
        return Err(private_error("admitted artifact digest shape changed").into());
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| private_error("admitted artifact digest shape changed"))?;
    }
    Ok(digest)
}

fn artifact_descriptor(manifest: &Value, role: &str) -> Result<ArtifactDescriptor, Box<dyn Error>> {
    let descriptor = &manifest["artifacts"][role];
    let path = descriptor["path"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| private_error("admitted artifact shape changed"))?;
    let length = descriptor["length"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| private_error("admitted artifact shape changed"))?;
    let sha256_text = descriptor["sha256"]
        .as_str()
        .ok_or_else(|| private_error("admitted artifact shape changed"))?;
    let sha256 = decode_sha256(sha256_text)?;
    Ok(ArtifactDescriptor {
        path,
        length,
        sha256,
    })
}

fn require_manifest_scope(manifest: &Value) -> Result<(), Box<dyn Error>> {
    if manifest["schema"] != "fn64.private-input-admission.v7"
        || manifest["purpose"] != "f3dzex2_characterization"
        || manifest["intent"]["wire_family"] != "f3dzex2"
        || manifest["intent"]["characterization_suite"] != CHARACTERIZATION_SUITE
    {
        return Err(private_error("private admission scope differs from this runner").into());
    }
    Ok(())
}

fn load_verified_artifact(descriptor: &ArtifactDescriptor) -> Result<Vec<u8>, Box<dyn Error>> {
    let link_metadata = fs::symlink_metadata(&descriptor.path)
        .map_err(|_| private_error("admitted artifact could not be inspected"))?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
        return Err(private_error("admitted artifact is not a regular non-symlink file").into());
    }
    let mut file = File::open(&descriptor.path)
        .map_err(|_| private_error("admitted artifact could not be opened"))?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| private_error("opened artifact could not be inspected"))?;
    if !opened_metadata.is_file()
        || usize::try_from(opened_metadata.len()).ok() != Some(descriptor.length)
    {
        return Err(private_error("opened artifact geometry differs from admission").into());
    }
    let mut bytes = Vec::with_capacity(descriptor.length);
    file.read_to_end(&mut bytes)
        .map_err(|_| private_error("admitted artifact could not be read"))?;
    if bytes.len() != descriptor.length
        || <[u8; 32]>::from(Sha256::digest(&bytes)) != descriptor.sha256
    {
        return Err(private_error("consumed artifact differs from admission").into());
    }
    Ok(bytes)
}

fn load_private_windows(arguments: &Arguments) -> Result<PrivateRawWindows, Box<dyn Error>> {
    let validator =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/private_input_admission.py");
    let runtime_policy = fs::read(&validator)
        .map_err(|_| private_error("private admission policy could not be read"))?;
    if runtime_policy != PRIVATE_INPUT_ADMISSION_SCRIPT {
        return Err(
            private_error("private admission policy differs from the compiled runner").into(),
        );
    }
    let manifest_bytes = fs::read(&arguments.manifest)
        .map_err(|_| private_error("admitted manifest could not be read"))?;
    let supplied = fs::read(&arguments.readiness)
        .map_err(|_| private_error("supplied readiness could not be read"))?;
    let stage = PrivateStage::create(&manifest_bytes)?;
    let python = Path::new(SYSTEM_PYTHON3)
        .canonicalize()
        .map_err(|_| private_error("pinned system Python could not be resolved"))?;
    if !python.is_file() {
        return Err(private_error("pinned system Python is not a regular file").into());
    }
    let mut child = Command::new(&python)
        .args(["-I", "-B", "-c", PYTHON_EMBEDDED_SCRIPT_BOOTSTRAP])
        .arg(&validator)
        .arg("--manifest")
        .arg(&stage.manifest)
        .arg("--report")
        .arg(&stage.report)
        .current_dir(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .map_err(|_| private_error("repository root could not be resolved"))?,
        )
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| private_error("private admission validator could not run"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| private_error("private admission validator stdin was unavailable"))?
        .write_all(PRIVATE_INPUT_ADMISSION_SCRIPT)
        .map_err(|_| private_error("private admission policy could not be supplied"))?;
    let status = child
        .wait()
        .map_err(|_| private_error("private admission validator could not finish"))?;
    if !status.success() {
        return Err(private_error("private admission validation failed").into());
    }
    let regenerated = fs::read(&stage.report)
        .map_err(|_| private_error("regenerated readiness could not be read"))?;
    if regenerated != supplied {
        return Err(private_error("supplied readiness does not match current admission").into());
    }
    if fs::read(&arguments.manifest)
        .map_err(|_| private_error("admitted manifest could not be rechecked"))?
        != manifest_bytes
        || fs::read(&arguments.readiness)
            .map_err(|_| private_error("supplied readiness could not be rechecked"))?
            != supplied
    {
        return Err(private_error("private admission inputs changed during validation").into());
    }
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| private_error("admitted manifest could not be decoded"))?;
    require_manifest_scope(&manifest)?;
    let text = load_verified_artifact(&artifact_descriptor(
        &manifest,
        "microcode_text_raw_window",
    )?)?;
    let data = load_verified_artifact(&artifact_descriptor(
        &manifest,
        "microcode_data_raw_window",
    )?)?;
    stage.cleanup()?;
    if text.len() != RAW_TEXT_BYTES || data.len() != RAW_DATA_BYTES {
        return Err(private_error("admitted raw windows have invalid geometry").into());
    }
    Ok(PrivateRawWindows { text, data })
}

fn write_guard(rdram: &mut [u8], start: usize, len: usize) {
    let mut view = RdramViewMut::from_storage(rdram);
    view.write_u32(RdramAddr::from_offset((start - 4) as u32), GUARD);
    view.write_u32(RdramAddr::from_offset((start + len) as u32), GUARD);
}

fn private_guards_unchanged(rdram: &[u8]) -> bool {
    let view = RdramView::from_storage(rdram);
    for (start, len) in [
        (TEXT_ADDRESS, RAW_TEXT_BYTES),
        (DATA_ADDRESS, RAW_DATA_BYTES),
    ] {
        if view.read_u32(RdramAddr::from_offset((start - 4) as u32)) != GUARD
            || view.read_u32(RdramAddr::from_offset((start + len) as u32)) != GUARD
        {
            return false;
        }
    }
    true
}

fn raw_windows_unchanged(rdram: &[u8], inputs: &PrivateRawWindows) -> bool {
    rdram[TEXT_ADDRESS..TEXT_ADDRESS + RAW_TEXT_BYTES] == inputs.text
        && rdram[DATA_ADDRESS..DATA_ADDRESS + RAW_DATA_BYTES] == inputs.data
}

fn derive_logical_bytes(rdram: &[u8], address: usize, len: usize) -> Vec<u8> {
    let mut logical = vec![0; len];
    RdramView::from_storage(rdram)
        .copy_logical_bytes(RdramAddr::from_offset(address as u32), &mut logical);
    logical
}

fn stage_task(
    inputs: &PrivateRawWindows,
    vector: &VectorCase,
) -> Result<StagedTask, Box<dyn Error>> {
    if inputs.text.len() != RAW_TEXT_BYTES || inputs.data.len() != RAW_DATA_BYTES {
        return Err(private_error("raw characterization windows have invalid geometry").into());
    }
    let mut rdram = vec![0; RDRAM_BYTES];
    rdram[TEXT_ADDRESS..TEXT_ADDRESS + RAW_TEXT_BYTES].copy_from_slice(&inputs.text);
    rdram[DATA_ADDRESS..DATA_ADDRESS + RAW_DATA_BYTES].copy_from_slice(&inputs.data);
    let installed = vector
        .install(&mut rdram)
        .map_err(|_| private_error("repository characterization vector could not be installed"))?;
    for (start, len) in [
        (TEXT_ADDRESS, RAW_TEXT_BYTES),
        (DATA_ADDRESS, RAW_DATA_BYTES),
    ] {
        write_guard(&mut rdram, start, len);
    }

    let logical_text = derive_logical_bytes(&rdram, TEXT_ADDRESS, LOGICAL_TEXT_BYTES);
    let logical_data = derive_logical_bytes(&rdram, DATA_ADDRESS, RAW_DATA_BYTES);
    let mut rsp_memory = RspMemory::new();
    rsp_memory.write_bytes(
        RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
        &logical_text,
    )?;
    let task = OsTask {
        task_type: M_GFXTASK,
        ucode: TEXT_ADDRESS as u32,
        ucode_size: logical_text.len() as u32,
        ucode_data: DATA_ADDRESS as u32,
        ucode_data_size: logical_data.len() as u32,
        data_ptr: DISPLAY_LIST_ADDRESS,
        data_size: installed.display_list_bytes,
        ..OsTask::default()
    };
    Ok(StagedTask {
        rdram,
        rsp_memory,
        task,
        vector: installed,
    })
}

fn require_production_closed(
    backend: &mut Rt64Backend,
    staged: &mut StagedTask,
) -> Result<(), Box<dyn Error>> {
    if !backend.supported_ucodes().is_empty() {
        return Err(private_error("production microcode catalog unexpectedly opened").into());
    }
    let status = backend
        .process_task(
            &mut staged.rdram,
            &mut staged.rsp_memory,
            &staged.task,
            OUTPUT_ADDRESS,
        )
        .map_err(|_| private_error("production admission probe failed"))?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(private_error("production F3DZEX2 admission unexpectedly opened").into());
    }
    if !private_guards_unchanged(&staged.rdram)
        || !staged
            .vector
            .guards_unchanged(&staged.rdram)
            .map_err(|_| private_error("repository characterization guard check failed"))?
    {
        return Err(private_error("production admission probe changed a guard").into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct CaseObservation {
    denominator: RequiredCase,
    pixels: Vec<u8>,
}

fn presentation(case_ordinal: usize) -> ViPresentation {
    ViPresentation {
        noise_seed: 0x4633_5a00 | u64::try_from(case_ordinal).expect("case ordinal fits u64"),
        scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        }),
        ..ViPresentation::default()
    }
}

fn validate_present(
    evidence_workload: u64,
    pixels: &Rt64PresentedPixels,
    selection: fn64_render_rt64::Rt64PresentSelection,
) -> Result<(), Box<dyn Error>> {
    if pixels.workload_id != evidence_workload
        || pixels.present_id == 0
        || pixels.width != WIDTH
        || pixels.height != HEIGHT
        || pixels.row_bytes != WIDTH * 4
        || pixels.bytes.len() != (WIDTH * HEIGHT * 4) as usize
        || pixels.format != Rt64PresentPixelFormat::Bgra8Unorm
        || pixels.graphics_api != ActiveRenderGraphicsApi::Metal
        || selection.present_id != pixels.present_id
        || selection.source_texture_identity == 0
        || selection.target_address != OUTPUT_ADDRESS
        || selection.target_width != WIDTH
        || selection.target_height != HEIGHT
        || selection.target_size != 2
    {
        return Err(private_error("characterization task/present association changed").into());
    }
    let Some(first) = pixels.bytes.get(..4) else {
        return Err(private_error("characterization capture is empty").into());
    };
    if !pixels.bytes.chunks_exact(4).any(|pixel| pixel != first) {
        return Err(private_error("characterization capture contains no visible geometry").into());
    }
    Ok(())
}

fn run_case(
    inputs: &PrivateRawWindows,
    vector: &VectorCase,
    case_ordinal: usize,
) -> Result<CaseObservation, Box<dyn Error>> {
    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime);
    backend
        .create(&RenderConfig::ntsc(WIDTH, HEIGHT))
        .map_err(|_| private_error("native characterization backend creation failed"))?;
    backend
        .enable_present_capture()
        .map_err(|_| private_error("native characterization capture could not be enabled"))?;

    let mut staged = stage_task(inputs, vector)?;
    require_production_closed(&mut backend, &mut staged)?;
    if !raw_windows_unchanged(&staged.rdram, inputs) {
        return Err(private_error("production admission changed a raw window").into());
    }
    let evidence = backend
        .process_f3dzex2_task_for_characterization_evidence(
            &mut staged.rdram,
            &mut staged.rsp_memory,
            &staged.task,
            OUTPUT_ADDRESS,
            1,
        )
        .map_err(|_| private_error("native characterization task failed"))?;
    if evidence.planned_generation_count != 1
        || evidence.observed_generation_count != 1
        || evidence.workload_id_before.checked_add(1) != Some(evidence.workload_id_after)
        || evidence.full_sync_count != 1
        || evidence.initial_ucode.text != TEXT_ADDRESS as u32
        || evidence.initial_ucode.data != DATA_ADDRESS as u32
        || evidence.final_ucode != evidence.initial_ucode
        || evidence.variant.family() != fn64_render::UcodeId::F3dzex2
    {
        return Err(private_error("native characterization evidence shape changed").into());
    }
    if !private_guards_unchanged(&staged.rdram)
        || !staged
            .vector
            .guards_unchanged(&staged.rdram)
            .map_err(|_| private_error("repository characterization guard check failed"))?
    {
        return Err(private_error("native characterization changed a staging guard").into());
    }
    if !raw_windows_unchanged(&staged.rdram, inputs) {
        return Err(private_error("native characterization changed a raw window").into());
    }
    let output_start = OUTPUT_ADDRESS as usize;
    let output_end = output_start + (WIDTH * HEIGHT * 2) as usize;
    if staged.rdram[output_start..output_end]
        .iter()
        .all(|byte| *byte == 0)
    {
        return Err(private_error("native characterization produced no RDRAM framebuffer").into());
    }
    backend
        .present_physical_compatibility(&staged.rdram, presentation(case_ordinal))
        .map_err(|_| private_error("native characterization presentation failed"))?;
    let pixels = backend
        .presented_pixels()
        .map_err(|_| private_error("native characterization pixels were unavailable"))?;
    let selection = backend
        .present_selection()
        .map_err(|_| private_error("native characterization selection was unavailable"))?;
    validate_present(evidence.workload_id_after, &pixels, selection)?;
    require_production_closed(&mut backend, &mut staged)?;
    if !raw_windows_unchanged(&staged.rdram, inputs) {
        return Err(private_error("production admission changed a raw window").into());
    }
    Ok(CaseObservation {
        denominator: vector.id().denominator_case(),
        pixels: pixels.bytes,
    })
}

fn run(inputs: &PrivateRawWindows) -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(private_error("fixture requires clean pinned Metal RT64").into());
    }

    let cases = CharacterizationSuite.initial_cases();
    let mut observations = Vec::with_capacity(cases.len());
    for (ordinal, case) in cases.iter().enumerate() {
        observations.push(run_case(inputs, case, ordinal).map_err(|error| {
            io::Error::other(format!(
                "repository case {} failed: {error}",
                case.id().name()
            ))
        })?);
    }
    let covered: BTreeSet<_> = observations
        .iter()
        .map(|observation| observation.denominator)
        .collect();
    if covered != RequiredCase::ALL.into_iter().collect() {
        return Err(private_error("characterization denominator coverage changed").into());
    }
    let control = |case| {
        observations
            .iter()
            .find(|observation| observation.denominator == case)
            .map(|observation| observation.pixels.as_slice())
    };
    if control(RequiredCase::LightingDisabledControl)
        == control(RequiredCase::DirectionalLightControl)
    {
        return Err(private_error("public lighting controls did not separate").into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments(std::env::args_os().skip(1))?;
    let inputs = load_private_windows(&arguments)?;
    run(&inputs)?;
    println!("RT64 F3DZEX2 controlled characterization suite completed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_windows() -> PrivateRawWindows {
        PrivateRawWindows {
            text: (0..RAW_TEXT_BYTES)
                .map(|index| (index % 251) as u8)
                .collect(),
            data: (0..RAW_DATA_BYTES)
                .map(|index| (index % 239) as u8)
                .collect(),
        }
    }

    #[test]
    fn staging_preserves_raw_windows_and_derives_logical_banks() {
        let inputs = synthetic_windows();
        let case = CharacterizationSuite.initial_cases().remove(0);
        let staged = stage_task(&inputs, &case).unwrap();
        assert_eq!(
            &staged.rdram[TEXT_ADDRESS..TEXT_ADDRESS + RAW_TEXT_BYTES],
            inputs.text
        );
        assert_eq!(
            &staged.rdram[DATA_ADDRESS..DATA_ADDRESS + RAW_DATA_BYTES],
            inputs.data
        );
        let expected_text = derive_logical_bytes(&staged.rdram, TEXT_ADDRESS, LOGICAL_TEXT_BYTES);
        assert_eq!(
            staged.rsp_memory.bank(RspMemoryBank::Imem),
            expected_text.as_slice()
        );
        assert_eq!(staged.task.ucode_size as usize, LOGICAL_TEXT_BYTES);
        assert_eq!(staged.task.ucode_data_size as usize, RAW_DATA_BYTES);
        assert_eq!(staged.task.data_ptr, DISPLAY_LIST_ADDRESS);
        assert_eq!(staged.task.data_size, staged.vector.display_list_bytes);
        assert!(private_guards_unchanged(&staged.rdram));
        assert!(staged.vector.guards_unchanged(&staged.rdram).unwrap());
        assert!(raw_windows_unchanged(&staged.rdram, &inputs));
    }

    #[test]
    fn staging_rejects_short_or_long_private_windows() {
        let case = CharacterizationSuite.initial_cases().remove(0);
        let mut inputs = synthetic_windows();
        inputs.text.pop();
        assert!(stage_task(&inputs, &case).is_err());
        let mut inputs = synthetic_windows();
        inputs.data.push(0);
        assert!(stage_task(&inputs, &case).is_err());
    }

    #[test]
    fn consumed_artifact_is_bound_to_the_admitted_length_and_digest() {
        let stage = PrivateStage::create(b"{}\n").unwrap();
        let path = stage.directory.join("artifact.bin");
        let admitted = b"exact admitted private bytes";
        fs::write(&path, admitted).unwrap();
        let descriptor = ArtifactDescriptor {
            path: path.clone(),
            length: admitted.len(),
            sha256: Sha256::digest(admitted).into(),
        };
        assert_eq!(load_verified_artifact(&descriptor).unwrap(), admitted);

        let mut changed = admitted.to_vec();
        changed[0] ^= 0xff;
        fs::write(&path, changed).unwrap();
        assert!(load_verified_artifact(&descriptor).is_err());

        fs::remove_file(path).unwrap();
        stage.cleanup().unwrap();
    }

    #[test]
    fn argument_parser_requires_two_absolute_artifacts_without_echoing_them() {
        let parsed = parse_arguments(
            [
                "--manifest",
                "/private/admission.json",
                "--readiness",
                "/private/readiness.json",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(parsed.manifest, Path::new("/private/admission.json"));
        assert_eq!(parsed.readiness, Path::new("/private/readiness.json"));

        let error = parse_arguments(
            [
                "--manifest",
                "relative.json",
                "--readiness",
                "/private/readiness.json",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap_err()
        .to_string();
        assert!(!error.contains("relative.json"));
        assert!(!error.contains("/private/readiness.json"));
    }

    #[test]
    fn manifest_scope_binds_the_repository_owned_suite() {
        let mut manifest = serde_json::json!({
            "schema": "fn64.private-input-admission.v7",
            "purpose": "f3dzex2_characterization",
            "intent": {
                "wire_family": "f3dzex2",
                "characterization_suite": CHARACTERIZATION_SUITE,
            },
        });
        require_manifest_scope(&manifest).unwrap();
        manifest["intent"]["characterization_suite"] = Value::Null;
        assert!(require_manifest_scope(&manifest).is_err());
    }

    #[test]
    fn present_validation_binds_workload_present_and_target() {
        let mut bytes = vec![0; (WIDTH * HEIGHT * 4) as usize];
        bytes[4..8].copy_from_slice(&[1, 2, 3, 4]);
        let pixels = Rt64PresentedPixels {
            width: WIDTH,
            height: HEIGHT,
            row_bytes: WIDTH * 4,
            format: Rt64PresentPixelFormat::Bgra8Unorm,
            graphics_api: ActiveRenderGraphicsApi::Metal,
            present_id: 7,
            workload_id: 11,
            bytes,
        };
        let selection = fn64_render_rt64::Rt64PresentSelection {
            present_id: 7,
            source_texture_identity: 13,
            target_address: OUTPUT_ADDRESS,
            target_width: WIDTH,
            target_height: HEIGHT,
            target_size: 2,
        };
        validate_present(11, &pixels, selection).unwrap();
        assert!(validate_present(10, &pixels, selection).is_err());
        assert!(validate_present(
            11,
            &pixels,
            fn64_render_rt64::Rt64PresentSelection {
                target_address: OUTPUT_ADDRESS + 8,
                ..selection
            }
        )
        .is_err());
    }
}
