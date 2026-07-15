//! `fn64-render-rt64`: the `RenderBackend` adapter crate reserved for RT64
//! (MIT, C++) interop, per `docs/DESIGN.md` section 1 ("the ONLY crate in
//! the workspace permitted to contain C++ or call into RT64's C++ API")
//! and `docs/DECOUPLING.md`'s renderer-seam sequencing step 3 (this crate
//! IS the renamed `fn64-rt64`, per role).
//!
//! ## Honest status (read before assuming RT64 is wired up)
//!
//! **RT64 FFI is NOT live.** [`Rt64Backend`] is a named, loud stub: every
//! method returns `RenderError::NotReady`/`Backend` with a clear TODO
//! pointing at the exact blocker (see its doc comment) -- it exists so the
//! trait-shaped call site in `fn64-shell` has something to construct today
//! without lying about what it does. Building real RT64 FFI needs (a) the
//! MIT RT64 fork actually vendored/built (`docs/DECOUPLING.md`'s "refs has
//! RT64 via the game repos; or clone github fn64/rt64" -- not done in this
//! wave), and (b) the gfx task handoff signature, which the predecessor
//! `fn64-rt64` README already flagged as unresolved (no `osSpTaskLoad`/
//! `osSpTaskStartGo` call site observed yet in either game's generated
//! corpus, per `docs/COMPLETENESS.md` row `osSpTaskStartGo`). Neither
//! blocker is invented or hand-waved here.
//!
//! **What IS real and tested:** [`ReferenceBackend`], a headless, pure-Rust
//! software rasterizer implementing the full `RenderBackend` trait against
//! a real (if intentionally small) F3DEX2-family display-list subset
//! (`gbi.rs`) and a real scanline rasterizer (`raster.rs`). It proves the
//! seam end-to-end: a captured `OsTask` pointing at a display list in
//! `rdram` produces actual non-clear pixels in an output framebuffer,
//! dumpable as a PNG (`png_dump.rs`). See `tests/fixture_replay.rs` for the
//! fixture-replay test and the produced frame.
//!
//! This crate's dependency on `fn64-runtime` is for shared address-space
//! types ONLY (per `docs/DECOUPLING.md`: "for the shared types the gfx
//! task handoff needs to name") -- neither backend here calls back into
//! `fn64_runtime::Executor` or any other runtime state; both only ever see
//! `&[u8]` (rdram) and an `fn64_render::OsTask` value, matching the trait's
//! contract.

pub mod gbi;
pub mod png_dump;
pub mod raster;

// Named (not `_`-discarded) so the intended dependency-for-shared-types-only
// relationship documented above is visible to `cargo tree`/reviewers, even
// though no runtime type is used by name in this crate yet (see module doc:
// both backends only ever see `&[u8]` + `fn64_render::OsTask`, never an
// `fn64_runtime` type directly). Kept as an explicit workspace edge, not
// removed, per `docs/DECOUPLING.md` step 3's sequencing.
#[allow(unused_imports)]
use fn64_runtime as _;

use fn64_render::{FrameStatus, OsTask, RenderBackend, RenderConfig, RenderError, UcodeId};
use raster::Framebuffer;

/// A headless software `RenderBackend`: decodes a small F3DEX2-family
/// display-list subset (`gbi::decode_display_list`) and rasterizes it
/// (`raster::Framebuffer::draw_triangle`) into an off-screen RGBA8888
/// buffer. "Reference" in the sense of "the thing every future real backend
/// (RT64 adapter, wgpu HLE) can be A/B-diffed against for seam-level
/// correctness" -- not a claim of RDP-accurate output (see module doc).
/// Which display-list encoding `process_task` decodes with.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DecodeMode {
    /// The original simple F3D-style reference-fixture encoding
    /// (`gbi::decode_display_list`): raw screen-space `ob` coords,
    /// non-segmented `w1` addresses, `n<<12|v0` vertex packing. This is what
    /// the hand-built fixtures and the `fn64-abi` executor-seam test plant,
    /// so it stays the DEFAULT to keep those working bit-for-bit.
    Simple,
    /// Real F3DEX2 (`gbi::decode_display_list_f3dex2`): segment table,
    /// modelview/projection matrix stack, viewport, nested `G_DL`. Selected
    /// for decoding actual OoT display lists.
    F3dex2,
}

