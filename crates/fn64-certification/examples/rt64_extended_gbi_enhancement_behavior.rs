// Public, non-ROM behavioral closure for RT64 Extended-GBI cooperation.
//
// The opt-in admission substitutes only RT64's built-in F3DEX2 dialect.
// Production `process_task` recognition remains strict and is checked before
// and after the live command/pixel cases.

use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, OsTask, RefreshRateTarget, RenderBackend, RenderConfig, RenderFiltering,
    RenderGraphicsApi, RenderRefreshRate, RenderRuntimeSettings, ViFilterControl, ViPixelType,
    ViPresentation, M_GFXTASK,
};
use fn64_render_reference::gbi;
use fn64_render_rt64::{
    extended_gbi::{
        AspectMode, Availability, Component, MatrixGroup, MatrixMode, MatrixOrder, Policy, Version1,
    },
    Rt64Backend, Rt64ExtendedGbiEvidence, Rt64ExtendedPresentedPixels, Rt64PresentPixelFormat,
    Rt64SourceProvenance, Rt64TransformClass, Rt64TransformComponentSelector,
    Rt64TransformOrdering, Rt64VertexZMarkerKind,
};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const PINNED_OVERLAY: &str =
    "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const SEGMENT: u8 = 6;
const SEGMENT_BASE: usize = 0x0000_1000;
const VERTICES: usize = SEGMENT_BASE;
const PROJECTION: usize = SEGMENT_BASE + 0x0200;
const MODEL: usize = SEGMENT_BASE + 0x0240;
const VIEWPORT: usize = SEGMENT_BASE + 0x0280;
const VERSION_WORD: usize = 0x0000_1800;
const DISPLAY_LIST: usize = 0x0000_3000;
const DEPTH: usize = 0x0030_0000;
const TARGET: usize = 0x0040_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const ORIGINAL_RATE: u16 = 60;
const TARGET_RATE: u32 = 120;
const GUARD: u32 = 0x7ad1_5ea1;
const BUFFER_BYTES: usize = (WIDTH * HEIGHT * 2) as usize;

type WordPair = (u32, u32);

