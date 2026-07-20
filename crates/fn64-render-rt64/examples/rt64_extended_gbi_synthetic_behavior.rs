//! Synthetic, non-ROM causal evidence for RT64's Extended-GBI cooperation.
//!
//! This deliberately substitutes only the F3DEX2 dialect identity. Production
//! `process_task` hash recognition remains unchanged and is checked by a
//! negative control below.

use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, OsTask, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderFiltering, RenderGraphicsApi, RenderRuntimeSettings, M_GFXTASK,
};
use fn64_render_rt64::{
    extended_gbi::{AspectMode, Availability, Origin, Policy, RectAlignment, Version1},
    Rt64Backend, Rt64ExtendedAspectMode, Rt64ExtendedGbiEvidence, Rt64SourceProvenance,
};
use fn64_runtime::{RdramAddr, RdramView};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const DISPLAY_LIST: usize = 0x0000_2000;
const VERSION_WORD: usize = 0x0000_1800;
const TARGET: usize = 0x0040_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const RED: u16 = 0xf801;
const ORDINARY_SHA256: &str = "914dd6b3edcee857f98061e528cfa102b69344b6104867d0f6414c7ab3f5de25";
const ASPECT_SHA256: &str = "dbb8bb25a23e67b759bcfd8276aadd036a8e5ea304736a316531645fe0df0553";

type WordPair = (u32, u32);

fn wr_u32(rdram: &mut [u8], offset: usize, value: u32) {
    rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn rd_u32(rdram: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(rdram[offset..offset + 4].try_into().expect("four bytes"))
}

fn ordinary_fill() -> [WordPair; 6] {
    let lower_right = (((WIDTH - 9) * 4) << 12) | ((HEIGHT - 9) * 4);
    [
        (0xef30_00f0, 0),
        (0xff10_0000 | (WIDTH - 1), TARGET as u32),
        (0xf700_0000, u32::from(RED) * 0x1_0001),
        (0xf600_0000 | lower_right, (8 * 4) << 12 | (8 * 4)),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ]
}

fn display_list(prefix: impl IntoIterator<Item = WordPair>) -> Vec<WordPair> {
    prefix.into_iter().chain(ordinary_fill()).collect()
}

fn install_display_list(rdram: &mut [u8], commands: &[WordPair]) {
    for (index, &(word0, word1)) in commands.iter().enumerate() {
        wr_u32(rdram, DISPLAY_LIST + index * 8, word0);
        wr_u32(rdram, DISPLAY_LIST + index * 8 + 4, word1);
    }
}

fn runtime_settings() -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(16.0 / 9.0).expect("16:9 is valid"),
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    }
}

fn backend() -> Result<Rt64Backend, Box<dyn Error>> {
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime_settings());
    backend.create(&RenderConfig::new(WIDTH, HEIGHT))?;
    Ok(backend)
}

fn require_production_rejection(backend: &mut Rt64Backend) -> Result<(), Box<dyn Error>> {
    let mut rdram = vec![0; RDRAM_LEN];
    let status = backend.process_task(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        &OsTask {
            task_type: M_GFXTASK,
            ..OsTask::default()
        },
        0,
    )?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(io::Error::other(
            "synthetic Extended fixture changed production microcode admission",
        )
        .into());
    }
    Ok(())
}

fn require_target_footprint_rejection(backend: &mut Rt64Backend) -> Result<(), Box<dyn Error>> {
    let mut rdram = vec![0; RDRAM_LEN];
    let before = rdram.clone();
    let error = backend
        .process_synthetic_extended_f3dex2(&mut rdram, DISPLAY_LIST as u32, RDRAM_LEN as u32 - 8)
        .expect_err("a target address without room for the full RGBA16 image must fail");
    if !error.to_string().contains("RGBA16 target footprint") {
        return Err(io::Error::other(format!(
            "synthetic Extended target-footprint diagnostic drifted: {error}"
        ))
        .into());
    }
    if rdram != before {
        return Err(
            io::Error::other("rejected synthetic Extended target footprint mutated RDRAM").into(),
        );
    }
    Ok(())
}