pub struct ReferenceBackend {
    fb: Option<Framebuffer>,
    clear_color: [u8; 4],
    decode_mode: DecodeMode,
    /// If set, `process_task` writes the rasterized framebuffer to
    /// `<dir>/<prefix>-NNNN.png` after each task, and logs whether the frame
    /// was non-clear. This is how a harness that MOVED the backend into
    /// `fn64_abi::set_render_backend` (giving up its `&mut` handle, since the
    /// `dyn RenderBackend` trait object is deliberately not `Any`-downcastable
    /// per docs/DECOUPLING.md) still gets the rasterized output back out:
    /// the backend dumps it itself. Bounded by `auto_dump_limit`.
    auto_dump: Option<AutoDump>,
}

struct AutoDump {
    dir: std::path::PathBuf,
    prefix: String,
    /// How many gfx tasks have been processed (the PNG index).
    task_index: u64,
    /// How many non-clear PNGs have actually been written.
    written: u64,
    /// Stop dumping after this many non-clear frames (avoid flooding /tmp on
    /// a long boot). `u64::MAX` = unbounded.
    limit: u64,
}

impl ReferenceBackend {
    pub fn new() -> Self {
        ReferenceBackend {
            fb: None,
            clear_color: [0, 0, 0, 255],
            decode_mode: DecodeMode::Simple,
            auto_dump: None,
        }
    }

    /// Decode subsequent display lists as real F3DEX2 (matrix stack, segment
    /// table, viewport) instead of the simple reference-fixture encoding.
    /// Used by the OoT boot harness, whose display lists are genuine F3DEX2.
    pub fn with_f3dex2(mut self) -> Self {
        self.decode_mode = DecodeMode::F3dex2;
        self
    }

    /// After each `process_task`, write the rasterized framebuffer to
    /// `<dir>/<prefix>-NNNN.png` (NNNN = the non-clear-frame counter),
    /// stopping after `limit` non-clear frames. This lets a harness recover
    /// the backend's output even after `set_render_backend` has taken
    /// ownership of it. Every dump (and every all-clear skip) is logged so a
    /// blank boot is reported honestly, never faked.
    pub fn with_auto_dump(
        mut self,
        dir: impl Into<std::path::PathBuf>,
        prefix: impl Into<String>,
        limit: u64,
    ) -> Self {
        self.auto_dump = Some(AutoDump {
            dir: dir.into(),
            prefix: prefix.into(),
            task_index: 0,
            written: 0,
            limit,
        });
        self
    }

    /// Override the clear color a fresh/resized framebuffer starts from.
    /// Exposed mainly so tests can pick a clear color that's unambiguously
    /// distinct from any triangle color in a fixture, making "did geometry
    /// actually render" trivial to assert.
    pub fn with_clear_color(mut self, rgba: [u8; 4]) -> Self {
        self.clear_color = rgba;
        self
    }

    /// The current framebuffer's raw RGBA8888 pixels, for a test/harness to
    /// inspect or dump (`png_dump::write_png`). `None` before `create`.
    pub fn framebuffer(&self) -> Option<&Framebuffer> {
        self.fb.as_ref()
    }
}