fn wr_u32(rdram: &mut [u8], offset: usize, value: u32) {
    rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn rd_u32(rdram: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(rdram[offset..offset + 4].try_into().expect("four bytes"))
}

fn wr_u8(rdram: &mut [u8], offset: usize, value: u8) {
    rdram[offset ^ 3] = value;
}

fn wr_i16(rdram: &mut [u8], offset: usize, value: i16) {
    for (index, byte) in (value as u16).to_be_bytes().into_iter().enumerate() {
        wr_u8(rdram, offset + index, byte);
    }
}

fn write_matrix(rdram: &mut [u8], offset: usize, elements: [f32; 16]) {
    for (index, value) in elements.into_iter().enumerate() {
        let fixed = (value * 65536.0) as i32;
        wr_i16(rdram, offset + index * 2, (fixed >> 16) as i16);
        wr_i16(rdram, offset + 32 + index * 2, fixed as u16 as i16);
    }
}

fn push(commands: &mut Vec<WordPair>, command: WordPair) {
    commands.push(command);
}

fn install_display_list(rdram: &mut [u8], commands: &[WordPair]) {
    if DISPLAY_LIST + commands.len() * 8 > DISPLAY_LIST + 0x200 {
        panic!("Extended-GBI closure fixture overflowed its display-list region");
    }
    for (index, &(word0, word1)) in commands.iter().enumerate() {
        wr_u32(rdram, DISPLAY_LIST + index * 8, word0);
        wr_u32(rdram, DISPLAY_LIST + index * 8 + 4, word1);
    }
}

fn arm_guards(rdram: &mut [u8]) {
    for offset in [
        SEGMENT_BASE - 4,
        SEGMENT_BASE + 0x300,
        DISPLAY_LIST - 4,
        DISPLAY_LIST + 0x200,
        DEPTH - 4,
        DEPTH + BUFFER_BYTES,
        TARGET - 4,
        TARGET + BUFFER_BYTES,
    ] {
        wr_u32(rdram, offset, GUARD);
    }
}

fn require_guards(rdram: &[u8], case: &str) -> Result<(), Box<dyn Error>> {
    for offset in [
        SEGMENT_BASE - 4,
        SEGMENT_BASE + 0x300,
        DISPLAY_LIST - 4,
        DISPLAY_LIST + 0x200,
        DEPTH - 4,
        DEPTH + BUFFER_BYTES,
        TARGET - 4,
        TARGET + BUFFER_BYTES,
    ] {
        if rd_u32(rdram, offset) != GUARD {
            return Err(
                io::Error::other(format!("{case} RDRAM guard changed at {offset:#010x}")).into(),
            );
        }
    }
    Ok(())
}

fn write_geometry(rdram: &mut [u8], translation_x: f32, test_z: i16) {
    let vertices = [
        ([-5_i16, -5_i16, test_z], [255_u8, 0_u8, 0_u8, 255_u8]),
        ([5, -5, test_z], [255, 0, 0, 255]),
        ([0, 6, test_z], [255, 0, 0, 255]),
    ];
    for (index, (position, color)) in vertices.into_iter().enumerate() {
        let offset = VERTICES + index * 16;
        wr_i16(rdram, offset, position[0]);
        wr_i16(rdram, offset + 2, position[1]);
        wr_i16(rdram, offset + 4, position[2]);
        for (channel, value) in color.into_iter().enumerate() {
            wr_u8(rdram, offset + 12 + channel, value);
        }
    }

    let mut projection = [0.0_f32; 16];
    projection[0] = 1.0 / 16.0;
    projection[5] = 1.0 / 16.0;
    projection[10] = 1.0;
    projection[15] = 1.0;
    write_matrix(rdram, PROJECTION, projection);

    let mut model = [0.0_f32; 16];
    model[0] = 1.0;
    model[5] = 1.0;
    model[10] = 1.0;
    model[12] = translation_x;
    model[15] = 1.0;
    write_matrix(rdram, MODEL, model);

    for (index, value) in [
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        wr_i16(rdram, VIEWPORT + index * 2, value);
    }
}

fn transform_group(id: u32, projection: bool) -> MatrixGroup {
    MatrixGroup {
        id,
        projection,
        mode: MatrixMode::Decompose,
        position: Component::Interpolate,
        rotation: Component::Interpolate,
        scale: Component::Interpolate,
        skew: Component::Skip,
        perspective: Component::Interpolate,
        vertex: Component::Skip,
        tile: Component::Skip,
        order: MatrixOrder::Auto,
        editable: true,
        aspect: AspectMode::Auto,
        texcoord: Component::Skip,
        look_at: Component::Skip,
        ..MatrixGroup::default()
    }
}

fn scene_commands(
    version: Version1,
    vertex_z: bool,
    depth_value: u16,
) -> Result<Vec<WordPair>, Box<dyn Error>> {
    let mut commands = Vec::new();
    push(&mut commands, version.enable_command().words());
    push(
        &mut commands,
        version.set_refresh_rate(ORIGINAL_RATE)?.words(),
    );
    push(
        &mut commands,
        (
            ((gbi::G_MOVEWORD as u32) << 24) | (0x06 << 16) | (u32::from(SEGMENT) * 4),
            SEGMENT_BASE as u32,
        ),
    );
    push(
        &mut commands,
        (
            ((gbi::G_MOVEMEM as u32) << 24) | (1 << 19) | 8,
            (u32::from(SEGMENT) << 24) | 0x0280,
        ),
    );
    push(&mut commands, (0xfe00_0000, DEPTH as u32));
    push(&mut commands, (0xff10_0000 | (WIDTH - 1), TARGET as u32));
    push(
        &mut commands,
        (0xed00_0000, (WIDTH * 4) << 12 | (HEIGHT * 4)),
    );
    push(&mut commands, (0xef30_00f0, 0));
    push(&mut commands, (0xf700_0000, 0x0001_0001));
    push(
        &mut commands,
        (
            0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
            0,
        ),
    );
    push(&mut commands, (0xe700_0000, 0));
    // Write the declared background plane through the ordinary primitive-Z
    // update path, then erase only its color contribution. Vertex-Z samples
    // the retained managed depth target below.
    push(&mut commands, (0xfcff_ffff, 0xfffd_f6fb));
    push(&mut commands, (0xfa00_0000, 0x0000_00ff));
    push(
        &mut commands,
        (0xee00_0000, (u32::from(depth_value) << 16) | 1),
    );
    push(&mut commands, (0xef00_00f0, 0x24));
    push(
        &mut commands,
        (
            0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
            0,
        ),
    );
    push(&mut commands, (0xe700_0000, 0));
    push(&mut commands, (0xef30_00f0, 0));
    push(&mut commands, (0xf700_0000, 0x0001_0001));
    push(
        &mut commands,
        (
            0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
            0,
        ),
    );
    push(&mut commands, (0xe700_0000, 0));
    let matrix_length = (((64_u32 - 1) / 8) & 0x1f) << 19;
    for command in version.matrix_group(transform_group(0x5052_4f4a, true)) {
        push(&mut commands, command.words());
    }
    push(
        &mut commands,
        (
            ((gbi::G_MTX as u32) << 24) | matrix_length | 0x07,
            (u32::from(SEGMENT) << 24) | 0x0200,
        ),
    );
    for command in version.matrix_group(transform_group(0x4d4f_444c, false)) {
        push(&mut commands, command.words());
    }
    push(
        &mut commands,
        (
            ((gbi::G_MTX as u32) << 24) | matrix_length | 0x03,
            (u32::from(SEGMENT) << 24) | 0x0240,
        ),
    );
    push(&mut commands, (0xfcff_ffff, 0xfffd_f6fb));
    push(&mut commands, (0xfa00_0000, 0xf800_00ff));
    push(&mut commands, (0xef00_00f0, 0));
    push(
        &mut commands,
        (
            ((gbi::G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            u32::from(SEGMENT) << 24,
        ),
    );
    if vertex_z {
        push(&mut commands, version.begin_vertex_z_test(0).words());
    }
    push(
        &mut commands,
        (((gbi::G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1), 0),
    );
    if vertex_z {
        push(&mut commands, version.end_vertex_z_test().words());
    }
    push(&mut commands, (0xe900_0000, 0));
    push(&mut commands, ((gbi::G_ENDDL as u32) << 24, 0));
    Ok(commands)
}

fn settings(refresh_rate: RenderRefreshRate) -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        refresh_rate,
        refresh_rate_target: RefreshRateTarget::new(TARGET_RATE)
            .expect("120 Hz is in the typed range"),
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    }
}

fn presentation(seed: u64) -> ViPresentation {
    ViPresentation {
        noise_seed: seed,
        scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        }),
        ..ViPresentation::default()
    }
}