fn submit(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    commands: &[WordPair],
    capture_extended: bool,
) -> Result<Option<Rt64ExtendedGbiEvidence>, Box<dyn Error>> {
    install_display_list(rdram, commands);
    if capture_extended {
        backend.enable_extended_gbi_evidence()?;
    }
    backend.process_synthetic_extended_f3dex2(rdram, DISPLAY_LIST as u32, TARGET as u32)?;
    if capture_extended {
        Ok(Some(backend.extended_gbi_evidence()?))
    } else {
        Ok(None)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Shape {
    count: u32,
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
}

fn target_shape(rdram: &[u8]) -> Result<Shape, Box<dyn Error>> {
    let view = RdramView::from_storage(rdram);
    let mut shape = Shape {
        count: 0,
        min_x: WIDTH,
        max_x: 0,
        min_y: HEIGHT,
        max_y: 0,
    };
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let address = RdramAddr::from_offset(TARGET as u32 + (y * WIDTH + x) * 2);
            if view.read_u16(address) == RED {
                shape.count += 1;
                shape.min_x = shape.min_x.min(x);
                shape.max_x = shape.max_x.max(x);
                shape.min_y = shape.min_y.min(y);
                shape.max_y = shape.max_y.max(y);
            }
        }
    }
    if shape.count == 0 {
        return Err(io::Error::other("synthetic Extended fixture rendered no red pixels").into());
    }
    Ok(shape)
}

fn target_sha256(rdram: &[u8]) -> [u8; 32] {
    Sha256::digest(&rdram[TARGET..TARGET + (WIDTH * HEIGHT * 2) as usize]).into()
}

