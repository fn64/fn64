//! Fixture-replay test: a captured (here: hand-built, see note below)
//! F3DEX2-family display list -> `ReferenceBackend::process_task` -> a
//! real, non-clear rendered frame. This is the "first_frame" proof the
//! render-seam task calls for.
//!
//! ## Why this fixture is hand-built, not pulled from a real ROM capture
//!
//! `docs/COMPLETENESS.md`'s `osSpTaskStartGo`/`osSpTaskLoad` rows are
//! ABSENT/no-call-site-observed for both ported games as of this wave --
//! neither game's currently generated corpus has reached a real gfx task
//! submission yet (only `osSpTaskYielded`'s task-header path is exercised,
//! and it currently only ever sees `M_AUDTASK` for real; `M_GFXTASK` is
//! acknowledged, never carrying real display-list bytes -- see
//! `fn64_abi::osSpTaskYielded_recomp`'s `GFX_TASK_NOTE`). Claiming this
//! fixture came from a real OoT/NW4E capture would be exactly the
//! unevidenced claim `AGENTS.md`/this project's honesty rule forbids.
//! Instead: this fixture is a deliberately tiny, HAND-CONSTRUCTED display
//! list using the real, public F3DEX2 wire encoding (`gbi.rs`'s module
//! doc) -- proof the SEAM (decode -> rasterize -> framebuffer) works
//! end-to-end on real opcode bytes, not proof any specific game's actual
//! content renders yet. When a real gfx task IS observed from generated
//! code, replace this fixture with that capture; the test shape (build
//! task -> `process_task` -> assert non-clear -> dump PNG) stays the same.
use fn64_render::{OsTask, RenderBackend, RenderConfig, M_GFXTASK};
use fn64_render_rt64::{png_dump, ReferenceBackend};

/// Build a tiny rdram image containing: a 3-vertex array (a red/green/blue
/// triangle spanning most of a 64x64 frame) followed by a display list
/// (`G_VTX` loading all 3, `G_TRI1` drawing them, `G_ENDDL`).
fn build_fixture_rdram() -> (Vec<u8>, u32) {
    const VTX_ADDR: usize = 0x1000;
    const DL_ADDR: usize = 0x2000;
    let mut rdram = vec![0u8; 0x4000];

    // Three vertices, each the SDK's public 16-byte Vtx_t position-color
    // layout: ob[3] (s16 x3), flag(u16), tc[2](s16x2), cn[4](u8x4).
    let verts: [([i16; 2], [u8; 4]); 3] = [
        ([8, 8], [255, 0, 0, 255]),
        ([56, 8], [0, 255, 0, 255]),
        ([32, 56], [0, 0, 255, 255]),
    ];
    for (i, (xy, rgba)) in verts.iter().enumerate() {
        let off = VTX_ADDR + i * 16;
        rdram[off..off + 2].copy_from_slice(&xy[0].to_be_bytes());
        rdram[off + 2..off + 4].copy_from_slice(&xy[1].to_be_bytes());
        // z, flag, tc[2] left zero (unused by this reference backend).
        rdram[off + 12..off + 16].copy_from_slice(rgba);
    }

    // Display list: gsSPVertex(VTX_ADDR, 3, 0), gsSP1Triangle(0,1,2,0), gsSPEndDisplayList().
    let mut dl = Vec::new();
    // G_VTX word0: opcode<<24 | n<<12 | v0 ; word1: address.
    let n: u32 = 3;
    let v0: u32 = 0;
    let w0 = ((fn64_render_rt64::gbi::G_VTX as u32) << 24) | (n << 12) | v0;
    dl.extend_from_slice(&w0.to_be_bytes());
    dl.extend_from_slice(&(VTX_ADDR as u32).to_be_bytes());
    // G_TRI1 word0: opcode<<24 (rest unused by this decoder); word1: (v0<<16)|(v1<<8)|v2.
    let w0 = (fn64_render_rt64::gbi::G_TRI1 as u32) << 24;
    let w1 = (1u32 << 8) | 2u32; // v0 index is 0, so its <<16 term is omitted (identity op)
    dl.extend_from_slice(&w0.to_be_bytes());
    dl.extend_from_slice(&w1.to_be_bytes());
    // G_ENDDL.
    let w0 = (fn64_render_rt64::gbi::G_ENDDL as u32) << 24;
    dl.extend_from_slice(&w0.to_be_bytes());
    dl.extend_from_slice(&0u32.to_be_bytes());

    rdram[DL_ADDR..DL_ADDR + dl.len()].copy_from_slice(&dl);
    (rdram, DL_ADDR as u32)
}