fn backend(refresh_rate: RenderRefreshRate) -> Result<Rt64Backend, Box<dyn Error>> {
    let mut backend = Rt64Backend::new().with_runtime_settings(settings(refresh_rate));
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
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
            "Extended closure fixture changed production microcode admission",
        )
        .into());
    }
    Ok(())
}

fn negotiate_v1(backend: &mut Rt64Backend) -> Result<Version1, Box<dyn Error>> {
    let probe = Policy::Required
        .probe(VERSION_WORD as u32)?
        .expect("required policy emits a probe");
    let mut rdram = vec![0; RDRAM_LEN];
    wr_u32(
        &mut rdram,
        VERSION_WORD,
        fn64_render_rt64::extended_gbi::Probe::RETURN_WORD_INITIALIZER,
    );
    let commands = [
        probe.command().words(),
        (0xef30_00f0, 0),
        (0xff10_0000 | (WIDTH - 1), TARGET as u32),
        (0xf700_0000, 0x0001_0001),
        (
            0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
            0,
        ),
        (0xe900_0000, 0),
        ((gbi::G_ENDDL as u32) << 24, 0),
    ];
    install_display_list(&mut rdram, &commands);
    backend.process_synthetic_extended_f3dex2(&mut rdram, DISPLAY_LIST as u32, TARGET as u32)?;
    match probe.resolve(rd_u32(&rdram, VERSION_WORD))? {
        Availability::Version1(version) => Ok(version),
        Availability::Unavailable => {
            Err(io::Error::other("required live Extended probe was unavailable").into())
        }
    }
}

