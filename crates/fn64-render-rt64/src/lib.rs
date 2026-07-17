//! `fn64-render-rt64`: the `RenderBackend` adapter crate reserved for RT64
//! (MIT, C++) interop, per `docs/DESIGN.md` section 1 ("the ONLY crate in
//! the workspace permitted to contain C++ or call into RT64's C++ API")
//! and `docs/DECOUPLING.md`'s renderer-seam sequencing step 3 (this crate
//! IS the renamed `fn64-rt64`, per role).
//!
//! ## RT64 feature boundary
//!
//! [`Rt64Backend`] is live when this crate's opt-in `rt64` feature is
//! enabled. Its build script compiles the sibling RT64 checkout's MIT
//! `rt64` target as a static library and links the crate-local C ABI shim;
//! the default build remains pure Rust and keeps [`ReferenceBackend`] as the
//! headless CI oracle. Building without `rt64` makes `Rt64Backend::create`
//! return a named error so a shell can fall back without pretending a GPU
//! backend exists.
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

/// Read a `FN64_*` debug knob, trapping if its retired `OOT_*` name is set.
///
/// These knobs are generic observability, not game-specific state; they were
/// renamed off the `OOT_` prefix so a second game cannot fork them (ROADMAP
/// H2b). An unset var means "feature off", so a bare rename would make an
/// existing `OOT_DUMP_PROJ=1` invocation silently do nothing -- the exact
/// silent shrug AGENTS.md bans. Every read of a renamed knob goes through
/// here so the old spelling stays loud instead of no-op.
#[cfg(not(test))]
pub(crate) fn debug_flag(name: &str) -> bool {
    let legacy = format!("OOT_{}", name.strip_prefix("FN64_").unwrap_or(name));
    assert!(
        std::env::var_os(&legacy).is_none(),
        "{legacy} was renamed to {name}; it is no longer read. Re-run with {name} set."
    );
    std::env::var_os(name).is_some()
}

#[cfg(feature = "rt64")]
mod ffi;

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
    /// Counts every gfx task this backend processes, independent of
    /// `auto_dump` being configured, so `FN64_GFX_TASK_DUMP` selects the same
    /// task index whether or not PNG auto-dumping is on.
    diag_task_index: u64,
}

