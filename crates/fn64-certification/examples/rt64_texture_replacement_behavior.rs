use std::error::Error;
use std::io;
use std::path::PathBuf;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig, RenderFiltering,
    RenderResolution, RenderRuntimeSettings, ResolutionMultiplier, ViFilterControl, ViPixelType,
    ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64ReplacementPackInput, Rt64SourceProvenance, Rt64TextureReplacementEvidence,
};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TEXTURE: u32 = 0x1000;
const TARGET: u32 = 0x2000;
const WIDTH: u32 = 16;
const HEIGHT: u32 = 16;
const RICE_HASH: &str = "c0ffee00";

#[derive(Copy, Clone)]
enum Footprint {
    Full,
    Minified,
}

struct SyntheticPack(PathBuf);

impl SyntheticPack {
    fn new() -> io::Result<Self> {
        let path =
            std::env::temp_dir().join(format!("fn64-rt64-texture-behavior-{}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn write(
        &self,
        texture_hash: u64,
        auto_path: &str,
        operation: &str,
        dds: &[u8],
    ) -> io::Result<()> {
        for entry in std::fs::read_dir(&self.0)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "dds") {
                std::fs::remove_file(path)?;
            }
        }
        let filename = match auto_path {
            "rt64" => format!("{texture_hash:016x}.dds"),
            "rice" => format!("synthetic#{RICE_HASH}_0.dds"),
            _ => unreachable!("fixture only uses pinned RT64 auto-path modes"),
        };
        std::fs::write(self.0.join(filename), dds)?;
        std::fs::write(
            self.0.join("rt64.json"),
            format!(
                "{{\"configuration\":{{\"configurationVersion\":3,\"autoPath\":\"{auto_path}\",\"defaultOperation\":\"{operation}\",\"defaultShift\":\"none\",\"hashVersion\":5}},\"textures\":[{{\"path\":\"\",\"hashes\":{{\"rt64\":\"{texture_hash:016x}\",\"rice\":\"{RICE_HASH}\"}}}}],\"operationFilters\":[],\"shiftFilters\":[],\"extraFiles\":[]}}"
            ),
        )
    }

    fn input(&self) -> Rt64ReplacementPackInput {
        Rt64ReplacementPackInput::new(&self.0)
    }
}

impl Drop for SyntheticPack {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn rgba8_dds(width: u32, height: u32, mip_colors: &[[u8; 4]]) -> Vec<u8> {
    assert!(!mip_colors.is_empty());
    let mut bytes = Vec::new();
    push_u32(&mut bytes, 0x2053_4444); // DDS magic.
    push_u32(&mut bytes, 124);
    push_u32(&mut bytes, 0x0002_100f); // Caps, size, pitch, pixel format, mips.
    push_u32(&mut bytes, height);
    push_u32(&mut bytes, width);
    push_u32(&mut bytes, width * 4);
    push_u32(&mut bytes, 1);
    push_u32(&mut bytes, mip_colors.len() as u32);
    for _ in 0..11 {
        push_u32(&mut bytes, 0);
    }
    push_u32(&mut bytes, 32);
    push_u32(&mut bytes, 4); // DDS_FOURCC.
    push_u32(&mut bytes, u32::from_le_bytes(*b"DX10"));
    for _ in 0..5 {
        push_u32(&mut bytes, 0);
    }
    push_u32(&mut bytes, 0x0040_1008); // Texture, complex, mipmap.
    for _ in 0..4 {
        push_u32(&mut bytes, 0);
    }
    push_u32(&mut bytes, 28); // DXGI_FORMAT_R8G8B8A8_UNORM.
    push_u32(&mut bytes, 3); // D3D10_RESOURCE_DIMENSION_TEXTURE2D.
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 1);
    push_u32(&mut bytes, 0);
    assert_eq!(bytes.len(), 148);

    for (mip, color) in mip_colors.iter().enumerate() {
        let mip_width = (width >> mip).max(1);
        let mip_height = (height >> mip).max(1);
        for _ in 0..mip_width * mip_height {
            bytes.extend_from_slice(color);
        }
    }
    bytes
}