fn negotiated_version() -> Result<Version1, Box<dyn Error>> {
    let mut probe_backend = backend(RenderRefreshRate::Original)?;
    require_production_rejection(&mut probe_backend)?;
    let version = negotiate_v1(&mut probe_backend)?;
    require_production_rejection(&mut probe_backend)?;
    Ok(version)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Shape {
    count: u32,
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
    sum_x: u64,
}

fn red_shape(capture: &Rt64ExtendedPresentedPixels) -> Result<Shape, Box<dyn Error>> {
    if capture.format != Rt64PresentPixelFormat::Bgra8Unorm
        || capture.row_bytes != capture.width * 4
    {
        return Err(io::Error::other("Extended closure capture format drifted").into());
    }
    let mut shape = Shape {
        count: 0,
        min_x: capture.width,
        max_x: 0,
        min_y: capture.height,
        max_y: 0,
        sum_x: 0,
    };
    for y in 0..capture.height {
        for x in 0..capture.width {
            let offset = (y * capture.row_bytes + x * 4) as usize;
            let blue = capture.bytes[offset];
            let green = capture.bytes[offset + 1];
            let red = capture.bytes[offset + 2];
            if red > 128 && green < 64 && blue < 64 {
                shape.count += 1;
                shape.min_x = shape.min_x.min(x);
                shape.max_x = shape.max_x.max(x);
                shape.min_y = shape.min_y.min(y);
                shape.max_y = shape.max_y.max(y);
                shape.sum_x += u64::from(x);
            }
        }
    }
    Ok(shape)
}

fn digest(capture: &Rt64ExtendedPresentedPixels) -> [u8; 32] {
    Sha256::digest(&capture.bytes).into()
}

fn validate_groups(evidence: &Rt64ExtendedGbiEvidence) -> Result<(), Box<dyn Error>> {
    if evidence.command_counts[0x0c] != 2 || evidence.groups.len() != 2 {
        return Err(io::Error::other("Extended matrix-group dispatch count drifted").into());
    }
    for (index, group) in evidence.groups.iter().enumerate() {
        let expected_id = [0x5052_4f4a, 0x4d4f_444c][index];
        let expected_class = [Rt64TransformClass::Projection, Rt64TransformClass::Model][index];
        if group.group_id != expected_id
            || group.class != expected_class
            || group.push
            || !group.decompose
            || !group.editable
            || group.position != Rt64TransformComponentSelector::Interpolate
            || group.rotation != Rt64TransformComponentSelector::Interpolate
            || group.scale != Rt64TransformComponentSelector::Interpolate
            || group.skew != Rt64TransformComponentSelector::Skip
            || group.perspective != Rt64TransformComponentSelector::Interpolate
            || group.vertex != Rt64TransformComponentSelector::Skip
            || group.texcoord != Rt64TransformComponentSelector::Skip
            || group.tile != Rt64TransformComponentSelector::Skip
            || group.look_at != Rt64TransformComponentSelector::Skip
            || group.ordering != Rt64TransformOrdering::Auto
            || group.aspect_mode != fn64_render_rt64::Rt64ExtendedAspectMode::Auto
        {
            return Err(io::Error::other(format!(
                "Extended matrix-group evidence drifted at {index}: {group:?}"
            ))
            .into());
        }
    }
    Ok(())
}

#[derive(Copy, Clone)]
struct SceneSubmission {
    translation_x: f32,
    test_z: i16,
    vertex_z: bool,
    depth_value: u16,
    seed: u64,
}

fn submit_scene(
    backend: &mut Rt64Backend,
    version: Version1,
    rdram: &mut [u8],
    scene: SceneSubmission,
) -> Result<(), Box<dyn Error>> {
    write_geometry(rdram, scene.translation_x, scene.test_z);
    install_display_list(
        rdram,
        &scene_commands(version, scene.vertex_z, scene.depth_value)?,
    );
    backend.process_synthetic_hfr_f3dex2(
        rdram,
        DISPLAY_LIST as u32,
        TARGET as u32,
        ORIGINAL_RATE,
    )?;
    backend.present_physical_compatibility(&*rdram, presentation(scene.seed))?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InterpolationEvidence {
    semantic: Rt64ExtendedGbiEvidence,
    previous: Shape,
    midpoint: Shape,
    current: Shape,
    midpoint_digest: [u8; 32],
    current_digest: [u8; 32],
}

fn interpolation_case() -> Result<InterpolationEvidence, Box<dyn Error>> {
    let version = negotiated_version()?;
    let mut backend = backend(RenderRefreshRate::Manual)?;
    require_production_rejection(&mut backend)?;
    let mut rdram = vec![0; RDRAM_LEN];
    arm_guards(&mut rdram);
    submit_scene(
        &mut backend,
        version,
        &mut rdram,
        SceneSubmission {
            translation_x: -4.0,
            test_z: 0,
            vertex_z: false,
            depth_value: 0xfffc,
            seed: 1,
        },
    )?;
    submit_scene(
        &mut backend,
        version,
        &mut rdram,
        SceneSubmission {
            translation_x: -2.0,
            test_z: 0,
            vertex_z: false,
            depth_value: 0xfffc,
            seed: 2,
        },
    )?;
    let previous_pixels = backend.presented_pixels()?;
    let previous_capture = Rt64ExtendedPresentedPixels {
        capture_generation: 0,
        workload_id: 0,
        present_id: previous_pixels.present_id,
        capture_ordinal: 0,
        generated_ordinal: None,
        interpolation_numerator: 1,
        interpolation_denominator: 1,
        width: previous_pixels.width,
        height: previous_pixels.height,
        row_bytes: previous_pixels.row_bytes,
        format: previous_pixels.format,
        bytes: previous_pixels.bytes,
    };
    let previous = red_shape(&previous_capture)?;

    backend.enable_extended_gbi_evidence()?;
    submit_scene(
        &mut backend,
        version,
        &mut rdram,
        SceneSubmission {
            translation_x: 0.0,
            test_z: 0,
            vertex_z: false,
            depth_value: 0xfffc,
            seed: 3,
        },
    )?;
    let semantic = backend.extended_gbi_evidence()?;
    let frames = backend.extended_presented_pixels()?;
    validate_groups(&semantic)?;
    if semantic.enabled_opcode != Some(0x64)
        || semantic.hook_enable_count != 1
        || semantic.command_counts[0x09] != 1
        || semantic.refresh_rate != Some(ORIGINAL_RATE)
        || semantic.generated_presents.len() != 2
        || frames.len() != 2
    {
        return Err(io::Error::other(format!(
            "Extended interpolation semantic evidence is incomplete: {semantic:?}, frames={} ",
            frames.len()
        ))
        .into());
    }
    for (index, (generated, frame)) in semantic
        .generated_presents
        .iter()
        .zip(frames.iter())
        .enumerate()
    {
        if generated.previous_workload_id == generated.current_workload_id
            || generated.current_workload_id != semantic.workload_id
            || generated.present_id != semantic.present_id
            || generated.presentation_ordinal != index as u32
            || generated.interpolation_numerator != index as u32 + 1
            || generated.interpolation_denominator != 2
            || generated.original_refresh_rate != u32::from(ORIGINAL_RATE)
            || generated.target_refresh_rate != TARGET_RATE
            || frame.workload_id != semantic.workload_id
            || frame.present_id != semantic.present_id
            || frame.generated_ordinal != Some(index as u32)
            || frame.interpolation_numerator != index as u32 + 1
            || frame.interpolation_denominator != 2
        {
            return Err(io::Error::other(format!(
                "Extended generated-frame provenance drifted at {index}: {generated:?}, frame={frame:?}"
            ))
            .into());
        }
    }
    let midpoint = red_shape(&frames[0])?;
    let current = red_shape(&frames[1])?;
    if previous.count == 0
        || previous.count != midpoint.count
        || midpoint.count != current.count
        || !(previous.min_x < midpoint.min_x && midpoint.min_x < current.min_x)
        || !(previous.max_x < midpoint.max_x && midpoint.max_x < current.max_x)
        || !(previous.sum_x < midpoint.sum_x && midpoint.sum_x < current.sum_x)
        || digest(&frames[0]) == digest(&frames[1])
    {
        return Err(io::Error::other(format!(
            "Extended matrix-group interpolation lacks an ordered spatial midpoint: {previous:?}->{midpoint:?}->{current:?}"
        ))
        .into());
    }
    require_guards(&rdram, "Extended interpolation")?;
    require_production_rejection(&mut backend)?;
    Ok(InterpolationEvidence {
        semantic,
        previous,
        midpoint,
        current,
        midpoint_digest: digest(&frames[0]),
        current_digest: digest(&frames[1]),
    })
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum DepthSeed {
    Near,
    Far,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VertexEvidence {
    seed: DepthSeed,
    enabled: bool,
    semantic: Rt64ExtendedGbiEvidence,
    shape: Shape,
    digest: [u8; 32],
}

fn vertex_case(seed: DepthSeed, enabled: bool) -> Result<VertexEvidence, Box<dyn Error>> {
    let version = negotiated_version()?;
    let mut backend = backend(RenderRefreshRate::Original)?;
    require_production_rejection(&mut backend)?;
    let mut rdram = vec![0; RDRAM_LEN];
    arm_guards(&mut rdram);
    let depth_value = match seed {
        DepthSeed::Near => 0x0000,
        DepthSeed::Far => 0x7ffc,
    };
    backend.enable_extended_gbi_evidence()?;
    submit_scene(
        &mut backend,
        version,
        &mut rdram,
        SceneSubmission {
            translation_x: 0.0,
            test_z: 0,
            vertex_z: enabled,
            depth_value,
            seed: 11,
        },
    )?;
    let semantic = backend.extended_gbi_evidence()?;
    let frames = backend.extended_presented_pixels()?;
    validate_groups(&semantic)?;
    if frames.len() != 1
        || frames[0].generated_ordinal.is_some()
        || !semantic.generated_presents.is_empty()
    {
        return Err(
            io::Error::other("Original-rate vertex-Z control generated extra frames").into(),
        );
    }
    if enabled {
        if semantic.command_counts[0x0a] != 1
            || semantic.command_counts[0x0b] != 1
            || semantic.vertex_z.len() != 2
            || semantic.vertex_z[0].marker_kind != Rt64VertexZMarkerKind::Begin
            || semantic.vertex_z[0].command_vertex_index != Some(0)
            || semantic.vertex_z[0].resolved_source_index != 0
            || semantic.vertex_z[0].affected_face_index_count != 3
            || semantic.vertex_z[1].marker_kind != Rt64VertexZMarkerKind::End
            || semantic.vertex_z[1].command_vertex_index.is_some()
            || semantic.vertex_z[1].resolved_source_index != 0
            || semantic.vertex_z[1].affected_face_index_start
                != semantic.vertex_z[0].affected_face_index_start
            || semantic.vertex_z[1].affected_face_index_count != 3
        {
            return Err(io::Error::other(format!(
                "typed Extended vertex-Z marker evidence drifted: {:?}",
                semantic.vertex_z
            ))
            .into());
        }
    } else if semantic.command_counts[0x0a] != 0
        || semantic.command_counts[0x0b] != 0
        || !semantic.vertex_z.is_empty()
    {
        return Err(io::Error::other("disabled vertex-Z control emitted marker evidence").into());
    }
    let shape = red_shape(&frames[0])?;
    require_guards(&rdram, "Extended vertex-Z")?;
    require_production_rejection(&mut backend)?;
    Ok(VertexEvidence {
        seed,
        enabled,
        semantic,
        shape,
        digest: digest(&frames[0]),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SuiteEvidence {
    interpolation: InterpolationEvidence,
    far_control: VertexEvidence,
    far_enabled: VertexEvidence,
    near_control: VertexEvidence,
    near_enabled: VertexEvidence,
}

fn run_once() -> Result<SuiteEvidence, Box<dyn Error>> {
    let interpolation = interpolation_case()?;
    let far_control = vertex_case(DepthSeed::Far, false)?;
    let far_enabled = vertex_case(DepthSeed::Far, true)?;
    let near_control = vertex_case(DepthSeed::Near, false)?;
    let near_enabled = vertex_case(DepthSeed::Near, true)?;
    if far_control.shape.count == 0
        || far_enabled.shape != far_control.shape
        || far_enabled.digest != far_control.digest
        || near_control.shape != far_control.shape
        || near_control.digest != far_control.digest
        || near_enabled.shape.count != 0
        || near_enabled.digest == near_control.digest
    {
        return Err(io::Error::other(format!(
            "Extended vertex-Z visible/occluded pixels are not causal: far_control={far_control:?}, far_enabled={far_enabled:?}, near_control={near_control:?}, near_enabled={near_enabled:?}"
        ))
        .into());
    }
    let expected_previous = Shape {
        count: 161,
        min_x: 17,
        max_x: 35,
        min_y: 15,
        max_y: 29,
        sum_x: 4186,
    };
    let expected_midpoint = Shape {
        count: 161,
        min_x: 21,
        max_x: 39,
        min_y: 15,
        max_y: 29,
        sum_x: 4830,
    };
    let expected_current = Shape {
        count: 161,
        min_x: 23,
        max_x: 41,
        min_y: 15,
        max_y: 29,
        sum_x: 5152,
    };
    if interpolation.previous != expected_previous
        || interpolation.midpoint != expected_midpoint
        || interpolation.current != expected_current
        || hex(&interpolation.midpoint_digest)
            != "af5e25c1f10351d0fddb503a545a173c2167bcf137ab9df83ab43b6b86dc45b0"
        || hex(&interpolation.current_digest)
            != "b7116e2234e90cc2eaa468cd8506204c1015285bcb03c5b5672c118c38b22e61"
        || hex(&far_enabled.digest)
            != "b7116e2234e90cc2eaa468cd8506204c1015285bcb03c5b5672c118c38b22e61"
        || hex(&near_enabled.digest)
            != "5e9d5b686a0c4122c77d69144a5874dcbbcd05c1b4717bb11d8d730cc17a6659"
    {
        return Err(io::Error::other(format!(
            "Extended closure exact Metal evidence drifted: interpolation={interpolation:?}, far={far_enabled:?}, near={near_enabled:?}"
        ))
        .into());
    }
    Ok(SuiteEvidence {
        interpolation,
        far_control,
        far_enabled,
        near_control,
        near_enabled,
    })
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.source_overlay_id != PINNED_OVERLAY
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "Extended closure requires clean pinned Metal RT64: {identity:?}"
        ))
        .into());
    }
    let evidence = run_once()?;
    println!(
        "RT64 Extended enhancement closure passed: midpoint={} endpoint={} visible={} occluded={} shapes={:?}->{:?}->{:?}",
        hex(&evidence.interpolation.midpoint_digest),
        hex(&evidence.interpolation.current_digest),
        hex(&evidence.far_enabled.digest),
        hex(&evidence.near_enabled.digest),
        evidence.interpolation.previous,
        evidence.interpolation.midpoint,
        evidence.interpolation.current,
    );
    fn64_boot_harness::emit_rt64_platform_child_identity(
        identity.source_id,
        identity.is_source_authoritative(),
        identity.adapter_source_sha256,
        identity.post_vi_api,
    )?;
    Ok(())
}