impl Default for ReferenceBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderBackend for ReferenceBackend {
    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        let mut fb = Framebuffer::new(cfg.width, cfg.height);
        let [r, g, b, a] = self.clear_color;
        fb.clear(r, g, b, a);
        self.fb = Some(fb);
        Ok(())
    }

    fn process_task(&mut self, rdram: &[u8], task: &OsTask) -> Result<FrameStatus, RenderError> {
        let fb = self
            .fb
            .as_mut()
            .ok_or(RenderError::NotReady("create() not called"))?;

        if !gbi::SUPPORTED.contains(&UcodeId::F3dex2) {
            // Unreachable given this backend's fixed SUPPORTED list, but
            // written as a real check (not `unreachable!()`) so
            // `supported_ucodes()` and the enforcement path can never drift
            // apart silently if SUPPORTED ever grows a second variant.
            return Err(RenderError::UnsupportedUcode {
                ucode_addr: task.ucode,
            });
        }

        // `output_buff`/`output_buff_size` are the RSP's DRAM output region.
        // CRITICAL (was a blank-frame bug): in the public `OSTask_t` layout
        // (`ultra64/task.h`) `output_buff_size` is declared `u64*` -- a
        // POINTER TO THE END of the output buffer, NOT a byte count. OoT
        // fills both as KSEG0 pointers (`output_buff=0x80151640`,
        // `output_buff_size=0x80169640`, verified from a live task header),
        // so the real byte length is `end_ptr - start_ptr`, and the previous
        // code (`out_phys + output_buff_size`) computed
        // `0x151640 + 0x80169640` -> way past rdram, tripping
        // InvalidTaskBounds on EVERY real frame and returning before any
        // decode ran (0 triangles, blank frame). We now mask BOTH pointers
        // to physical offsets and validate the END offset against rdram.
        // (The reference backend rasterizes into its own framebuffer and
        // decodes from `data_ptr`, so this is only a sanity bound, not a
        // region the decoder reads -- but a wrong bound must not veto the
        // whole frame.)
        let out_start = (task.output_buff & 0x00FF_FFFF) as usize;
        let out_end = (task.output_buff_size & 0x00FF_FFFF) as usize;
        if task.output_buff_size != 0 && out_end > rdram.len() {
            return Err(RenderError::InvalidTaskBounds {
                offset: task.output_buff,
                len: out_end.saturating_sub(out_start) as u32,
                rdram_len: rdram.len(),
            });
        }

        let triangles = match self.decode_mode {
            DecodeMode::Simple => gbi::decode_display_list(rdram, task.data_ptr)?,
            DecodeMode::F3dex2 => gbi::decode_display_list_f3dex2(rdram, task.data_ptr)?,
        };
        let tri_count = triangles.len();
        match self.decode_mode {
            // Simple reference-fixture path: pure 2D fill, no culling/z-test,
            // to keep the hand-built fixtures bit-compatible.
            DecodeMode::Simple => {
                for tri in &triangles {
                    fb.draw_triangle(tri);
                }
            }
            // Real F3DEX2 scene path: honor per-triangle back/front-face
            // culling (from G_GEOMETRYMODE) + z-buffering + texture sampling,
            // so far geometry is occluded, inside-out back faces don't
            // overpaint front faces, and textured surfaces show their texels.
            DecodeMode::F3dex2 => {
                for tri in &triangles {
                    fb.draw_triangle_culled(tri, tri.cull);
                }
            }
        }

        // Auto-dump the rasterized frame if configured (the harness's only
        // way to see the output once set_render_backend owns this backend).
        if let Some(dump) = self.auto_dump.as_mut() {
            let idx = dump.task_index;
            dump.task_index += 1;
            let [cr, cg, cb, ca] = self.clear_color;
            let non_clear = fb.has_non_uniform_content(cr, cg, cb, ca);
            if !non_clear {
                eprintln!(
                    "[fn64-render-rt64] gfx task #{idx}: decoded {tri_count} triangle(s); \
                     framebuffer is UNIFORM clear -- reported blank, not dumped."
                );
            } else if dump.written >= dump.limit {
                eprintln!(
                    "[fn64-render-rt64] gfx task #{idx}: non-clear ({tri_count} tris) but \
                     auto-dump limit ({}) reached -- not writing another PNG.",
                    dump.limit
                );
            } else {
                let _ = std::fs::create_dir_all(&dump.dir);
                let path = dump
                    .dir
                    .join(format!("{}-{:04}.png", dump.prefix, dump.written));
                match png_dump::write_png(&path, fb.width, fb.height, &fb.pixels) {
                    Ok(()) => {
                        dump.written += 1;
                        eprintln!(
                            "[fn64-render-rt64] gfx task #{idx}: NON-CLEAR ({tri_count} tris) \
                             -- dumped {}",
                            path.display()
                        );
                    }
                    Err(e) => eprintln!(
                        "[fn64-render-rt64] gfx task #{idx}: failed to write {}: {e}",
                        path.display()
                    ),
                }
            }
        }
        Ok(FrameStatus::Complete)
    }

    fn present(&mut self) -> Result<(), RenderError> {
        if self.fb.is_none() {
            return Err(RenderError::NotReady("create() not called"));
        }
        Ok(())
    }

    fn resize(&mut self, w: u32, h: u32) {
        let clear_color = self.clear_color;
        if let Some(fb) = &mut self.fb {
            let mut new_fb = Framebuffer::new(w, h);
            new_fb.clear(
                clear_color[0],
                clear_color[1],
                clear_color[2],
                clear_color[3],
            );
            *fb = new_fb;
        }
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        gbi::SUPPORTED
    }
}