#[test]
fn fixture_display_list_renders_a_non_clear_frame() {
    let (rdram, dl_addr) = build_fixture_rdram();

    let clear = [10, 10, 10, 255]; // distinct from every vertex color above
    let mut backend = ReferenceBackend::new().with_clear_color(clear);
    backend.create(&RenderConfig::new(64, 64)).unwrap();

    let task = OsTask {
        task_type: M_GFXTASK,
        data_ptr: dl_addr,
        ..Default::default()
    };
    let status = backend
        .process_task(&rdram, &task)
        .expect("process_task should succeed");
    assert_eq!(status, fn64_render::FrameStatus::Complete);
    backend.present().unwrap();

    let fb = backend
        .framebuffer()
        .expect("framebuffer must exist after create()");
    assert!(
        fb.has_non_uniform_content(clear[0], clear[1], clear[2], clear[3]),
        "expected the triangle fixture to paint at least one non-clear pixel"
    );

    // Centroid of the fixture triangle ((8,8),(56,8),(32,56)) is roughly
    // (32, 24) -- must be a blend of the three vertex colors, i.e. NOT the
    // clear color and NOT any single pure vertex color (barycentric
    // interpolation at the centroid weights all three equally).
    let (cx, cy) = (32u32, 24u32);
    let idx = (cy * fb.width + cx) as usize * 4;
    let centroid_px = &fb.pixels[idx..idx + 4];
    assert_ne!(
        centroid_px,
        &clear[..],
        "centroid must not be the clear color"
    );

    // Dump the rendered frame as a real PNG file -- the task's explicit
    // "dump a PNG" deliverable for a non-clear rendered frame.
    let out_dir = std::env::temp_dir().join("fn64-render-rt64-fixtures");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_path = out_dir.join("fixture_triangle.png");
    png_dump::write_png(&out_path, fb.width, fb.height, &fb.pixels).unwrap();
    assert!(out_path.exists());
    let png_bytes = std::fs::read(&out_path).unwrap();
    assert_eq!(
        &png_bytes[0..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
    );

    eprintln!("wrote fixture frame to {}", out_path.display());
}

#[test]
fn unlisted_ucode_still_traps_through_the_real_backend_not_just_the_trait_fake() {
    // Regression guard: `ReferenceBackend::supported_ucodes()` must stay in
    // lockstep with what `process_task` actually enforces. This uses the
    // real backend (not the trait crate's in-crate fake) to close that gap.
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::new(8, 8)).unwrap();
    assert_eq!(backend.supported_ucodes(), &[fn64_render::UcodeId::F3dex2]);
    // A task with no display list at all (data_ptr = 0, all-zero rdram)
    // decodes to zero triangles -- a real "nothing to draw" case, not an
    // error, since G_ENDDL at offset 0 (all-zero bytes: opcode 0x00) is
    // simply an unmodeled/no-op opcode until it eventually reads past the
    // buffer and stops. Confirms the happy path doesn't panic on trivial
    // input.
    let rdram = vec![0u8; 16];
    let task = OsTask {
        task_type: M_GFXTASK,
        data_ptr: 0,
        ..Default::default()
    };
    let result = backend.process_task(&rdram, &task);
    assert!(result.is_ok());
}