fn write_command(rdram: &mut [u8], offset: usize, word0: u32, word1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn fixture(footprint: Footprint, target: u32) -> (Vec<u8>, u32) {
    let mut rdram = vec![0; RDRAM_LEN];
    let source = [
        0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001, 0xffff, 0x003f, 0x07c1,
        0xf801, 0x0001, 0xffc1, 0xf83f, 0x07ff,
    ];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in source.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                pixel,
            );
        }
    }

    // (0-0)*0+TEXEL0 in both combiner slots.
    let commands = [
        (0xfc8f_ff1f, 0x88fc_f279),
        (0xff10_000f, target),
        (0xfd10_0003, TEXTURE),
        (0xf510_0000, 7 << 24),
        (0xf300_0000, (7 << 24) | (15 << 12) | 0x800),
        (0xf510_0200, 0x0008_0200),
        (0xf200_0000, 0x0000_c00c),
        match footprint {
            Footprint::Full => (0xe400_0000 | ((WIDTH * 4) << 12) | (HEIGHT * 4), 0),
            Footprint::Minified => (0xe400_0000 | (4 << 12) | 4, 0),
        },
        match footprint {
            Footprint::Full => (0, 0x0100_0100),
            Footprint::Minified => (0, 0x1000_1000),
        },
        (0xe900_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_command(&mut rdram, COMMANDS + index * 8, word0, word1);
    }
    (rdram, (COMMANDS + commands.len() * 8) as u32)
}

fn render(
    backend: &mut Rt64Backend,
    footprint: Footprint,
    guest_cycle: u64,
) -> Result<Vec<u8>, Box<dyn Error>> {
    render_at(backend, footprint, guest_cycle, TARGET)
}