/// The RT64 adapter -- see module doc's "Honest status" section. Every
/// method is a named, loud stub: no C++ is linked, no window is opened, no
/// frame is ever produced. This is intentionally NOT a silent no-op that
/// looks like it might work; every call returns an error that names exactly
/// what's missing, so a caller can never mistake this for a working
/// backend.
///
/// TODO(rt64-ffi): real implementation needs
/// (1) the MIT RT64 fork vendored + built (`docs/DECOUPLING.md` step 3's
///     "clone github fn64/rt64", not done),
/// (2) a resolved gfx task handoff signature (blocked on a `profile.toml`
///     rename wave reaching `osSpTaskLoad`/`osSpTaskStartGo`, per
///     `docs/COMPLETENESS.md`),
/// (3) `cxx`/bindgen-generated FFI bindings living ONLY in this crate per
///     `docs/DESIGN.md` section 1's C++ quarantine rule.
pub struct Rt64Backend {
    created: bool,
}

impl Rt64Backend {
    pub fn new() -> Self {
        Rt64Backend { created: false }
    }
}

impl Default for Rt64Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderBackend for Rt64Backend {
    fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
        // TODO(rt64-ffi): open a real RT64 device/window here once the
        // fork is vendored. Until then this is a named stub, not a
        // pretend-success no-op -- see struct doc comment.
        self.created = false;
        Err(RenderError::Backend {
            backend: "rt64",
            reason: "RT64 FFI is not wired up yet; see Rt64Backend's doc comment for the two \
                     concrete blockers (fork not vendored, gfx task handoff signature unresolved)"
                .to_string(),
        })
    }

    fn process_task(&mut self, _rdram: &[u8], _task: &OsTask) -> Result<FrameStatus, RenderError> {
        Err(RenderError::NotReady(
            "Rt64Backend::create was never able to succeed (RT64 FFI not wired up)",
        ))
    }

    fn present(&mut self) -> Result<(), RenderError> {
        Err(RenderError::NotReady(
            "Rt64Backend::create was never able to succeed (RT64 FFI not wired up)",
        ))
    }

    fn resize(&mut self, _w: u32, _h: u32) {}

    fn supported_ucodes(&self) -> &[UcodeId] {
        &[] // deliberately empty: this backend supports nothing yet.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rt64_backend_create_is_a_named_stub_not_a_silent_success() {
        let mut backend = Rt64Backend::new();
        let err = backend.create(&RenderConfig::new(320, 240)).unwrap_err();
        match err {
            RenderError::Backend { backend, .. } => assert_eq!(backend, "rt64"),
            other => panic!("expected Backend stub error, got {other:?}"),
        }
        assert!(!backend.created);
        assert!(backend.supported_ucodes().is_empty());
    }

    #[test]
    fn reference_backend_create_then_present_succeeds_with_no_geometry() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend.present().unwrap();
        assert!(!backend
            .framebuffer()
            .unwrap()
            .has_non_uniform_content(0, 0, 0, 255));
    }

    #[test]
    fn reference_backend_rejects_process_task_before_create() {
        let mut backend = ReferenceBackend::new();
        let rdram = vec![0u8; 64];
        let err = backend
            .process_task(&rdram, &OsTask::default())
            .unwrap_err();
        assert!(matches!(err, RenderError::NotReady(_)));
    }
}