struct AutoDump {
    dir: std::path::PathBuf,
    prefix: String,
    /// How many gfx tasks have been processed (the PNG index).
    task_index: u64,
    /// Do not write PNGs for tasks before this index. The task counter still
    /// advances, so a long-running harness can capture a bounded late window
    /// without flooding the output directory with boot frames.
    skip_before_task: u64,
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
            diag_task_index: 0,
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
            skip_before_task: 0,
            written: 0,
            limit,
        });
        self
    }

    /// Start auto-dumping at gfx task index `first_task`.
    ///
    /// Call this after [`Self::with_auto_dump`]. Tasks before the requested
    /// index are still rendered and written back to guest RDRAM; only their
    /// diagnostic PNG output is suppressed.
    pub fn with_auto_dump_skip(mut self, first_task: u64) -> Self {
        self.auto_dump
            .as_mut()
            .expect("with_auto_dump_skip requires with_auto_dump first")
            .skip_before_task = first_task;
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

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
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
            DecodeMode::Simple => gbi::decode_display_list(&*rdram, task.data_ptr)?,
            DecodeMode::F3dex2 => gbi::decode_display_list_f3dex2(&*rdram, task.data_ptr)?,
        };
        let tri_count = triangles.len();
        // The FN64_GFX_TASK_DUMP diagnostic was originally reachable only from
        // Rt64Backend, but the A/B lane-parity gate (scripts/lane-parity.sh)
        // runs the DEFAULT build, which has no `rt64` feature and therefore
        // lands here -- so the documented recipe silently produced no dump on
        // the exact configuration it was needed for. Same env-var contract,
        // same trace function; task indices are per-backend.
        #[cfg(not(test))]
        {
            let dump_index = self.diag_task_index;
            self.diag_task_index += 1;
            if let Some(spec) = std::env::var_os("FN64_GFX_TASK_DUMP") {
                let selected = spec.to_string_lossy().split(',').any(|entry| {
                    entry.trim().parse::<u64>().unwrap_or_else(|error| {
                        panic!("FN64_GFX_TASK_DUMP entry {entry:?} is not a u64 task index: {error}")
                    }) == dump_index
                });
                if selected {
                    let directory = std::env::var_os("FN64_GFX_TASK_DUMP_DIR")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/fn64-gfx-task-dumps"));
                    std::fs::create_dir_all(&directory).unwrap_or_else(|error| {
                        panic!("failed to create FN64_GFX_TASK_DUMP_DIR {directory:?}: {error}")
                    });
                    let command_trace = gbi::trace_display_list_f3dex2(&*rdram, task.data_ptr);
                    let report = format!(
                        "task_index={dump_index}\noutput_addr={output_addr:#010x}\n\
                         reference_triangle_count={tri_count}\ntask={task:#?}\n{command_trace}",
                    );
                    let path = directory.join(format!("task-{dump_index:04}.txt"));
                    std::fs::write(&path, report).unwrap_or_else(|error| {
                        panic!("failed to write gfx task diagnostic {path:?}: {error}")
                    });
                    eprintln!(
                        "[fn64-render-rt64] dumped gfx task #{dump_index} ({tri_count} reference \
                         triangles) to {path:?}"
                    );
                }
            }
        }
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
                // TEMP (env `FN64_NO_DEPTH=1`): force painter's-order (no
                // z-test) to A/B-prove the z-buffer is what produces correct
                // occlusion. Off by default; remove/keep behind the flag.
                #[cfg(not(test))]
                let no_depth = crate::debug_flag("FN64_NO_DEPTH");
                #[cfg(test)]
                let no_depth = false;
                for tri in &triangles {
                    if no_depth {
                        fb.draw_triangle_no_depth_culled(tri, tri.cull);
                    } else {
                        fb.draw_triangle_culled(tri, tri.cull);
                    }
                }
                #[cfg(not(test))]
                raster::zstat::summary();
            }
        }

        // Write the rasterized color image BACK into the game's framebuffer
        // in rdram, matching real RDP behavior (the RDP writes its color
        // image into DRAM, which the VI then scans out via osViSwapBuffer).
        // The reference backend rasterizes into its own RGBA8888 surface, so
        // here we convert to the framebuffer's native format and copy it into
        // `rdram[output_addr..]`. WITHOUT this, the backend's pixels never
        // reach the DRAM region the VI presents, so every VI frame is blank
        // even though rasterization succeeded.
        //
        // Target address (byte-cited): NOT `task.output_buff`. On OoT the gfx
        // task's `output_buff` is 0x80151640 (the RSP's DRAM command-FIFO
        // output region), whereas the color framebuffer the game actually
        // swaps/presents (`osViSwapBuffer`) is at 0x3b5000 / 0x3da800 -- a
        // different address. So the caller passes the VI's current framebuffer
        // offset as `output_addr`; `0` means "no known color target" (a
        // fixture/test path) and we skip the write-back.
        //
        // Format (byte-cited): RGBA5551, 16-bit, big-endian halfwords, exactly
        // matching `examples/oot-boot/src/main.rs`'s `dump_rgba5551_as_png`
        // (`u16::from_be_bytes`, r5=px>>11, g5=px>>6, b5=px>>1, a1=px&1). The
        // VI/harness reads the framebuffer as 2-byte big-endian RGBA5551, so
        // we store each pixel's halfword big-endian to match.
        if output_addr != 0 {
            write_rgba5551_framebuffer(rdram, output_addr as usize, fb);
        }

        // Auto-dump the rasterized frame if configured (the harness's only
        // way to see the output once set_render_backend owns this backend).
        if let Some(dump) = self.auto_dump.as_mut() {
            let idx = dump.task_index;
            dump.task_index += 1;
            if idx < dump.skip_before_task {
                return Ok(FrameStatus::Complete);
            }
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

/// Convert `fb`'s RGBA8888 pixels to N64 RGBA5551 and write them into
/// `rdram` starting at byte offset `start`, in the framebuffer's native
/// on-DRAM layout: 2 bytes per pixel, big-endian halfword, row-major,
/// top-left origin. This is the exact inverse of
/// `examples/oot-boot/src/main.rs`'s `dump_rgba5551_as_png`, which reads the
/// framebuffer back as `u16::from_be_bytes([b0, b1])` with `r5 = px >> 11`,
/// `g5 = px >> 6`, `b5 = px >> 1`, `a1 = px & 1` -- the same layout the VI
/// scans out. Storing the halfword big-endian (high byte at `start+2i`, low
/// at `start+2i+1`) makes the VI-presented frame match what the backend
/// rasterized. A pixel whose 2 bytes would run past `rdram` is skipped
/// (bounds-safe; the caller already validated `output_addr` is a real
/// framebuffer offset, but a wrong width/height must not panic).
///
/// The 8->5 bit reduction rounds like the game's inverse of the PNG dump's
/// 5->8 expansion: `c5 = (c8 * 31 + 127) / 255`. Alpha maps any non-zero
/// input alpha to the single RGBA5551 alpha/coverage bit.
fn write_rgba5551_framebuffer(rdram: &mut [u8], start: usize, fb: &Framebuffer) {
    let px_count = (fb.width * fb.height) as usize;
    // The framebuffer format is a fixed 2 bytes/pixel; only write pixels the
    // fb actually has AND that fit within rdram.
    let to_5 = |c: u8| -> u16 { ((c as u16 * 31 + 127) / 255) & 0x1F };
    for i in 0..px_count {
        let dst = start + i * 2;
        if dst + 2 > rdram.len() {
            break;
        }
        let src = i * 4;
        let r = fb.pixels[src];
        let g = fb.pixels[src + 1];
        let b = fb.pixels[src + 2];
        let a = fb.pixels[src + 3];
        let px: u16 =
            (to_5(r) << 11) | (to_5(g) << 6) | (to_5(b) << 1) | (if a != 0 { 1 } else { 0 });
        // Big-endian halfword, matching capture_framebuffer's from_be_bytes.
        let [hi, lo] = px.to_be_bytes();
        rdram[dst] = hi;
        rdram[dst + 1] = lo;
    }
}

/// RT64's MIT C++ render/HLE core behind one crate-local C ABI boundary.
/// The feature-gated implementation passes fn64's stable RDRAM allocation,
/// the task's ucode/display-list addresses, and a private register block to
/// `RT64::Application::Core`. RT64's render-to-RAM path writes the native
/// RGBA5551 framebuffer back into the same slice the existing fn64 VI path
/// presents.
pub struct Rt64Backend {
    #[cfg(feature = "rt64")]
    task_index: u64,
    #[cfg(feature = "rt64")]
    context: Option<ffi::Context>,
    #[cfg(not(feature = "rt64"))]
    created: bool,
}

impl Rt64Backend {
    pub fn new() -> Self {
        Rt64Backend {
            #[cfg(feature = "rt64")]
            task_index: 0,
            #[cfg(feature = "rt64")]
            context: None,
            #[cfg(not(feature = "rt64"))]
            created: false,
        }
    }
}

impl Default for Rt64Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderBackend for Rt64Backend {
    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.task_index = 0;
            self.context = None;
            let context = ffi::Context::create(cfg.width, cfg.height).map_err(|reason| {
                RenderError::Backend {
                    backend: "rt64",
                    reason,
                }
            })?;
            self.context = Some(context);
            Ok(())
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = cfg;
            self.created = false;
            Err(RenderError::Backend {
                backend: "rt64",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let task_index = self.task_index;
            self.task_index += 1;
            if let Some(spec) = std::env::var_os("FN64_GFX_TASK_DUMP") {
                let selected = spec.to_string_lossy().split(',').any(|entry| {
                    entry.trim().parse::<u64>().unwrap_or_else(|error| {
                        panic!(
                            "FN64_GFX_TASK_DUMP entry {entry:?} is not a u64 task index: {error}"
                        )
                    }) == task_index
                });
                if selected {
                    let directory = std::env::var_os("FN64_GFX_TASK_DUMP_DIR")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/fn64-gfx-task-dumps"));
                    std::fs::create_dir_all(&directory).unwrap_or_else(|error| {
                        panic!("failed to create FN64_GFX_TASK_DUMP_DIR {directory:?}: {error}")
                    });
                    let triangles = gbi::decode_display_list_f3dex2(rdram, task.data_ptr)
                        .unwrap_or_else(|error| {
                            panic!("failed to decode diagnostic gfx task {task_index}: {error}")
                        });
                    let command_trace = gbi::trace_display_list_f3dex2(rdram, task.data_ptr);
                    let report = format!(
                        "task_index={task_index}\noutput_addr={output_addr:#010x}\n\
                         reference_triangle_count={}\ntask={task:#?}\n{command_trace}",
                        triangles.len(),
                    );
                    let path = directory.join(format!("task-{task_index:04}.txt"));
                    std::fs::write(&path, report).unwrap_or_else(|error| {
                        panic!("failed to write gfx task diagnostic {path:?}: {error}")
                    });
                    eprintln!(
                        "[fn64-render-rt64] dumped gfx task #{task_index} ({} reference \
                         triangles) to {path:?}",
                        triangles.len()
                    );
                }
            }
            let context = self
                .context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?;
            context
                .process_task(rdram, task, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            Ok(FrameStatus::Complete)
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, task, output_addr);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn present(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .present()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        #[cfg(feature = "rt64")]
        if let Some(context) = &mut self.context {
            context.resize(w, h);
        }

        #[cfg(not(feature = "rt64"))]
        let _ = (w, h);
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        #[cfg(feature = "rt64")]
        {
            gbi::SUPPORTED
        }

        #[cfg(not(feature = "rt64"))]
        {
            &[]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "rt64"))]
    fn rt64_backend_without_feature_is_a_named_error_not_a_silent_success() {
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
        let mut rdram = vec![0u8; 64];
        let err = backend
            .process_task(&mut rdram, &OsTask::default(), 0)
            .unwrap_err();
        assert!(matches!(err, RenderError::NotReady(_)));
    }

    #[test]
    fn reference_backend_auto_dump_can_skip_to_a_late_task_window() {
        let backend = ReferenceBackend::new()
            .with_auto_dump("/tmp", "fn64-render-test", 3)
            .with_auto_dump_skip(4_180);
        let dump = backend.auto_dump.unwrap();
        assert_eq!(dump.task_index, 0);
        assert_eq!(dump.skip_before_task, 4_180);
        assert_eq!(dump.written, 0);
        assert_eq!(dump.limit, 3);
    }
}