fn negotiated_v1(backend: &mut Rt64Backend) -> Result<(u32, Version1), Box<dyn Error>> {
    let probe = Policy::IfAvailable
        .probe(VERSION_WORD as u32)?
        .expect("optional policy emits a probe");
    let mut rdram = vec![0; RDRAM_LEN];
    wr_u32(
        &mut rdram,
        VERSION_WORD,
        fn64_render_rt64::extended_gbi::Probe::RETURN_WORD_INITIALIZER,
    );
    submit(
        backend,
        &mut rdram,
        &display_list([probe.command().words()]),
        false,
    )?;
    let response = rd_u32(&rdram, VERSION_WORD);
    let version = match probe.resolve(response)? {
        Availability::Version1(version) => version,
        Availability::Unavailable => {
            return Err(io::Error::other("synthetic RT64 probe was not recognized").into())
        }
    };
    Ok((response, version))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SuiteEvidence {
    disabled_shape: Shape,
    disabled_sha256: [u8; 32],
    probe_response: u32,
    enabled: Rt64ExtendedGbiEvidence,
    fallback_shape: Shape,
    fallback_sha256: [u8; 32],
    required_failure: String,
    aspect: Rt64ExtendedGbiEvidence,
    aspect_shape: Shape,
    aspect_sha256: [u8; 32],
}

fn run_once() -> Result<SuiteEvidence, Box<dyn Error>> {
    if Policy::Disabled.probe(VERSION_WORD as u32)?.is_some() {
        return Err(io::Error::other("disabled policy unexpectedly emitted a probe").into());
    }
    let mut backend = backend()?;
    require_production_rejection(&mut backend)?;
    require_target_footprint_rejection(&mut backend)?;
    let mut disabled_rdram = vec![0; RDRAM_LEN];
    submit(&mut backend, &mut disabled_rdram, &display_list([]), true)?;
    let disabled_shape = target_shape(&disabled_rdram)?;
    let disabled_sha256 = target_sha256(&disabled_rdram);
    if disabled_shape
        != (Shape {
            count: 48 * 32,
            min_x: 8,
            max_x: 55,
            min_y: 8,
            max_y: 39,
        })
        || hex(disabled_sha256) != ORDINARY_SHA256
    {
        return Err(io::Error::other("synthetic Extended ordinary output drifted").into());
    }

    let (probe_response, version) = negotiated_v1(&mut backend)?;

    let mut enabled_rdram = vec![0; RDRAM_LEN];
    let enabled = submit(
        &mut backend,
        &mut enabled_rdram,
        &display_list([
            version.enable_command().words(),
            version.set_rect_aspect(AspectMode::Auto).words(),
        ]),
        true,
    )?
    .expect("enabled case was armed");
    if enabled.enabled_opcode != Some(0x64)
        || enabled.hook_enable_count != 1
        || enabled.command_counts[0x33] != 1
    {
        return Err(io::Error::other("synthetic Extended enable evidence mismatch").into());
    }

    let optional = Policy::IfAvailable
        .probe(VERSION_WORD as u32)?
        .expect("optional policy emits a probe");
    if optional.resolve(0)? != Availability::Unavailable {
        return Err(
            io::Error::other("optional missing-hook fallback did not remain ordinary").into(),
        );
    }
    let mut fallback_rdram = vec![0; RDRAM_LEN];
    submit(&mut backend, &mut fallback_rdram, &display_list([]), true)?;
    let fallback_shape = target_shape(&fallback_rdram)?;
    let fallback_sha256 = target_sha256(&fallback_rdram);
    if fallback_shape != disabled_shape || fallback_sha256 != disabled_sha256 {
        return Err(io::Error::other("optional fallback changed the ordinary render path").into());
    }

    let required_failure = Policy::Required
        .probe(VERSION_WORD as u32)?
        .expect("required policy emits a probe")
        .resolve(0)
        .expect_err("missing required cooperation must fail")
        .to_string();
    if !required_failure.contains("required Extended-GBI cooperation was not recognized") {
        return Err(io::Error::other("required failure diagnostic drifted").into());
    }

    let mut aspect_rdram = vec![0; RDRAM_LEN];
    let alignment = version.set_rect_align(RectAlignment {
        left_origin: Origin::Left,
        right_origin: Origin::Right,
        left_offset: 16,
        right_offset: 16,
        ..RectAlignment::default()
    });
    let aspect = submit(
        &mut backend,
        &mut aspect_rdram,
        &display_list([
            version.enable_command().words(),
            version.set_rect_aspect(AspectMode::Adjust).words(),
            alignment[0].words(),
            alignment[1].words(),
        ]),
        true,
    )?
    .expect("aspect case was armed");
    let aspect_shape = target_shape(&aspect_rdram)?;
    let aspect_sha256 = target_sha256(&aspect_rdram);
    if aspect.enabled_opcode != Some(0x64)
        || aspect.hook_enable_count != 1
        || aspect.command_counts[0x33] != 1
        || aspect.command_counts[0x06] != 1
        || aspect.rects.len() != 1
        || aspect.rects[0].aspect_mode != Rt64ExtendedAspectMode::Adjust
        || aspect.rects[0].left_offset != 16
        || aspect.rects[0].right_offset != 16
        || aspect_shape
            != (Shape {
                count: 52 * 32,
                min_x: 12,
                max_x: 63,
                min_y: 8,
                max_y: 39,
            })
        || hex(aspect_sha256) != ASPECT_SHA256
    {
        return Err(io::Error::other("synthetic Extended aspect cooperation mismatch").into());
    }

    Ok(SuiteEvidence {
        disabled_shape,
        disabled_sha256,
        probe_response,
        enabled,
        fallback_shape,
        fallback_sha256,
        required_failure,
        aspect,
        aspect_shape,
        aspect_sha256,
    })
}

fn hex(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(
            "synthetic Extended fixture requires clean pinned Metal RT64",
        )
        .into());
    }
    let evidence = run_once()?;
    println!(
        "RT64 synthetic Extended-GBI fixture passed: ordinary={} aspect={} ordinary_shape={:?} aspect_shape={:?}",
        hex(evidence.disabled_sha256),
        hex(evidence.aspect_sha256),
        evidence.disabled_shape,
        evidence.aspect_shape,
    );
    Ok(())
}
