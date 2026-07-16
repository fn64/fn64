//! Snapshot test for render output, using `insta`. Instead of hand-maintaining
//! a golden PNG (huge, opaque diffs), this captures a STRUCTURAL DIGEST of the
//! rendered frame — dimensions, non-clear pixel count, a stable framebuffer
//! hash, and a few sampled pixels — as a reviewable text snapshot. A render
//! regression changes the digest and fails the test; an INTENTIONAL render
//! change is blessed with `cargo insta review` instead of editing a golden by
//! hand. This is the maintainable version of the golden-frame oracle.
//!
//! The fixture is the same hand-built F3DEX2 triangle as `fixture_replay.rs`
//! (see its doc comment for why it's synthetic, not a ROM capture).
use fn64_render::{OsTask, RenderBackend, RenderConfig, M_GFXTASK};
use fn64_render_rt64::{gbi, ReferenceBackend};

// Match fixture_replay.rs's WORKING layout exactly (vtx at 0x1000, DL at 0x2000,
// and G_TRI1 word1 = (v0<<16)|(v1<<8)|v2). Getting these wrong renders a blank
// frame — the snapshot would then green-check emptiness, the exact weak-check
// anti-pattern. The digest's non_clear_pixels > 0 assert guards against it.
const VTX_ADDR: usize = 0x1000;
const DL_ADDR: usize = 0x2000;

/// Build the tiny triangle display list (mirrors fixture_replay's fixture).
fn build_fixture_rdram() -> (Vec<u8>, u32) {
    let mut rdram = vec![0u8; 0x4000];
    let verts: [(i16, i16, i16, [u8; 4]); 3] = [
        (8, 8, 0, [255, 0, 0, 255]),
        (56, 8, 0, [0, 255, 0, 255]),
        (32, 56, 0, [0, 0, 255, 255]),
    ];
    for (i, &(x, y, z, c)) in verts.iter().enumerate() {
        let o = VTX_ADDR + i * 16;
        rdram[o..o + 2].copy_from_slice(&x.to_be_bytes());
        rdram[o + 2..o + 4].copy_from_slice(&y.to_be_bytes());
        rdram[o + 4..o + 6].copy_from_slice(&z.to_be_bytes());
        rdram[o + 12..o + 16].copy_from_slice(&c);
    }
    let mut dl = Vec::new();
    let (n, v0) = (3u32, 0u32);
    let w0 = ((gbi::G_VTX as u32) << 24) | (n << 12) | v0;
    dl.extend_from_slice(&w0.to_be_bytes());
    dl.extend_from_slice(&(VTX_ADDR as u32).to_be_bytes());
    let w0 = (gbi::G_TRI1 as u32) << 24;
    let w1 = (1u32 << 8) | 2u32; // (v0<<16)|(v1<<8)|v2 with v0=0
    dl.extend_from_slice(&w0.to_be_bytes());
    dl.extend_from_slice(&w1.to_be_bytes());
    let w0 = (gbi::G_ENDDL as u32) << 24;
    dl.extend_from_slice(&w0.to_be_bytes());
    dl.extend_from_slice(&0u32.to_be_bytes());
    rdram[DL_ADDR..DL_ADDR + dl.len()].copy_from_slice(&dl);
    (rdram, DL_ADDR as u32)
}

/// A stable, human-readable digest of a rendered framebuffer. Deterministic
/// per render, so a regression is a visible one-line diff in the snapshot.
fn framebuffer_digest(fb: &fn64_render_rt64::raster::Framebuffer, clear: [u8; 4]) -> String {
    let non_clear = fb
        .pixels
        .chunks_exact(4)
        .filter(|px| *px != clear)
        .count();
    // FNV-1a over the pixels — a stable content hash without pulling a crate.
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in &fb.pixels {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Sample the triangle centroid + a corner (expected clear).
    let centroid = {
        let (cx, cy) = (32u32, 24u32);
        let i = (cy * fb.width + cx) as usize * 4;
        &fb.pixels[i..i + 4]
    };
    let corner = &fb.pixels[0..4];
    format!(
        "dims: {}x{}\nnon_clear_pixels: {}\nfb_fnv1a: {:016x}\ncentroid_rgba: {:?}\ncorner_rgba: {:?}",
        fb.width, fb.height, non_clear, hash, centroid, corner
    )
}

#[test]
fn fixture_triangle_render_digest_snapshot() {
    let (mut rdram, dl_addr) = build_fixture_rdram();
    let clear = [10, 10, 10, 255];
    let mut backend = ReferenceBackend::new().with_clear_color(clear);
    backend.create(&RenderConfig::new(64, 64)).unwrap();
    let task = OsTask {
        task_type: M_GFXTASK,
        data_ptr: dl_addr,
        ..Default::default()
    };
    backend.process_task(&mut rdram, &task, 0).unwrap();
    backend.present().unwrap();
    let fb = backend.framebuffer().expect("framebuffer after create()");

    // Guard against a degenerate fixture: a blank frame must FAIL loudly, never
    // be snapshotted as green emptiness (weak-check anti-pattern).
    let non_clear = fb.pixels.chunks_exact(4).filter(|px| *px != clear).count();
    assert!(non_clear > 0, "fixture rendered a blank frame -- fixture is broken, not a valid snapshot");

    // The reviewable golden: `cargo insta review` blesses an intentional
    // render change; a regression fails with a one-line digest diff.
    insta::assert_snapshot!("fixture_triangle_render", framebuffer_digest(fb, clear));
}