fn render_at(
    backend: &mut Rt64Backend,
    footprint: Footprint,
    guest_cycle: u64,
    target: u32,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let (mut rdram, end) = fixture(footprint, target);
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, target)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!("texture fixture returned {status:?}")).into());
    }
    backend.present_physical_compatibility(
        &rdram,
        ViPresentation {
            noise_seed: guest_cycle,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    Ok(backend.presented_pixels()?.bytes)
}

fn require_installed(
    state: &Rt64TextureReplacementEvidence,
    texture_hash: u64,
    mip_levels: u32,
) -> Result<(), Box<dyn Error>> {
    if state.texture_hash != texture_hash
        || state.texture_count != 1
        || !state.texture_known
        || !state.replacement_resolved
        || !state.replacement_installed
        || !state.replacements_enabled
        || state.replacement_mip_levels != mip_levels
    {
        return Err(io::Error::other(format!(
            "unexpected installed replacement evidence: {state:?}"
        ))
        .into());
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn load_pack(backend: &mut Rt64Backend, pack: &SyntheticPack) -> Result<(), Box<dyn Error>> {
    backend.load_replacement_packs(&[pack.input()], true)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_provenance != Rt64SourceProvenance::GitClean
        || source.source_id != "git:f0728a2520d5aa735886240de3fee75cc805f6d6"
    {
        return Err(io::Error::other(format!(
            "texture behavior evidence requires the clean pinned RT64 source: {source:?}"
        ))
        .into());
    }
    let settings = RenderRuntimeSettings {
        resolution: RenderResolution::Manual,
        resolution_multiplier: ResolutionMultiplier::new(1.0)?,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(settings);
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;

    let base = render(&mut backend, Footprint::Full, 1)?;
    let base_state = backend.wait_texture_replacement_evidence(None, false)?;
    if base_state.texture_count != 1 || !base_state.texture_known {
        return Err(
            io::Error::other(format!("unexpected base texture evidence: {base_state:?}")).into(),
        );
    }
    let texture_hash = base_state.texture_hash;
    let pack = SyntheticPack::new()?;
    let mip_colors = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
        [255, 0, 255, 255],
    ];
    let multi_mip = rgba8_dds(16, 16, &mip_colors);

    pack.write(texture_hash, "rt64", "stall", &multi_mip)?;
    load_pack(&mut backend, &pack)?;
    let rt64_state = backend.wait_texture_replacement_evidence(Some(texture_hash), true)?;
    require_installed(&rt64_state, texture_hash, mip_colors.len() as u32)?;
    let rt64_full = render(&mut backend, Footprint::Full, 2)?;
    let rt64_minified = render(&mut backend, Footprint::Minified, 3)?;
    if rt64_full == base {
        return Err(io::Error::other("RT64-named DDS did not change rendered pixels").into());
    }

    let single_mip = rgba8_dds(16, 16, &mip_colors[..1]);
    pack.write(texture_hash, "rt64", "stall", &single_mip)?;
    load_pack(&mut backend, &pack)?;
    let single_state = backend.wait_texture_replacement_evidence(Some(texture_hash), true)?;
    require_installed(&single_state, texture_hash, 1)?;
    let single_minified = render(&mut backend, Footprint::Minified, 4)?;
    if rt64_minified == single_minified {
        return Err(io::Error::other(
            "multi-mip and single-mip DDS produced identical minified output",
        )
        .into());
    }

    pack.write(texture_hash, "rice", "stall", &multi_mip)?;
    load_pack(&mut backend, &pack)?;
    let rice_state = backend.wait_texture_replacement_evidence(Some(texture_hash), true)?;
    require_installed(&rice_state, texture_hash, mip_colors.len() as u32)?;
    let rice_full = render(&mut backend, Footprint::Full, 5)?;
    if rice_full != rt64_full {
        return Err(io::Error::other(format!(
            "Rice and RT64 auto paths selected different pixels: rt64={}, rice={}",
            digest(&rt64_full),
            digest(&rice_full)
        ))
        .into());
    }

    backend.load_replacement_packs(&[], true)?;
    let stream_base_first =
        render_at(&mut backend, Footprint::Full, 6, TARGET + 0x1000).map_err(|error| {
            io::Error::other(format!(
                "first cleared no-replacement presentation failed: {error}"
            ))
        })?;
    let stream_base = render(&mut backend, Footprint::Full, 7).map_err(|error| {
        io::Error::other(format!(
            "second cleared no-replacement presentation failed: {error}"
        ))
    })?;
    if stream_base_first != stream_base {
        return Err(io::Error::other(format!(
            "consecutive cleared no-replacement presentations were unstable: first={}, second={}",
            digest(&stream_base_first),
            digest(&stream_base)
        ))
        .into());
    }

    load_pack(&mut backend, &pack)?;
    let pre_stream_state = backend.wait_texture_replacement_evidence(Some(texture_hash), true)?;
    require_installed(&pre_stream_state, texture_hash, mip_colors.len() as u32)?;
    let pre_stream_replaced = render(&mut backend, Footprint::Full, 8)?;
    if pre_stream_replaced == stream_base {
        return Err(io::Error::other(
            "pre-Stream installed replacement did not differ from the controlled base",
        )
        .into());
    }

    let stream_colors = [[0, 255, 255, 255]; 5];
    let stream_dds = rgba8_dds(16, 16, &stream_colors);
    backend.set_texture_stream_workers_paused_for_evidence(true)?;
    pack.write(texture_hash, "rt64", "stream", &stream_dds)?;
    load_pack(&mut backend, &pack)?;
    let fallback_before = backend.wait_texture_stream_fallback_evidence(texture_hash)?;
    if !fallback_before.stream_workers_paused
        || fallback_before.stream_worker_count == 0
        || fallback_before.stream_queued == 0
    {
        return Err(io::Error::other(format!(
            "Stream fallback was not held with queued RT64 work: {fallback_before:?}"
        ))
        .into());
    }
    let stream_fallback = render(&mut backend, Footprint::Full, 9).map_err(|error| {
        io::Error::other(format!(
            "paused Stream fallback presentation failed: {error}"
        ))
    })?;
    let fallback_after = backend.wait_texture_stream_fallback_evidence(texture_hash)?;
    if stream_fallback != stream_base
        || fallback_after.stream_queued != fallback_before.stream_queued
        || fallback_after.stream_load_count != 0
    {
        return Err(io::Error::other(format!(
            "fallback presentation blocked on or consumed held Stream work: before={fallback_before:?}, after={fallback_after:?}, base={}, fallback={}",
            digest(&stream_base),
            digest(&stream_fallback)
        ))
        .into());
    }
    backend.set_texture_stream_workers_paused_for_evidence(false)?;
    let stream_state = backend.wait_texture_replacement_evidence(Some(texture_hash), true)?;
    require_installed(&stream_state, texture_hash, stream_colors.len() as u32)?;
    if !stream_state.observed_resolved_not_installed || stream_state.stream_load_count == 0 {
        return Err(io::Error::other(format!(
            "Stream did not prove resolved/not-installed -> worker completion: {stream_state:?}"
        ))
        .into());
    }
    let stream_final = render(&mut backend, Footprint::Full, 10).map_err(|error| {
        io::Error::other(format!("completed Stream presentation failed: {error}"))
    })?;
    if stream_final == stream_base {
        return Err(
            io::Error::other("Stream final pixels did not replace the base texture").into(),
        );
    }

    println!(
        "texture_hash={texture_hash:016x} base={} rt64={} mip={} single={} rice={} stream_fallback={} stream={} stream_loads={} stream_transition={} mip_levels={} stream_workers={}",
        digest(&stream_base),
        digest(&rt64_full),
        digest(&rt64_minified),
        digest(&single_minified),
        digest(&rice_full),
        digest(&stream_fallback),
        digest(&stream_final),
        stream_state.stream_load_count,
        stream_state.observed_resolved_not_installed,
        rt64_state.replacement_mip_levels,
        fallback_before.stream_worker_count,
    );
    Ok(())
}
