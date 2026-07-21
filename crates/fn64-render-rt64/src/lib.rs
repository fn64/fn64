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

pub mod depth;
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

use fn64_render::{
    FrameStatus, OsTask, RenderBackend, RenderConfig, RenderError, UcodeId, ViPresentation,
};
use raster::Framebuffer;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RdramHiddenSample {
    visible: u16,
    bits: u8,
}

fn read_rdram_hidden_bits(
    hidden: &mut HashMap<u32, RdramHiddenSample>,
    address: u32,
    visible: u16,
) -> u8 {
    if let Some(sample) = hidden.get(&address) {
        if sample.visible == visible {
            return sample.bits & 3;
        }
    }
    // Programming Manual 15.5.6: a non-RDP 16-bit write replicates the
    // visible LSB into both physical hidden bits. A changed visible word is
    // therefore observable evidence that another RDRAM master wrote it.
    let bits = if visible & 1 == 0 { 0 } else { 3 };
    hidden.insert(address, RdramHiddenSample { visible, bits });
    bits
}

fn write_rdram_hidden_bits(
    hidden: &mut HashMap<u32, RdramHiddenSample>,
    address: u32,
    visible: u16,
    bits: u8,
) {
    hidden.insert(
        address,
        RdramHiddenSample {
            visible,
            bits: bits & 3,
        },
    );
}
use std::collections::HashMap;

/// A headless software `RenderBackend`: decodes a small F3DEX2-family
/// display-list subset to ordered geometry/image/fill/sync operations and
/// rasterizes them into an off-screen RGBA8888 buffer with explicit RGBA16/32
/// RDRAM target write-back. "Reference" in the sense of "the thing every future real backend
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
    /// Bounded raw RDP command DMA. Triangle opcodes are variable-width
    /// edge/coefficient records, not eight-byte RSP display-list commands.
    RawRdp,
}

pub struct ReferenceBackend {
    fb: Option<Framebuffer>,
    /// Last VI scanout image. This is deliberately distinct from `fb`: VI
    /// blanking must not erase the RDP image that becomes visible again when
    /// blanking is disabled at a later V-blank.
    presented_fb: Option<Framebuffer>,
    presentation: ViPresentation,
    /// Persistent RDP color-image register. RDP state survives across OSTask
    /// boundaries; keeping the target beside the surface prevents a later
    /// task from silently falling back to the current VI buffer.
    color_image: Option<gbi::ColorImage>,
    /// Persistent RDP depth-image register, independent of color targets.
    depth_image: Option<gbi::DepthImage>,
    /// Persistent RDP primitive Z/DeltaZ registers.
    primitive_depth: Option<gbi::PrimitiveDepth>,
    /// Persistent RDP command-decode registers and physical TMEM. This is
    /// shared by admitted F3DEX2 HLE tasks and raw DPC submissions; OSTask
    /// boundaries reset RSP state, not the RDP device.
    rdp_decode_state: gbi::RdpDecodeState,
    /// The two non-CPU-visible bits owned by every physical RDRAM halfword the
    /// RDP has touched. Color images interpret them as low coverage bits;
    /// depth images interpret them as low DeltaZ bits. One address-keyed store
    /// preserves real aliasing between overlapping image ranges.
    rdram_hidden_bits: HashMap<u32, RdramHiddenSample>,
    clear_color: [u8; 4],
    decode_mode: DecodeMode,
    /// Exact F3DEX2-compatible text images allowed at task entry and after a
    /// `G_LOAD_UCODE`. Selecting the decode mode does not admit content.
    f3dex2_ucodes: gbi::F3dex2UcodeCatalog,
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
    #[cfg(not(test))]
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

fn vi_scanout(
    source: &Framebuffer,
    presentation: ViPresentation,
) -> Result<Framebuffer, RenderError> {
    let mut output = source.clone();
    if presentation.blanked {
        output.clear(0, 0, 0, 255);
        return Ok(output);
    }

    let row_bytes = usize::try_from(source.width)
        .expect("framebuffer width exceeds usize")
        .checked_mul(4)
        .expect("framebuffer row byte count overflow");
    let repeated_row = if let Some(factor) = presentation.fade {
        if source.height < 2 {
            return Err(RenderError::Backend {
                backend: "reference",
                reason: "osViFade requires at least two framebuffer rows".to_string(),
            });
        }
        let factor = u32::from(factor);
        let inverse = 0x03ff - factor;
        let mut row = vec![0u8; row_bytes];
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let top = x * 4;
            let bottom = row_bytes + top;
            for (channel, output_channel) in pixel.iter_mut().take(3).enumerate() {
                let mixed = u32::from(source.pixels[top + channel]) * inverse
                    + u32::from(source.pixels[bottom + channel]) * factor;
                *output_channel = u8::try_from((mixed + 0x01ff) / 0x03ff)
                    .expect("VI fade interpolation exceeds u8");
            }
            pixel[3] = 255;
        }
        Some(row)
    } else if presentation.repeat_line {
        Some(source.pixels[..row_bytes].to_vec())
    } else {
        None
    };

    if let Some(row) = repeated_row {
        for destination in output.pixels.chunks_exact_mut(row_bytes) {
            destination.copy_from_slice(&row);
        }
    }
    Ok(output)
}

impl ReferenceBackend {
    pub fn new() -> Self {
        ReferenceBackend {
            fb: None,
            presented_fb: None,
            presentation: ViPresentation::default(),
            color_image: None,
            depth_image: None,
            primitive_depth: None,
            rdp_decode_state: gbi::RdpDecodeState::default(),
            rdram_hidden_bits: HashMap::new(),
            clear_color: [0, 0, 0, 255],
            decode_mode: DecodeMode::Simple,
            f3dex2_ucodes: gbi::F3dex2UcodeCatalog::default(),
            auto_dump: None,
            #[cfg(not(test))]
            diag_task_index: 0,
        }
    }

    /// Select real F3DEX2 command decoding (matrix stack, segment table,
    /// viewport) instead of the simple reference-fixture encoding. This does
    /// not admit any microcode image: callers must also register every exact
    /// compatible text digest, or the task is replayed through LLE.
    pub fn with_f3dex2(mut self) -> Self {
        self.decode_mode = DecodeMode::F3dex2;
        self
    }

    /// Admit one exact task-entry or self-load target as F3DEX2-compatible.
    /// The digest is SHA-256 over the complete logical 4 KiB text image. This
    /// API carries identity rather than game bytes, so a host can configure
    /// known public variants without placing ROM or ucode content in fn64.
    pub fn with_f3dex2_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.f3dex2_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact logical 4 KiB F3DEX2 text image, retaining only its
    /// SHA-256 identity. Primarily useful to deterministic fixtures that
    /// construct synthetic IMEM rather than carrying a precomputed digest.
    pub fn with_f3dex2_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "F3DEX2 text admission requires one complete 4 KiB IMEM image"
        );
        self.f3dex2_ucodes.admit_text(text);
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

    /// The image produced by the most recent VI presentation boundary.
    /// Unlike [`Self::framebuffer`], this includes VI-level blanking.
    pub fn presented_framebuffer(&self) -> Option<&Framebuffer> {
        self.presented_fb.as_ref()
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
        self.presented_fb = Some(fb.clone());
        self.presentation = ViPresentation::default();
        self.fb = Some(fb);
        self.color_image = None;
        self.depth_image = None;
        self.primitive_depth = None;
        self.rdp_decode_state = gbi::RdpDecodeState::default();
        self.rdram_hidden_bits.clear();
        Ok(())
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        let persistent_target = self.color_image;
        let persistent_depth_image = self.depth_image;
        let mut active_primitive_depth = self.primitive_depth;
        let rdram_hidden_bits = &mut self.rdram_hidden_bits;
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

        let operations = match self.decode_mode {
            DecodeMode::Simple => gbi::decode_display_list(&*rdram, task.data_ptr)?
                .into_iter()
                .map(gbi::RenderOp::Triangle)
                .collect::<Vec<_>>(),
            DecodeMode::F3dex2 => {
                if let Err(RenderError::RequiresLle { ucode_sha256 }) = self
                    .f3dex2_ucodes
                    .require_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                {
                    return Ok(FrameStatus::NeedsLle { ucode_sha256 });
                }
                // HLE decode is a transaction. A family-changing self-load
                // cannot be continued instruction-exactly from HLE because
                // scalar/VU registers are intentionally not represented at
                // this seam. Execute against clones first; an unadmitted
                // generation leaves live task-entry state untouched so the
                // runtime can replay the whole ucode phase through LLE.
                let mut speculative_rdram = rdram.to_vec();
                let mut speculative_rsp = rsp_memory.clone();
                let mut speculative_rdp = self.rdp_decode_state.clone();
                let operations = match gbi::execute_display_list_f3dex2_ops_admitted_with_rdp_state(
                    &mut speculative_rdram,
                    &mut speculative_rsp,
                    task.data_ptr,
                    &self.f3dex2_ucodes,
                    &mut speculative_rdp,
                ) {
                    Ok(operations) => operations,
                    Err(RenderError::RequiresLle { ucode_sha256 }) => {
                        return Ok(FrameStatus::NeedsLle { ucode_sha256 });
                    }
                    Err(error) => return Err(error),
                };
                rdram.copy_from_slice(&speculative_rdram);
                *rsp_memory = speculative_rsp;
                self.rdp_decode_state = speculative_rdp;
                operations
            }
            DecodeMode::RawRdp => gbi::decode_raw_rdp_ops_with_state(
                &*rdram,
                task.data_ptr,
                &mut self.rdp_decode_state,
            )?,
        };
        let tri_count = operations
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    gbi::RenderOp::Triangle(_)
                        | gbi::RenderOp::Line(_)
                        | gbi::RenderOp::RawTriangle(_)
                )
            })
            .count();
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
                        panic!(
                            "FN64_GFX_TASK_DUMP entry {entry:?} is not a u64 task index: {error}"
                        )
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
        // `output_addr` is a compatibility target only for the fixture-only
        // simple lane. F3DEX2 and raw RDP execution consume the RDP's
        // persistent G_SETCIMG register; borrowing VI state would hide a
        // missing color-image command and can write the wrong DRAM surface.
        let mut active_target = persistent_target;
        if self.decode_mode == DecodeMode::Simple && active_target.is_none() && output_addr != 0 {
            active_target = Some(gbi::ColorImage {
                format: gbi::ColorImage::RGBA_FORMAT,
                size: gbi::ColorImage::BITS_16,
                width: u16::try_from(fb.width).expect("reference framebuffer width exceeds u16"),
                address: output_addr,
            });
        }
        let mut target_loaded = persistent_target.is_some();
        let mut active_depth_image = persistent_depth_image;
        let mut saw_explicit_target = false;
        let mut dirty = false;
        let mut depth_dirty = false;

        // The register persists, but RDRAM remains the color image's storage.
        // Re-import it at every production task boundary so intervening CPU,
        // PI, or other device writes cannot be overwritten from a stale host
        // RGBA cache when this task later commits an untouched pixel.
        if self.decode_mode != DecodeMode::Simple {
            if let Some(target) = active_target {
                validate_reference_color_image(rdram, fb.height, target)?;
                load_color_image(rdram, target, fb, rdram_hidden_bits);
            }
        }

        // CPU-visible Z bits can change between tasks while the RDP register
        // remains selected. Reload those bits at the task boundary; the
        // address-keyed hidden store supplies the two bits ordinary CPU
        // halfword accesses cannot observe.
        if let Some(target) = active_depth_image {
            load_rdp_depth_image(rdram, target, fb, rdram_hidden_bits)?;
        }

        // TEMP (env `FN64_NO_DEPTH=1`): force painter's-order (no z-test) to
        // A/B-prove the z-buffer is what produces correct occlusion.
        #[cfg(not(test))]
        let no_depth = crate::debug_flag("FN64_NO_DEPTH");
        #[cfg(test)]
        let no_depth = false;

        for operation in &operations {
            match operation {
                gbi::RenderOp::Triangle(triangle) => {
                    require_reference_color_target(
                        self.decode_mode,
                        active_target,
                        "F3DEX2 triangle",
                    )?;
                    if !no_depth
                        && (triangle.other_mode.depth_compare_enabled()
                            || triangle.other_mode.depth_update_enabled())
                        && active_depth_image.is_none()
                    {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason: "F3DEX2 triangle enables Z compare/update without a selected G_SETZIMG target"
                            .to_string(),
                        });
                    }
                    if !no_depth
                        && (triangle.other_mode.depth_compare_enabled()
                            || triangle.other_mode.depth_update_enabled())
                        && triangle.other_mode.primitive_depth_source()
                        && active_primitive_depth.is_none()
                    {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason:
                                "F3DEX2 triangle selects primitive Z without prior G_SETPRIMDEPTH"
                                    .to_string(),
                        });
                    }
                    fb.set_primitive_depth(active_primitive_depth);
                    if self.decode_mode == DecodeMode::Simple {
                        fb.draw_triangle(triangle);
                    } else if no_depth {
                        fb.draw_triangle_no_depth_culled(triangle, triangle.cull);
                    } else {
                        fb.draw_triangle_culled(triangle, triangle.cull);
                    }
                    depth_dirty |= !no_depth && triangle.other_mode.depth_update_enabled();
                    dirty = true;
                }
                gbi::RenderOp::Line(line) => {
                    require_reference_color_target(self.decode_mode, active_target, "G_LINE3D")?;
                    if !no_depth
                        && line.other_mode.depth_compare_enabled()
                        && active_depth_image.is_none()
                    {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason:
                                "G_LINE3D enables Z compare without a selected G_SETZIMG target"
                                    .to_string(),
                        });
                    }
                    if !no_depth
                        && line.other_mode.depth_compare_enabled()
                        && line.other_mode.primitive_depth_source()
                        && active_primitive_depth.is_none()
                    {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason: "G_LINE3D selects primitive Z without prior G_SETPRIMDEPTH"
                                .to_string(),
                        });
                    }
                    fb.set_primitive_depth(active_primitive_depth);
                    if no_depth {
                        fb.draw_line_no_depth(line);
                    } else {
                        fb.draw_line(line);
                    }
                    // Public line modes read Z but never write it.
                    dirty = true;
                }
                gbi::RenderOp::RawTriangle(triangle) => {
                    require_reference_color_target(
                        self.decode_mode,
                        active_target,
                        "raw RDP triangle",
                    )?;
                    if !no_depth
                        && (triangle.other_mode.depth_compare_enabled()
                            || triangle.other_mode.depth_update_enabled())
                        && active_depth_image.is_none()
                    {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason: "raw RDP triangle enables Z compare/update without a selected G_SETZIMG target"
                                .to_string(),
                        });
                    }
                    if !no_depth
                        && (triangle.other_mode.depth_compare_enabled()
                            || triangle.other_mode.depth_update_enabled())
                        && ((triangle.other_mode.primitive_depth_source()
                            && active_primitive_depth.is_none())
                            || (!triangle.other_mode.primitive_depth_source()
                                && triangle.z.is_none()))
                    {
                        let reason = if triangle.other_mode.primitive_depth_source() {
                            "raw RDP triangle selects primitive Z without prior G_SETPRIMDEPTH"
                        } else {
                            "raw RDP triangle enables pixel Z compare/update without carrying Z coefficients"
                        };
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason: reason.to_string(),
                        });
                    }
                    fb.set_primitive_depth(active_primitive_depth);
                    if no_depth {
                        fb.draw_raw_rdp_triangle_no_depth(triangle);
                    } else {
                        fb.draw_raw_rdp_triangle(triangle);
                    }
                    depth_dirty |= !no_depth && triangle.other_mode.depth_update_enabled();
                    dirty = true;
                }
                gbi::RenderOp::SetColorImage(target) => {
                    validate_reference_color_image(rdram, fb.height, *target)?;
                    let changes_target = active_target != Some(*target) || !target_loaded;
                    if changes_target {
                        if depth_dirty {
                            if let Some(depth_target) = active_depth_image {
                                commit_rdp_depth_image(rdram, depth_target, fb, rdram_hidden_bits)?;
                            }
                            depth_dirty = false;
                        }
                        if dirty {
                            if let Some(previous) = active_target {
                                commit_color_image(rdram, previous, fb, rdram_hidden_bits);
                            }
                        }
                        load_color_image(rdram, *target, fb, rdram_hidden_bits);
                        if let Some(depth_target) = active_depth_image {
                            load_rdp_depth_image(rdram, depth_target, fb, rdram_hidden_bits)?;
                        }
                        dirty = false;
                    }
                    active_target = Some(*target);
                    target_loaded = true;
                    saw_explicit_target = true;
                }
                gbi::RenderOp::SetDepthImage(target) => {
                    if active_depth_image != Some(*target) {
                        if depth_dirty {
                            if let Some(previous) = active_depth_image {
                                commit_rdp_depth_image(rdram, previous, fb, rdram_hidden_bits)?;
                            }
                            depth_dirty = false;
                        }
                        load_rdp_depth_image(rdram, *target, fb, rdram_hidden_bits)?;
                        active_depth_image = Some(*target);
                    }
                }
                gbi::RenderOp::SetPrimitiveDepth(primitive_depth) => {
                    active_primitive_depth = Some(*primitive_depth);
                    fb.set_primitive_depth(active_primitive_depth);
                }
                gbi::RenderOp::FillRectangle(rectangle) => {
                    require_reference_color_target(self.decode_mode, active_target, "G_FILLRECT")?;
                    if rectangle.cycle_type != gbi::CycleType::Fill {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason: format!(
                                "G_FILLRECT in {:?} cycle is not implemented; refusing to apply fill-cycle semantics",
                                rectangle.cycle_type
                            ),
                        });
                    }
                    let target = active_target.unwrap_or(gbi::ColorImage {
                        format: gbi::ColorImage::RGBA_FORMAT,
                        size: gbi::ColorImage::BITS_16,
                        width: u16::try_from(fb.width)
                            .expect("reference framebuffer width exceeds u16"),
                        address: 0,
                    });
                    fb.draw_fill_rectangle(rectangle, target);
                    if active_target.map(|target| target.address)
                        == active_depth_image.map(|target| target.address)
                    {
                        if rectangle.fill_color & 0x0003_0003 != 0 {
                            return Err(RenderError::Backend {
                                backend: "reference",
                                reason: format!(
                                    "depth-image G_FILLRECT value {:#010x} has nonzero encoded DeltaZ; hidden-bit fill semantics are not implemented",
                                    rectangle.fill_color
                                ),
                            });
                        }
                        fb.clear_depth_rectangle(rectangle);
                        depth_dirty = true;
                    }
                    dirty = true;
                }
                gbi::RenderOp::TextureRectangle(rectangle) => {
                    require_reference_color_target(
                        self.decode_mode,
                        active_target,
                        texture_rectangle_name(rectangle),
                    )?;
                    validate_texture_rectangle(rectangle, active_target)?;
                    if (rectangle.other_mode.depth_compare_enabled()
                        || rectangle.other_mode.depth_update_enabled())
                        && active_primitive_depth.is_none()
                    {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason: format!(
                                "{} selects primitive Z without prior G_SETPRIMDEPTH",
                                texture_rectangle_name(rectangle)
                            ),
                        });
                    }
                    if (rectangle.other_mode.depth_compare_enabled()
                        || rectangle.other_mode.depth_update_enabled())
                        && active_depth_image.is_none()
                    {
                        return Err(RenderError::Backend {
                            backend: "reference",
                            reason: format!(
                                "{} enables Z compare/update without a selected G_SETZIMG target",
                                texture_rectangle_name(rectangle)
                            ),
                        });
                    }
                    fb.set_primitive_depth(active_primitive_depth);
                    match rectangle.other_mode.cycle_type() {
                        gbi::CycleType::Copy => fb.draw_copy_texture_rectangle(rectangle),
                        gbi::CycleType::OneCycle | gbi::CycleType::TwoCycle => {
                            fb.draw_texture_rectangle(rectangle)
                        }
                        gbi::CycleType::Fill => {
                            unreachable!("fill-cycle texture rectangle passed reference validation")
                        }
                    }
                    depth_dirty |= rectangle.other_mode.depth_update_enabled();
                    dirty = true;
                }
                gbi::RenderOp::FullSync => {
                    if dirty {
                        if let Some(target) = active_target {
                            commit_color_image(rdram, target, fb, rdram_hidden_bits);
                        }
                        dirty = false;
                    }
                    if depth_dirty {
                        if let Some(target) = active_depth_image {
                            commit_rdp_depth_image(rdram, target, fb, rdram_hidden_bits)?;
                        }
                        depth_dirty = false;
                    }
                }
            }
        }
        #[cfg(not(test))]
        if self.decode_mode == DecodeMode::F3dex2 {
            raster::zstat::summary();
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
        // different address. Production F3DEX2/raw work therefore uses only
        // the persistent G_SETCIMG register. `output_addr` remains a simple-
        // fixture compatibility target; it cannot satisfy an RDP color write.
        //
        // Format: the active public 8-bit, RGBA16, or RGBA32 color-image encoding,
        // written through fn64-runtime's canonical RDRAM view. The view owns
        // the ABI's native-word storage mapping; this renderer never applies
        // byte-order transformations by hand.
        if let Some(target) = active_target {
            commit_color_image(rdram, target, fb, rdram_hidden_bits);
        }
        if depth_dirty {
            if let Some(target) = active_depth_image {
                commit_rdp_depth_image(rdram, target, fb, rdram_hidden_bits)?;
            }
        }
        if saw_explicit_target || persistent_target.is_some() {
            self.color_image = active_target;
        }
        self.depth_image = active_depth_image;
        self.primitive_depth = active_primitive_depth;

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

    fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        _output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        gbi::validate_raw_rdp_command_range(rdram, start, end)?;
        let terminated_len = (end as usize)
            .checked_add(8)
            .ok_or_else(|| RenderError::Backend {
                backend: "reference",
                reason: "raw RDP terminator address overflow".to_string(),
            })?;
        let mut image = rdram.to_vec();
        image.resize(terminated_len.max(image.len()), 0);
        image[end as usize..end as usize + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        image[end as usize + 4..end as usize + 8].copy_from_slice(&0u32.to_ne_bytes());

        let previous_mode = self.decode_mode;
        self.decode_mode = DecodeMode::RawRdp;
        let result = self.process_task(
            &mut image,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: start,
                ..OsTask::default()
            },
            0,
        );
        self.decode_mode = previous_mode;
        if result.is_ok() {
            rdram.copy_from_slice(&image[..rdram.len()]);
        }
        result
    }

    fn present(&mut self, vi: ViPresentation) -> Result<(), RenderError> {
        let source = self
            .fb
            .as_ref()
            .ok_or(RenderError::NotReady("create() not called"))?;
        self.presented_fb = Some(vi_scanout(source, vi)?);
        self.presentation = vi;
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
        if let Some(fb) = &self.fb {
            // `resize` is infallible by trait contract. If the new dimensions
            // cannot support the retained VI effect, leave no fabricated
            // scanout; the next `present` reports the named error.
            self.presented_fb = vi_scanout(fb, self.presentation).ok();
        }
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        gbi::SUPPORTED
    }
}

fn validate_reference_color_image(
    rdram: &[u8],
    height: u32,
    target: gbi::ColorImage,
) -> Result<(), RenderError> {
    let Some(layout) = target.layout() else {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETCIMG format={} size={} is unsupported; reference execution requires 8-bit intensity, RGBA16, or RGBA32",
                target.format, target.size
            ),
        });
    };
    let bytes_per_pixel = layout.bytes_per_pixel();
    if target.width == 0 {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG decoded a zero-width color image".to_string(),
        });
    }
    if !target.address.is_multiple_of(8) {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETCIMG {} base {:#010x} is not 64-bit aligned",
                layout.name(),
                target.address,
            ),
        });
    }
    let byte_len = usize::from(target.width)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG dimensions overflow host address space".to_string(),
        })?;
    let end = (target.address as usize)
        .checked_add(byte_len)
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG address range overflows host address space".to_string(),
        })?;
    if end > rdram.len() {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETCIMG {} target [{:#010x}, {end:#010x}) exceeds RDRAM length {}",
                layout.name(),
                target.address,
                rdram.len()
            ),
        });
    }
    Ok(())
}

fn require_reference_color_target(
    decode_mode: DecodeMode,
    target: Option<gbi::ColorImage>,
    operation: &str,
) -> Result<(), RenderError> {
    if decode_mode != DecodeMode::Simple && target.is_none() {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "{operation} has no persistent G_SETCIMG color target; VI/output_addr state is not an RDP color-image substitute"
            ),
        });
    }
    Ok(())
}

fn validate_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
    target: Option<gbi::ColorImage>,
) -> Result<(), RenderError> {
    match rectangle.other_mode.cycle_type() {
        gbi::CycleType::Copy => validate_copy_texture_rectangle(rectangle, target),
        gbi::CycleType::OneCycle | gbi::CycleType::TwoCycle => {
            validate_combined_texture_rectangle(rectangle)
        }
        gbi::CycleType::Fill => Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "{} in Fill cycle is invalid; fill cycle bypasses texture sampling",
                texture_rectangle_name(rectangle)
            ),
        }),
    }
}

fn texture_rectangle_name(rectangle: &gbi::TextureRectangle) -> &'static str {
    if rectangle.flip {
        "G_TEXRECTFLIP"
    } else {
        "G_TEXRECT"
    }
}

fn validate_copy_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
    target: Option<gbi::ColorImage>,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    debug_assert_eq!(rectangle.other_mode.cycle_type(), gbi::CycleType::Copy);
    if rectangle.other_mode.depth_compare_enabled() || rectangle.other_mode.depth_update_enabled() {
        return Err(reject(format!(
            "{} enables depth in Copy cycle, which bypasses the blender",
            texture_rectangle_name(rectangle)
        )));
    }
    if rectangle.dsdx != 4 << 10 {
        return Err(reject(format!(
            "{} copy dsdx={} violates the public copy-mode 4<<10 step",
            texture_rectangle_name(rectangle),
            rectangle.dsdx
        )));
    }
    if rectangle.other_mode.alpha_compare() == gbi::AlphaCompare::Reserved {
        return Err(reject(format!(
            "{} uses reserved alpha-compare mode 2",
            texture_rectangle_name(rectangle)
        )));
    }
    if rectangle.other_mode.alpha_compare() == gbi::AlphaCompare::Dither {
        return Err(reject(format!(
            "{} uses G_AC_DITHER, whose hardware pseudo-random alpha threshold is not implemented",
            texture_rectangle_name(rectangle)
        )));
    }
    let texture = rectangle.texture.as_ref().ok_or_else(|| {
        reject(format!(
            "{} references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
            texture_rectangle_name(rectangle),
            rectangle.tile
        ))
    })?;
    let rgba16 =
        texture.format == gbi::ColorImage::RGBA_FORMAT && texture.size == gbi::ColorImage::BITS_16;
    let direct_ci8 = texture.format == gbi::ColorImage::CI_FORMAT
        && texture.size == gbi::ColorImage::BITS_8
        && rectangle.other_mode.texture_lut() == 0;
    if !rgba16 && !direct_ci8 {
        return Err(reject(format!(
            "{} copy source format={} size={} LUT={} is unsupported; expected RGBA16 or non-dereferenced CI8",
            texture_rectangle_name(rectangle), texture.format, texture.size
            , rectangle.other_mode.texture_lut()
        )));
    }
    if let Some(target) = target {
        let matching_target = matches!(
            (rgba16, direct_ci8, target.layout()),
            (true, false, Some(gbi::ColorImageLayout::Rgba16))
                | (false, true, Some(gbi::ColorImageLayout::Index8))
        );
        if !matching_target {
            return Err(reject(format!(
                "{} copy source format={} size={} does not match color target format={} size={}",
                texture_rectangle_name(rectangle),
                texture.format,
                texture.size,
                target.format,
                target.size
            )));
        }
    }
    if let Some(scissor) = rectangle.scissor {
        let multiple_of_four = |edge: f32| edge.fract() == 0.0 && (edge as i32).rem_euclid(4) == 0;
        if ![scissor.ulx, scissor.uly, scissor.lrx, scissor.lry]
            .into_iter()
            .all(multiple_of_four)
        {
            return Err(reject(format!(
                "{} copy scissor ({}, {})..({}, {}) is not aligned to the documented four-pixel boundary",
                texture_rectangle_name(rectangle), scissor.ulx, scissor.uly, scissor.lrx, scissor.lry
            )));
        }
    }
    Ok(())
}

fn validate_combined_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    let name = texture_rectangle_name(rectangle);
    let mode = rectangle.other_mode;
    if mode.alpha_compare() == gbi::AlphaCompare::Reserved {
        return Err(reject(format!("{name} uses reserved alpha-compare mode 2")));
    }
    if mode.alpha_compare() == gbi::AlphaCompare::Dither {
        return Err(reject(format!(
            "{name} uses G_AC_DITHER, whose hardware pseudo-random alpha threshold is not implemented"
        )));
    }
    if mode.texture_filter() == gbi::TextureFilter::Reserved {
        return Err(reject(format!(
            "{name} uses reserved texture-filter mode 1"
        )));
    }
    if (mode.depth_compare_enabled() || mode.depth_update_enabled())
        && !mode.primitive_depth_source()
    {
        return Err(reject(format!(
            "{name} requests depth compare/update with pixel Z, but rectangles require G_ZS_PRIM"
        )));
    }
    if !matches!(mode.texture_convert(), 0 | 5 | 6) {
        return Err(reject(format!(
            "{name} uses reserved texture-convert mode {}",
            mode.texture_convert()
        )));
    }
    if mode.texture_detail() == 3 {
        return Err(reject(format!(
            "{name} selects reserved texture-detail mode 3"
        )));
    }
    rectangle.texture.as_ref().ok_or_else(|| {
        reject(format!(
            "{name} references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
            rectangle.tile
        ))
    })?;

    let cycle_count = match mode.cycle_type() {
        gbi::CycleType::OneCycle => 1,
        gbi::CycleType::TwoCycle => 2,
        _ => unreachable!("combined rectangle validator called for bypass cycle"),
    };
    for (cycle_index, cycle) in rectangle
        .combiner
        .mode
        .cycles
        .iter()
        .take(cycle_count)
        .enumerate()
    {
        for source in cycle.rgb {
            validate_rectangle_color_source(rectangle, cycle_index, source)?;
        }
        for source in cycle.alpha {
            validate_rectangle_alpha_source(rectangle, cycle_index, source)?;
        }
    }
    if rectangle
        .blender
        .cycles
        .iter()
        .take(usize::from(rectangle.blender.cycle_count))
        .any(|cycle| cycle.a == gbi::BlendAlphaInput::Shade)
    {
        return Err(reject(format!(
            "{name} blender selects SHADE alpha, but rectangle commands carry no shade attributes"
        )));
    }
    Ok(())
}

fn validate_rectangle_color_source(
    rectangle: &gbi::TextureRectangle,
    cycle_index: usize,
    source: gbi::ColorSource,
) -> Result<(), RenderError> {
    use gbi::ColorSource;
    let name = texture_rectangle_name(rectangle);
    let unsupported = |reason: &str| RenderError::Backend {
        backend: "reference",
        reason: format!("{name} combiner cycle {} {reason}", cycle_index + 1),
    };
    match source {
        ColorSource::Combined | ColorSource::CombinedAlpha if cycle_index == 0 => Err(unsupported(
            "selects COMBINED before a first-cycle result exists",
        )),
        ColorSource::Texel1 | ColorSource::Texel1Alpha
            if rectangle.texture1.is_none() && !rectangle.other_mode.texture_lod() =>
        {
            Err(unsupported("selects TEXEL1 without a decoded tile+1 image"))
        }
        ColorSource::Shade | ColorSource::ShadeAlpha => Err(unsupported(
            "selects SHADE, but rectangle commands carry no shade attributes",
        )),
        ColorSource::Noise => Err(unsupported("selects the unmodeled noise register")),
        _ => Ok(()),
    }
}

fn validate_rectangle_alpha_source(
    rectangle: &gbi::TextureRectangle,
    cycle_index: usize,
    source: gbi::AlphaSource,
) -> Result<(), RenderError> {
    use gbi::AlphaSource;
    let name = texture_rectangle_name(rectangle);
    let unsupported = |reason: &str| RenderError::Backend {
        backend: "reference",
        reason: format!("{name} alpha combiner cycle {} {reason}", cycle_index + 1),
    };
    match source {
        AlphaSource::Combined if cycle_index == 0 => Err(unsupported(
            "selects COMBINED before a first-cycle result exists",
        )),
        AlphaSource::Texel1
            if rectangle.texture1.is_none() && !rectangle.other_mode.texture_lod() =>
        {
            Err(unsupported("selects TEXEL1 without a decoded tile+1 image"))
        }
        AlphaSource::Shade => Err(unsupported(
            "selects SHADE, but rectangle commands carry no shade attributes",
        )),
        _ => Ok(()),
    }
}

/// Load an RGBA16 color image into the software surface before ordered work
/// continues on that target. Depth is deliberately not reset: the RDP depth
/// image is independent of color-image switches and persists across tasks.
fn load_rgba5551_framebuffer(
    rdram: &[u8],
    target: gbi::ColorImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    if fb.width != u32::from(target.width) {
        *fb = Framebuffer::new(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let offset = u32::try_from(index * 2).expect("color-image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("color-image logical address overflow");
        let pixel = view.read_u16(address);
        let hidden = read_rdram_hidden_bits(hidden_bits, address.offset(), pixel);
        let stored_coverage = (((pixel & 1) as u8) << 2) | hidden;
        let expand = |value: u16| -> u8 {
            let value = value as u8;
            (value << 3) | (value >> 2)
        };
        let dst = index * 4;
        fb.pixels[dst..dst + 4].copy_from_slice(&[
            expand((pixel >> 11) & 0x1f),
            expand((pixel >> 6) & 0x1f),
            expand((pixel >> 1) & 0x1f),
            255,
        ]);
        fb.coverage[index] = raster::Coverage::from_stored(stored_coverage);
    }
}

/// Import the active public RDP color-image format into the software working
/// surface. Public Programming Manual section 15.5, "Color Image Format,"
/// defines RGBA32 memory alpha as five alpha bits plus the three coverage bits
/// in the byte's most-significant bits.
fn load_color_image(
    rdram: &[u8],
    target: gbi::ColorImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    match target
        .layout()
        .expect("validated color image changed format")
    {
        gbi::ColorImageLayout::Index8 => load_intensity8_framebuffer(rdram, target, fb),
        gbi::ColorImageLayout::Rgba16 => load_rgba5551_framebuffer(rdram, target, fb, hidden_bits),
        gbi::ColorImageLayout::Rgba32 => load_rgba8888_framebuffer(rdram, target, fb),
    }
}

/// Import the public 8-bit color-image layout. Programming Manual Figure
/// 15.5.4 labels each byte as one intensity component and states that hidden
/// coverage bits are ignored for this format.
fn load_intensity8_framebuffer(rdram: &[u8], target: gbi::ColorImage, fb: &mut Framebuffer) {
    if fb.width != u32::from(target.width) {
        *fb = Framebuffer::new(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let address = start
            .checked_add(u32::try_from(index).expect("I8 color-image offset exceeds u32"))
            .expect("I8 color-image logical address overflow");
        let intensity = view.read_u8(address);
        let destination = index * 4;
        fb.pixels[destination..destination + 4]
            .copy_from_slice(&[intensity, intensity, intensity, 255]);
        fb.coverage[index] = raster::Coverage::FULL;
    }
}

fn load_rgba8888_framebuffer(rdram: &[u8], target: gbi::ColorImage, fb: &mut Framebuffer) {
    if fb.width != u32::from(target.width) {
        *fb = Framebuffer::new(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let offset = u32::try_from(index * 4).expect("color-image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("color-image logical address overflow");
        let [red, green, blue, alpha_coverage] = view.read_u32(address).to_be_bytes();
        let alpha5 = alpha_coverage & 0x1f;
        let alpha = (alpha5 << 3) | (alpha5 >> 2);
        let dst = index * 4;
        fb.pixels[dst..dst + 4].copy_from_slice(&[red, green, blue, alpha]);
        fb.coverage[index] = raster::Coverage::from_stored(alpha_coverage >> 5);
    }
}

/// Convert `fb`'s RGBA8888 pixels to N64 RGBA5551 and write them into
/// `rdram` starting at logical byte offset `start`, row-major with a top-left
/// origin. [`fn64_runtime::RdramViewMut`] is the sole translation from those
/// logical halfwords to N64Recomp's native-word ABI storage. A pixel whose 2
/// bytes would run past `rdram` is skipped
/// (bounds-safe; the caller already validated `output_addr` is a real
/// framebuffer offset, but a wrong width/height must not panic).
///
/// Programming Manual Chapter 15.5 specifies that the memory interface adds
/// three low dither bits and then reduces RGB from eight to five bits. Active
/// dither modes are rejected by the rasterizer until their hardware tables or
/// noise sequence are proven, so the supported disabled path is exact `>> 3`
/// truncation. RGBA16's visible LSB is the high bit of stored coverage, not
/// retained pixel alpha; the lower two bits are committed to the physical
/// hidden-bit sidecar.
fn write_rgba5551_framebuffer(
    rdram: &mut [u8],
    start: usize,
    fb: &Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    let px_count = (fb.width * fb.height) as usize;
    // The framebuffer format is a fixed 2 bytes/pixel; only write pixels the
    // fb actually has AND that fit within rdram.
    let to_5 = |c: u8| -> u16 { u16::from(c >> 3) };
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("framebuffer RDRAM offset exceeds u32"),
    );
    assert!(
        start.offset().is_multiple_of(4),
        "RGBA5551 framebuffer base must be word-aligned, got {:#x}",
        start.offset()
    );
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for i in 0..px_count {
        let byte_offset = u32::try_from(i.checked_mul(2).expect("framebuffer size overflow"))
            .expect("framebuffer byte offset exceeds u32");
        let Some(dst) = start.checked_add(byte_offset) else {
            break;
        };
        if dst.offset() as usize + 2 > view.len() {
            break;
        }
        let src = i * 4;
        let r = fb.pixels[src];
        let g = fb.pixels[src + 1];
        let b = fb.pixels[src + 2];
        let stored_coverage = fb.coverage[i].stored();
        let px: u16 = (to_5(r) << 11)
            | (to_5(g) << 6)
            | (to_5(b) << 1)
            | u16::from((stored_coverage >> 2) & 1);
        view.write_u16(dst, px);
        write_rdram_hidden_bits(hidden_bits, dst.offset(), px, stored_coverage & 3);
    }
}

fn commit_color_image(
    rdram: &mut [u8],
    target: gbi::ColorImage,
    fb: &Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    match target
        .layout()
        .expect("validated color image changed format")
    {
        gbi::ColorImageLayout::Index8 => {
            write_intensity8_framebuffer(rdram, target.address as usize, fb)
        }
        gbi::ColorImageLayout::Rgba16 => {
            write_rgba5551_framebuffer(rdram, target.address as usize, fb, hidden_bits)
        }
        gbi::ColorImageLayout::Rgba32 => {
            write_rgba8888_framebuffer(rdram, target.address as usize, fb)
        }
    }
}

/// Commit the color pipeline's intensity component to the public one-byte
/// color-image layout. The RDP exposes no palette for this target; callers
/// program equal RGB components when the intermediate image is meaningful,
/// so the common red/intensity lane is the byte written by the memory model.
fn write_intensity8_framebuffer(rdram: &mut [u8], start: usize, fb: &Framebuffer) {
    let pixel_count = (fb.width * fb.height) as usize;
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("I8 framebuffer RDRAM offset exceeds u32"),
    );
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for index in 0..pixel_count {
        let Some(destination) = start
            .checked_add(u32::try_from(index).expect("I8 framebuffer byte offset exceeds u32"))
        else {
            break;
        };
        if destination.offset() as usize >= view.len() {
            break;
        }
        view.write_u8(destination, fb.pixels[index * 4]);
    }
}

/// Commit RGBA32 as RGB8 plus the five-bit memory alpha and three-bit coverage
/// packing defined by public Programming Manual section 15.5. Unlike RGBA16,
/// this format does not use RDRAM hidden bits.
fn write_rgba8888_framebuffer(rdram: &mut [u8], start: usize, fb: &Framebuffer) {
    let pixel_count = (fb.width * fb.height) as usize;
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("framebuffer RDRAM offset exceeds u32"),
    );
    assert!(
        start.offset().is_multiple_of(8),
        "RGBA8888 framebuffer base must be 64-bit aligned, got {:#x}",
        start.offset()
    );
    // Chapter 15.5 stores only five bits of alpha beside three bits of
    // coverage. As with disabled RGB dither, the supported no-alpha-dither
    // path truncates rather than rounding to the nearest expanded value.
    let to_5 = |channel: u8| -> u8 { channel >> 3 };
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for index in 0..pixel_count {
        let byte_offset = u32::try_from(index.checked_mul(4).expect("framebuffer size overflow"))
            .expect("framebuffer byte offset exceeds u32");
        let Some(destination) = start.checked_add(byte_offset) else {
            break;
        };
        if destination.offset() as usize + 4 > view.len() {
            break;
        }
        let source = index * 4;
        let alpha_coverage = (fb.coverage[index].stored() << 5) | to_5(fb.pixels[source + 3]);
        view.write_u32(
            destination,
            u32::from_be_bytes([
                fb.pixels[source],
                fb.pixels[source + 1],
                fb.pixels[source + 2],
                alpha_coverage,
            ]),
        );
    }
}

fn validate_rdp_depth_image(
    rdram: &[u8],
    target: gbi::DepthImage,
    fb: &Framebuffer,
) -> Result<(), RenderError> {
    if !target.address.is_multiple_of(2) {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETZIMG base {:#010x} is not halfword-aligned",
                target.address
            ),
        });
    }
    let byte_len = (fb.width as usize)
        .checked_mul(fb.height as usize)
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETZIMG dimensions overflow host address space".to_string(),
        })?;
    let end = (target.address as usize)
        .checked_add(byte_len)
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETZIMG address range overflows host address space".to_string(),
        })?;
    if end > rdram.len() {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETZIMG target [{:#010x}, {end:#010x}) exceeds RDRAM length {}",
                target.address,
                rdram.len()
            ),
        });
    }
    Ok(())
}

/// Load CPU-visible compressed Z and the separately owned hidden DeltaZ bits
/// into the software compare buffer. Nintendo 64 Programming Manual Chapter
/// 16, "Z Image Format" defines this 14+4 split; ordinary RDRAM reads expose
/// only the 16-bit word, so the hidden pair is maintained by physical address.
fn load_rdp_depth_image(
    rdram: &[u8],
    target: gbi::DepthImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) -> Result<(), RenderError> {
    validate_rdp_depth_image(rdram, target, fb)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..fb.depth.len() {
        let offset = u32::try_from(index.checked_mul(2).expect("depth image size overflow"))
            .expect("depth image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("validated depth-image logical address overflow");
        let visible = view.read_u16(address);
        let encoded = depth::EncodedDepth {
            visible,
            hidden: read_rdram_hidden_bits(hidden_bits, address.offset(), visible),
        };
        fb.depth[index] = depth::unpack(encoded).0 as f32;
        fb.encoded_depth[index] = Some(encoded);
    }
    Ok(())
}

/// Commit passing Z_UPD/fill samples to both halves of RDP depth memory.
/// Samples without an encoding are left loud at their producer rather than
/// fabricated here; every current persistent producer supplies one.
fn commit_rdp_depth_image(
    rdram: &mut [u8],
    target: gbi::DepthImage,
    fb: &Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) -> Result<(), RenderError> {
    validate_rdp_depth_image(rdram, target, fb)?;
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for (index, encoded) in fb.encoded_depth.iter().copied().enumerate() {
        let Some(encoded) = encoded else {
            continue;
        };
        let offset = u32::try_from(index.checked_mul(2).expect("depth image size overflow"))
            .expect("depth image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("validated depth-image logical address overflow");
        view.write_u16(address, encoded.visible);
        write_rdram_hidden_bits(
            hidden_bits,
            address.offset(),
            encoded.visible,
            encoded.hidden,
        );
    }
    Ok(())
}

/// RT64's MIT C++ render/HLE core behind one crate-local C ABI boundary.
/// The feature-gated implementation passes fn64's stable RDRAM allocation,
/// the task's ucode/display-list addresses, and a private register block to
/// `RT64::Application::Core`. RT64's render-to-RAM path writes the native
/// RGBA5551 framebuffer back into the same slice the existing fn64 VI path
/// presents.
pub struct Rt64Backend {
    /// RT64's GBI selection is still HLE. Apply the same exact task-entry
    /// admission as the Rust reference backend before crossing the C ABI.
    f3dex2_ucodes: gbi::F3dex2UcodeCatalog,
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
            f3dex2_ucodes: gbi::F3dex2UcodeCatalog::default(),
            #[cfg(feature = "rt64")]
            task_index: 0,
            #[cfg(feature = "rt64")]
            context: None,
            #[cfg(not(feature = "rt64"))]
            created: false,
        }
    }

    /// Admit one exact task-entry F3DEX2 text image for RT64 HLE. Unknown
    /// images return `NeedsLle` without crossing the C ABI.
    pub fn with_f3dex2_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.f3dex2_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact logical 4 KiB task-entry image, retaining only its
    /// SHA-256 identity. This mirrors `ReferenceBackend` fixture setup.
    pub fn with_f3dex2_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "F3DEX2 text admission requires one complete 4 KiB IMEM image"
        );
        self.f3dex2_ucodes.admit_text(text);
        self
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
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let context = self
                .context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?;
            if let Err(RenderError::RequiresLle { ucode_sha256 }) = self
                .f3dex2_ucodes
                .require_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
            {
                return Ok(FrameStatus::NeedsLle { ucode_sha256 });
            }
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
                    let mut diagnostic_rdram = rdram.to_vec();
                    let mut diagnostic_rsp = rsp_memory.clone();
                    let triangles = gbi::execute_display_list_f3dex2_ops(
                        &mut diagnostic_rdram,
                        &mut diagnostic_rsp,
                        task.data_ptr,
                    )
                    .unwrap_or_else(|error| {
                        panic!("failed to decode diagnostic gfx task {task_index}: {error}")
                    })
                    .into_iter()
                    .filter(|operation| matches!(operation, gbi::RenderOp::Triangle(_)))
                    .count();
                    let command_trace =
                        gbi::trace_display_list_f3dex2(&diagnostic_rdram, task.data_ptr);
                    let report = format!(
                        "task_index={task_index}\noutput_addr={output_addr:#010x}\n\
                         reference_triangle_count={}\ntask={task:#?}\n{command_trace}",
                        triangles,
                    );
                    let path = directory.join(format!("task-{task_index:04}.txt"));
                    std::fs::write(&path, report).unwrap_or_else(|error| {
                        panic!("failed to write gfx task diagnostic {path:?}: {error}")
                    });
                    eprintln!(
                        "[fn64-render-rt64] dumped gfx task #{task_index} ({} reference \
                         triangles) to {path:?}",
                        triangles
                    );
                }
            }
            context
                .process_task(rdram, rsp_memory, task, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            Ok(FrameStatus::Complete)
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, rsp_memory, task, output_addr);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let start_usize = usize::try_from(start).expect("u32 RDP start fits usize");
            let end_usize = usize::try_from(end).expect("u32 RDP end fits usize");
            if start >= end
                || !start.is_multiple_of(8)
                || !end.is_multiple_of(8)
                || end_usize > rdram.len()
            {
                return Err(RenderError::InvalidTaskBounds {
                    offset: start,
                    len: end.saturating_sub(start),
                    rdram_len: rdram.len(),
                });
            }
            debug_assert!(start_usize < end_usize);
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_rdp_commands(rdram, start, end, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            Ok(FrameStatus::Complete)
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, start, end, output_addr);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn present(&mut self, vi: ViPresentation) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .present(vi)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = vi;
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
        backend.present(ViPresentation::default()).unwrap();
        assert!(!backend
            .framebuffer()
            .unwrap()
            .has_non_uniform_content(0, 0, 0, 255));
    }

    #[test]
    fn reference_backend_blanks_scanout_without_destroying_the_rdp_image() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::new(2, 1)).unwrap();
        backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[9, 8, 7, 255]);

        backend.present(ViPresentation::default()).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );

        backend
            .present(ViPresentation {
                blanked: true,
                ..ViPresentation::default()
            })
            .unwrap();
        assert!(backend
            .presented_framebuffer()
            .unwrap()
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 255]));
        assert_eq!(
            &backend.framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );

        backend.present(ViPresentation::default()).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );
    }

    #[test]
    fn reference_backend_executes_public_fade_and_repeat_line_scanout() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::new(2, 2)).unwrap();
        backend.fb.as_mut().unwrap().pixels.copy_from_slice(&[
            10, 20, 30, 255, 40, 50, 60, 255, 110, 120, 130, 255, 140, 150, 160, 255,
        ]);

        backend
            .present(ViPresentation {
                fade: Some(0x03ff),
                ..ViPresentation::default()
            })
            .unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [110, 120, 130, 255, 140, 150, 160, 255, 110, 120, 130, 255, 140, 150, 160, 255,]
        );

        backend
            .present(ViPresentation {
                repeat_line: true,
                ..ViPresentation::default()
            })
            .unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [10, 20, 30, 255, 40, 50, 60, 255, 10, 20, 30, 255, 40, 50, 60, 255,]
        );
    }

    #[test]
    fn reference_backend_rejects_process_task_before_create() {
        let mut backend = ReferenceBackend::new();
        let mut rdram = vec![0u8; 64];
        let err = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask::default(),
                0,
            )
            .unwrap_err();
        assert!(matches!(err, RenderError::NotReady(_)));
    }

    #[test]
    fn reference_backend_lle_preflight_is_transactional() {
        const DL: usize = 0x1000;
        const TEXT: usize = 0x2000;
        const DATA: usize = 0x3200;
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        let mut rdram = vec![0u8; 0x4000];
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(TEXT as u32),
            &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        let write_word = |rdram: &mut [u8], offset: usize, word: u32| {
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        };
        write_word(&mut rdram, DL, 0xe100_0000);
        write_word(&mut rdram, DL + 4, DATA as u32);
        write_word(&mut rdram, DL + 8, 0xdd00_0007);
        write_word(&mut rdram, DL + 12, TEXT as u32);
        write_word(&mut rdram, DL + 16, 0xd500_0000);
        write_word(&mut rdram, DL + 20, 0);

        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            )
            .unwrap();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0x40),
                b"task-entry",
            )
            .unwrap();
        let rdram_before = rdram.clone();
        let rsp_before = rsp_memory.clone();
        let status = backend
            .process_task(
                &mut rdram,
                &mut rsp_memory,
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            status,
            FrameStatus::NeedsLle {
                ucode_sha256: gbi::UcodeDigest::from_text(
                    &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]
                )
                .as_bytes(),
            }
        );
        assert_eq!(rdram, rdram_before);
        assert_eq!(rsp_memory, rsp_before);
    }

    #[test]
    fn reference_backend_requires_exact_task_entry_admission() {
        const DL: usize = 0x100;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        let mut rdram = vec![0u8; 0x200];
        rdram[DL..DL + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &[0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            )
            .unwrap();
        let rdram_before = rdram.clone();
        let rsp_before = rsp_memory.clone();

        let status = backend
            .process_task(
                &mut rdram,
                &mut rsp_memory,
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            status,
            FrameStatus::NeedsLle {
                ucode_sha256: gbi::UcodeDigest::from_text(
                    &[0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE]
                )
                .as_bytes(),
            }
        );
        assert_eq!(rdram, rdram_before);
        assert_eq!(rsp_memory, rsp_before);
    }

    #[test]
    fn raw_depth_image_fill_clears_persistent_depth_across_color_switch() {
        const START: usize = 0x100;
        const Z_IMAGE: u32 = 0x400;
        const COLOR_IMAGE: u32 = 0x600;
        let commands: [(u32, u32); 7] = [
            (0xfe00_0000, Z_IMAGE),
            (0xff10_0003, Z_IMAGE),
            (0xef00_0000 | (3 << 20), 0),
            (0xf700_0000, 0xfffc_fffc),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xff10_0003, COLOR_IMAGE),
            (0xe900_0000, 0),
        ];
        let mut rdram = vec![0u8; 0x1000];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = START + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend.fb.as_mut().unwrap().depth.fill(1.0);

        backend
            .process_rdp_commands(
                &mut rdram,
                START as u32,
                (START + commands.len() * 8) as u32,
                0,
            )
            .unwrap();

        assert_eq!(
            backend.depth_image,
            Some(gbi::DepthImage { address: Z_IMAGE })
        );
        assert!(backend
            .fb
            .as_ref()
            .unwrap()
            .depth
            .iter()
            .all(|&value| value == 0x3ffff as f32));
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for pixel in 0..8 {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2)),
                0xfffc
            );
        }
    }

    #[test]
    fn raw_edge_triangle_rasterizes_into_commanded_color_image() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xff10_0007, TARGET); // RGBA16 width 8
            command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            command(0x0880_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0xf801,
            "raw edge triangle must cover its interior pixel in primitive red"
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0,
            "raw edge triangle must not paint outside its edges"
        );
        let partial_pixel = 4 * 8 + 3;
        assert_eq!(
            backend.fb.as_ref().unwrap().coverage[partial_pixel as usize],
            raster::Coverage::new(6),
            "the raw edge must retain six of the public checkerboard samples"
        );
        let partial_address = TARGET + partial_pixel * 2;
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(partial_address)),
            0xf801
        );
        assert_eq!(
            backend
                .rdram_hidden_bits
                .get(&partial_address)
                .map(|sample| sample.bits),
            Some(1),
            "coverage six stores code five as visible bit 1 plus hidden bits 01"
        );
    }

    #[test]
    fn raw_z_triangles_use_near_zero_depth_regardless_of_submission_order() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const Z_IMAGE: u32 = 0x600;
        let mut rdram = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                    0xfffc,
                );
            }
        }
        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xfe00_0000, Z_IMAGE);
            command(0xff10_0007, TARGET); // RGBA16 width 8
            command(0xef00_00f0, 0x30); // dither off | Z_CMP | Z_UPD
            command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            command(0x0980_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(2 << 16, 0); // near plane is Z=0
            command(0, 0);
            command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
            command(0x0980_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(4 << 16, 0); // submitted later, but farther
            command(0, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0x003f,
            "near blue raw triangle must reject the later far red fragment"
        );
    }

    #[test]
    fn raw_depth_update_persists_visible_and_hidden_bits_across_image_switches() {
        const START: usize = 0x100;
        const Z_IMAGE_A: u32 = 0x1000;
        const Z_IMAGE_B: u32 = 0x1200;
        const COLOR_IMAGE: u32 = 0x1400;
        let mut rdram = vec![0u8; 0x2000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE_A + pixel * 2),
                    0xfffc,
                );
            }
        }

        let mut offset = START;
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let triangle = 0x0980_0000 | yl;
        let edge_ym_yh = (ym << 16) | yh;
        let major_slope = (5.0f32 / 3.0 * 65536.0).round() as u32;
        let minor_slope = (5.0f32 / 6.0 * 65536.0).round() as u32;

        command(0xfe00_0000, Z_IMAGE_A);
        command(0xff10_0007, COLOR_IMAGE);
        command(0xef00_00f0, 0x30); // dither off | Z_CMP | Z_UPD
        command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(8 << 16, 0); // working Z = 64
        command(0, 4 << 16); // DeltaZ = |0| + |4|, then *8 = 32
        command(0xfe00_0000, Z_IMAGE_B); // commits A, then loads B
        command(0xfe00_0000, Z_IMAGE_A); // reloads A, including hidden bits
        command(0xef00_00f0, 0x10); // dither off, compare only: must not mutate A
        command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(16 << 16, 0); // farther working Z = 128, rejected
        command(0, 0);
        command(0xe900_0000, 0);
        let end = offset;

        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let pixel = 4 * 8 + 2;
        let address = Z_IMAGE_A + pixel * 2;
        let expected = depth::pack(64, 32);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
            expected.visible
        );
        assert_eq!(
            backend
                .rdram_hidden_bits
                .get(&address)
                .map(|sample| sample.bits),
            Some(expected.hidden)
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                COLOR_IMAGE + pixel * 2
            )),
            0x003f,
            "far compare-only red fragment must not replace the persisted near blue sample"
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE_B + pixel * 2)),
            0,
            "switching through a second depth image must not alias its visible samples"
        );
    }

    #[test]
    fn raw_primitive_depth_supplies_z_and_delta_without_triangle_coefficients() {
        const START: usize = 0x100;
        const Z_IMAGE: u32 = 0x1000;
        const COLOR_IMAGE: u32 = 0x1400;
        let mut rdram = vec![0u8; 0x2000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                    0xfffc,
                );
            }
        }
        let mut offset = START;
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        command(0xfe00_0000, Z_IMAGE);
        command(0xff10_0007, COLOR_IMAGE);
        command(0xee00_0000, (8 << 16) | 32); // primitive Z=8, DeltaZ=32
        command(0xef00_00f0, 0x34); // dither off | G_ZS_PRIM | Z_CMP | Z_UPD
        command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
        command(0x0880_0000 | yl, (ym << 16) | yh); // no Z coefficient words
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
        command(0xe900_0000, 0);
        let end = offset;

        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let pixel = 4 * 8 + 2;
        let depth_address = Z_IMAGE + pixel * 2;
        let expected = depth::pack(8 << 3, 32);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(depth_address)),
            expected.visible
        );
        assert_eq!(
            backend
                .rdram_hidden_bits
                .get(&depth_address)
                .map(|sample| sample.bits),
            Some(expected.hidden)
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                COLOR_IMAGE + pixel * 2
            )),
            0x003f
        );
        assert_eq!(
            backend.primitive_depth,
            Some(gbi::PrimitiveDepth { z: 8, delta_z: 32 })
        );
    }

    #[test]
    fn raw_decal_mode_accepts_correlated_depth_and_rejects_behind_depth() {
        const START: usize = 0x100;
        const Z_IMAGE: u32 = 0x1000;
        const COLOR_IMAGE: u32 = 0x1400;
        let mut rdram = vec![0u8; 0x2000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                    0xfffc,
                );
            }
        }
        let mut offset = START;
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let triangle = 0x0880_0000 | yl;
        let edge_ym_yh = (ym << 16) | yh;
        let major_slope = (5.0f32 / 3.0 * 65536.0).round() as u32;
        let minor_slope = (5.0f32 / 6.0 * 65536.0).round() as u32;

        command(0xfe00_0000, Z_IMAGE);
        command(0xff10_0007, COLOR_IMAGE);
        command(0xef00_00f0, 0x34); // dither off | G_ZS_PRIM | Z_CMP | Z_UPD | ZMODE_OPA
        command(0xee00_0000, (16 << 16) | 8); // working Z=128, DeltaZ=8
        command(0xfa00_0000, 0x0000_ffff); // blue depth seed
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(0xef00_00f0, 0x0c14); // dither off | G_ZS_PRIM | Z_CMP | ZMODE_DEC
        command(0xee00_0000, (17 << 16) | 4); // working Z=136: correlated boundary
        command(0xfa00_0000, 0xff00_00ff); // red decal must pass
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(0xee00_0000, (18 << 16) | 4); // working Z=144: clearly behind
        command(0xfa00_0000, 0x00ff_00ff); // green decal must reject
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(0xe900_0000, 0);
        let end = offset;

        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let pixel = 4 * 8 + 2;
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                COLOR_IMAGE + pixel * 2
            )),
            0xf801,
            "correlated red decal must pass while clearly-behind green rejects"
        );
        let seeded = depth::pack(128, 8);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2)),
            seeded.visible,
            "compare-only decals must retain the opaque seed depth"
        );
    }

    #[test]
    fn raw_shade_triangle_rasterizes_component_gradient() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xff10_0007, TARGET); // RGBA16 width 8
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            let drde = (32.0f32 * 5.0 / 6.0 * 65536.0).round() as u32;
            command(0x0c80_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(0, 255); // black, opaque base shade
            command(32 << 16, 0); // red increases 32 per X pixel
            command(0, 0);
            command(0, 0);
            command((drde >> 16) << 16, 0);
            command(0, 0);
            command((drde & 0xffff) << 16, 0);
            command(0, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let pixel = |x: u32, y: u32| {
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (y * 8 + x) * 2,
            ))
        };
        assert_eq!(pixel(2, 4), 0x3001);
        assert_eq!(pixel(3, 4), 0x5001);
    }

    #[test]
    fn raw_shade_texture_z_triangle_executes_maximum_width_layout() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURE: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [0xf801u16, 0x07c1, 0x003f, 0xffff, 0, 0, 0, 0];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in source.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
            }
        }
        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xff10_0007, TARGET); // RGBA16 width 8
            command(0xfd10_0003, TEXTURE); // RGBA16 width 4
            command(0xf510_0000, 7 << 24); // load tile 7, contiguous TMEM
            command(0xf300_0000, (7 << 24) | (7 << 12) | 0x800); // 8 texels
            command(0xf510_0200, 0x0008_0200); // render tile 0, clamp S/T
            command(0xf200_0000, 0x0000_c004); // 4x2 render extent
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            let dsde = (5.0f32 / 6.0 * 65536.0).round() as u32;
            command(0x0f80_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(0x00ff_00ff, 0x00ff_00ff); // opaque white base shade
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 1 << 16); // S=0, T=0, inverse-W=1
            command(1 << 16, 0); // dS/dX=1
            command(0, 0);
            command(0, 0);
            command((dsde >> 16) << 16, 0);
            command(0, 0);
            command((dsde & 0xffff) << 16, 0);
            command(0, 0);
            command(4 << 16, 0); // Z
            command(0, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let pixel = |x: u32, y: u32| {
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (y * 8 + x) * 2,
            ))
        };
        assert_eq!(pixel(2, 4), 0x07c1);
        assert_eq!(pixel(3, 4), 0x003f);
    }

    #[test]
    fn raw_command_stream_triangle_selects_mips_and_trilinear_fraction() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURES: [u32; 3] = [0x800, 0x810, 0x820];
        let mut rdram = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (address, texel) in TEXTURES.into_iter().zip([0xf801, 0x0001, 0xffff]) {
                view.write_u16(fn64_runtime::RdramAddr::from_offset(address), texel);
            }
        }

        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            let combine_w0 = 0xfc00_0000
                | (2 << 20) // cycle 0 A = TEXEL1
                | (13 << 15) // cycle 0 C = LOD_FRACTION
                | (2 << 12) // cycle 0 alpha A = TEXEL1
                | (8 << 5) // cycle 1 A = ZERO
                | 31; // cycle 1 C = ZERO
            let combine_w1 = (1 << 28) // cycle 0 B = TEXEL0
                | (8 << 24) // cycle 1 B = ZERO
                | (7 << 21) // cycle 1 alpha A = ZERO
                | (7 << 18) // cycle 1 alpha C = ZERO
                | (1 << 15) // cycle 0 D = TEXEL0
                | (1 << 12) // cycle 0 alpha B = TEXEL0
                | (1 << 9) // cycle 0 alpha D = TEXEL0
                | (7 << 3); // cycle 1 alpha B = ZERO; D = COMBINED

            command(0xff10_0007, TARGET); // RGBA16 width 8
                                          // Two-cycle, texture LOD enabled, clamp-detail mode, filter-only,
                                          // and deterministic dither disable. Raw edge `level=2` below is
                                          // the RDP primitive's maximum mip level.
            command(0xef00_0000 | (1 << 20) | (1 << 16) | (6 << 9) | 0xf0, 0);
            command(combine_w0, combine_w1);
            for (tile, address) in TEXTURES.into_iter().enumerate() {
                let tile = tile as u32;
                command(0xfd10_0000, address); // RGBA16 width 1
                command(0xf510_0200 | tile, (tile << 24) | 0x0008_0200);
                command(0xf200_0000, tile << 24); // 1x1 render tile
                command(0xf300_0000, tile << 24); // load into that tile
            }

            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            command(0x0a80_0000 | (2 << 19) | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            // S=T=0, W=1; dS/dX=dT/dY=2.5. Chapter 13.7 selects
            // tiles 1 and 2 with LOD fraction 0.25.
            command(0, 1 << 16);
            command(2 << 16, 0);
            command(0, 0);
            command(0x8000_0000, 0);
            command(0, 0);
            command(2, 0);
            command(0, 0);
            command(0x0000_8000, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0x4211,
            "LOD 2.5 must blend one quarter from black tile 1 toward white tile 2"
        );
    }

    #[test]
    fn raw_yuv_texture_rectangle_applies_set_convert_into_rdram() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURE: u32 = 0x600;
        let mut rdram = vec![0u8; 0x800];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            // Public RDP YUV16 byte order: Y0, U, Y1, V. Neutral chroma
            // makes the default public conversion table preserve each Y as
            // equal R/G/B, which gives this gate unambiguous expected pixels.
            for (index, value) in [16, 128, 235, 128].into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32),
                    value,
                );
            }
        }

        let field = |value: i16| u32::from(value as u16) & 0x1ff;
        let [k0, k1, k2, k3, k4, k5] = [175, -43, -89, 222, 114, 42].map(field);
        let set_convert = (
            0xec00_0000 | (k0 << 13) | (k1 << 4) | ((k2 >> 5) & 0x0f),
            ((k2 & 0x1f) << 27) | (k3 << 18) | (k4 << 9) | k5,
        );
        let combine_command = |rgb: [u32; 4], alpha: [u32; 4]| {
            let w0 = 0xfc00_0000
                | ((rgb[0] & 0x0f) << 20)
                | ((rgb[2] & 0x1f) << 15)
                | ((alpha[0] & 0x07) << 12)
                | ((alpha[2] & 0x07) << 9)
                | ((rgb[0] & 0x0f) << 5)
                | (rgb[2] & 0x1f);
            let w1 = ((rgb[1] & 0x0f) << 28)
                | ((rgb[1] & 0x0f) << 24)
                | ((alpha[0] & 0x07) << 21)
                | ((alpha[2] & 0x07) << 18)
                | ((rgb[3] & 0x07) << 15)
                | ((alpha[1] & 0x07) << 12)
                | ((alpha[3] & 0x07) << 9)
                | ((rgb[3] & 0x07) << 6)
                | ((alpha[1] & 0x07) << 3)
                | (alpha[3] & 0x07);
            (w0, w1)
        };

        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            // One-cycle, point sampled, G_TC_CONV, with color/alpha dither
            // disabled so this gate isolates the conversion table.
            command(0xef00_00f0, 0);
            command(set_convert.0, set_convert.1);
            let (combine_w0, combine_w1) = combine_command([8, 8, 31, 1], [7, 7, 7, 1]);
            command(combine_w0, combine_w1); // (0-0)*0+TEXEL0
            command(0xff10_0001, TARGET); // RGBA16 width 2
            command(0xfd30_0001, TEXTURE); // YUV16 width 2
            command(0xf530_0000, 7 << 24); // YUV16 load tile 7
            command(0xf300_0000, (7 << 24) | (1 << 12) | 0x800); // YUYV pair
            command(0xf530_0200, 0x0008_0200); // YUV16 render tile 0
            command(0xf200_0000, 0x0000_4000); // 2x1 render extent
            command(0xe400_0000 | ((2 * 4) << 12) | 4, 0);
            command(0, 0x0400_0400); // S/T=0, dS/dX=dT/dY=1
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(2, 1)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x1085
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
            0xef7b
        );
    }

    #[test]
    fn raw_chroma_key_commands_drive_alpha_fixup_and_compare() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURE: u32 = 0x600;
        let mut rdram = vec![0u8; 0x800];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in [0x07c1u16, 0xf801].into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TARGET + index as u32 * 2),
                    0xffff,
                );
            }
        }
        let combine_command = |rgb: [u32; 4], alpha: [u32; 4]| {
            let w0 = 0xfc00_0000
                | ((rgb[0] & 0x0f) << 20)
                | ((rgb[2] & 0x1f) << 15)
                | ((alpha[0] & 0x07) << 12)
                | ((alpha[2] & 0x07) << 9)
                | ((rgb[0] & 0x0f) << 5)
                | (rgb[2] & 0x1f);
            let w1 = ((rgb[1] & 0x0f) << 28)
                | ((rgb[1] & 0x0f) << 24)
                | ((alpha[0] & 0x07) << 21)
                | ((alpha[2] & 0x07) << 18)
                | ((rgb[3] & 0x07) << 15)
                | ((alpha[1] & 0x07) << 12)
                | ((alpha[3] & 0x07) << 9)
                | ((rgb[3] & 0x07) << 6)
                | ((alpha[1] & 0x07) << 3)
                | (alpha[3] & 0x07);
            (w0, w1)
        };

        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            // One-cycle, filter-only, chroma key enabled, alpha threshold on.
            command(0xef00_0df0, 1);
            command(0xf900_0000, 0x0000_0080); // threshold alpha = 128
            command(0xea10_0100, 0xffff_00ff); // center green, unit widths/scales
            command(0xeb00_0000, 0x0100_00ff);
            let (combine_w0, combine_w1) = combine_command([1, 6, 6, 7], [7, 7, 7, 7]);
            command(combine_w0, combine_w1); // (TEXEL0-CENTER)*SCALE
            command(0xff10_0001, TARGET); // RGBA16 width 2
            command(0xfd10_0001, TEXTURE); // RGBA16 width 2
            command(0xf510_0000, 7 << 24); // load tile 7, contiguous TMEM
            command(0xf300_0000, (7 << 24) | (1 << 12) | 0x800); // 2 texels
            command(0xf510_0200, 0x0008_0200); // render tile 0, clamp S/T
            command(0xf200_0000, 0x0000_4000); // 2x1 render extent
            command(0xe400_0000 | ((2 * 4) << 12) | 4, 0);
            command(0, 0x0400_0400);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(2, 1)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x0001
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
            0xffff
        );
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

    #[test]
    fn framebuffer_writer_and_runtime_view_agree_on_logical_pixel_order() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer.pixels[0..4].copy_from_slice(&[255, 0, 0, 255]);
        framebuffer.pixels[4..8].copy_from_slice(&[0, 0, 255, 255]);
        let mut storage = [0u8; 4];
        let mut hidden_bits = HashMap::new();

        write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);

        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(0)),
            0xF801,
            "pixel 0 must be logical RGBA5551 red"
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(2)),
            0x003F,
            "pixel 1 must be logical RGBA5551 blue"
        );
        assert_eq!(
            storage,
            [0x3F, 0x00, 0x01, 0xF8],
            "native-word storage must contain the two logical halfwords in lane-mapped order"
        );
    }

    #[test]
    fn disabled_dither_rgba16_truncates_low_three_bits() {
        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.pixels.copy_from_slice(&[7, 8, 15, 255]);
        let mut storage = [0u8; 4];
        let mut hidden_bits = HashMap::new();

        write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);

        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(0)),
            0x0043,
            "7 must remain zero while 8 and 15 truncate to one; round-to-nearest would change both boundary channels"
        );
    }

    #[test]
    fn rgba16_coverage_round_trips_visible_and_hidden_storage_bits() {
        let mut framebuffer = Framebuffer::new(8, 1);
        framebuffer.pixels.fill(255);
        for (index, coverage) in framebuffer.coverage.iter_mut().enumerate() {
            *coverage = raster::Coverage::new(index as u8 + 1);
        }
        let mut storage = [0u8; 16];
        let mut hidden_bits = HashMap::new();

        write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);
        let view = fn64_runtime::RdramView::from_storage(&storage);
        for index in 0..8u32 {
            let address = index * 2;
            let visible = view.read_u16(fn64_runtime::RdramAddr::from_offset(address));
            let stored = index as u8;
            assert_eq!((visible & 1) as u8, stored >> 2);
            assert_eq!(
                hidden_bits.get(&address).map(|sample| sample.bits),
                Some(stored & 3)
            );
        }

        let mut loaded = Framebuffer::new(8, 1);
        load_rgba5551_framebuffer(
            &storage,
            gbi::ColorImage {
                format: gbi::ColorImage::RGBA_FORMAT,
                size: gbi::ColorImage::BITS_16,
                width: 8,
                address: 0,
            },
            &mut loaded,
            &mut hidden_bits,
        );
        assert_eq!(loaded.coverage, framebuffer.coverage);
    }

    #[test]
    fn rgba32_round_trips_five_bit_alpha_and_three_bit_coverage() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer
            .pixels
            .copy_from_slice(&[0x12, 0x34, 0x56, 0x29, 0xab, 0xcd, 0xef, 0xbd]);
        framebuffer.coverage[0] = raster::Coverage::new(3);
        framebuffer.coverage[1] = raster::Coverage::FULL;
        let mut storage = [0u8; 8];

        write_rgba8888_framebuffer(&mut storage, 0, &framebuffer);
        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(0)),
            0x1234_5645
        );
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(4)),
            0xabcd_eff7
        );

        let mut loaded = Framebuffer::new(2, 1);
        load_rgba8888_framebuffer(
            &storage,
            gbi::ColorImage {
                format: gbi::ColorImage::RGBA_FORMAT,
                size: gbi::ColorImage::BITS_32,
                width: 2,
                address: 0,
            },
            &mut loaded,
        );
        assert_eq!(loaded.pixels, framebuffer.pixels);
        assert_eq!(loaded.coverage, framebuffer.coverage);
    }

    #[test]
    fn rgba32_memory_alpha_truncates_low_three_bits() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer
            .pixels
            .copy_from_slice(&[1, 2, 3, 7, 4, 5, 6, 8]);
        let mut storage = [0u8; 8];

        write_rgba8888_framebuffer(&mut storage, 0, &framebuffer);

        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(0)),
            0x0102_03e0
        );
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(4)),
            0x0405_06e1
        );
    }

    #[test]
    fn changed_cpu_visible_word_reconstructs_its_hidden_bits_from_the_lsb() {
        let mut hidden_bits = HashMap::from([(
            0,
            RdramHiddenSample {
                visible: 1,
                bits: 1,
            },
        )]);
        assert_eq!(read_rdram_hidden_bits(&mut hidden_bits, 0, 0), 0);
        assert_eq!(
            hidden_bits.get(&0),
            Some(&RdramHiddenSample {
                visible: 0,
                bits: 0,
            })
        );
        assert_eq!(read_rdram_hidden_bits(&mut hidden_bits, 0, 1), 3);
    }

    #[test]
    fn ordered_fill_rectangles_write_the_explicit_color_image() {
        const DL: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        // G_RDPSETOTHERMODE: G_CYC_FILL.
        write_command(&mut rdram, offset, 0xef00_0000 | (3 << 20), 0);
        offset += 8;
        // G_SETCIMG RGBA16 width 4.
        write_command(&mut rdram, offset, 0xff10_0003, TARGET);
        offset += 8;
        // Red fill across the full 4x2 target.
        write_command(&mut rdram, offset, 0xf700_0000, 0xf801_f801);
        offset += 8;
        write_command(&mut rdram, offset, 0xf600_0000 | ((3 * 4) << 12) | 4, 0);
        offset += 8;
        // Blue overwrites row 0 pixels 1..2. Keeping two fill operations in
        // one stream proves the decoder/backend no longer groups by primitive.
        write_command(&mut rdram, offset, 0xf700_0000, 0x003f_003f);
        offset += 8;
        write_command(&mut rdram, offset, 0xf600_0000 | ((2 * 4) << 12), 4 << 12);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let expected = [
            0xf801, 0x003f, 0x003f, 0xf801, 0xf801, 0xf801, 0xf801, 0xf801,
        ];
        for (index, expected) in expected.into_iter().enumerate() {
            let address = fn64_runtime::RdramAddr::from_offset(TARGET + index as u32 * 2);
            assert_eq!(view.read_u16(address), expected, "pixel {index}");
        }
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2), 0xffff);
        // RDP target state survives task boundaries. A second task omits
        // G_SETCIMG and must continue drawing the prior color image rather
        // than falling back to output_addr/VI state. The task-boundary import
        // must also retain the CPU's intervening white write to pixel 1.
        let mut second = DL;
        write_command(&mut rdram, second, 0xef00_0000 | (3 << 20), 0);
        second += 8;
        write_command(&mut rdram, second, 0xf700_0000, 0x07c1_07c1);
        second += 8;
        write_command(&mut rdram, second, 0xf600_0000, 0);
        second += 8;
        write_command(&mut rdram, second, 0xe900_0000, 0);
        second += 8;
        write_command(&mut rdram, second, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x07c1
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
            0xffff,
            "second task must re-import CPU-visible writes to untouched persistent-target pixels"
        );
    }

    #[test]
    fn reference_backend_preserves_rdp_mode_and_fill_registers_between_tasks() {
        const DL: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x800];
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(1, 1)).unwrap();

        // Task one only programs device registers; it emits no pixels.
        write_command(&mut rdram, DL, 0xef00_0000 | (3 << 20), 0);
        write_command(&mut rdram, DL + 8, 0xff10_0000, TARGET);
        write_command(&mut rdram, DL + 16, 0xf700_0000, 0xf801_f801);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        // Task two deliberately omits SETOTHERMODE, SETCIMG, and SETFILLCOLOR.
        // All three registers belong to the RDP and remain selected.
        write_command(&mut rdram, DL, 0xf600_0000, 0);
        write_command(&mut rdram, DL + 8, 0xe900_0000, 0);
        write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            fn64_runtime::RdramView::from_storage(&rdram)
                .read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0xf801
        );
    }

    #[test]
    fn raw_dpc_and_f3dex2_hle_share_one_persistent_rdp_register_file() {
        const RAW: usize = 0x100;
        const DL: usize = 0x200;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x800];
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(1, 1)).unwrap();

        // A bounded raw DPC submission programs the device without drawing.
        write_command(&mut rdram, RAW, 0xef00_0000 | (3 << 20), 0);
        write_command(&mut rdram, RAW + 8, 0xff10_0000, TARGET);
        write_command(&mut rdram, RAW + 16, 0xf700_0000, 0x07c1_07c1);
        backend
            .process_rdp_commands(&mut rdram, RAW as u32, (RAW + 24) as u32, 0)
            .unwrap();

        // The next admitted HLE task consumes those same registers.
        write_command(&mut rdram, DL, 0xf600_0000, 0);
        write_command(&mut rdram, DL + 8, 0xe900_0000, 0);
        write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            fn64_runtime::RdramView::from_storage(&rdram)
                .read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x07c1
        );
    }

    #[test]
    fn rgba32_fill_cycle_writes_rgb_alpha_and_coverage_packing() {
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 6] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff18_0003, 0x400),
            (0xf700_0000, 0x1234_56e5),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xe900_0000, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for index in 0..8 {
            assert_eq!(
                view.read_u32(fn64_runtime::RdramAddr::from_offset(0x400 + index * 4)),
                0x1234_56e5,
                "RGBA32 fill pixel {index}"
            );
        }
        let framebuffer = backend.framebuffer().unwrap();
        assert_eq!(&framebuffer.pixels[..4], &[0x12, 0x34, 0x56, 0x29]);
        assert_eq!(framebuffer.coverage[0], raster::Coverage::FULL);
    }

    #[test]
    fn ordered_target_switch_commits_each_rgba_format_with_its_own_packing() {
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 9] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff10_0001, 0x400),
            (0xf700_0000, 0xf801_f801),
            (0xf600_0000 | (4 << 12), 0),
            (0xff18_0001, 0x500),
            (0xf700_0000, 0x1234_56e5),
            (0xf600_0000 | (4 << 12), 0),
            (0xe900_0000, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(2, 1)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for address in [0x400, 0x402] {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
                0xf801
            );
        }
        for address in [0x500, 0x504] {
            assert_eq!(
                view.read_u32(fn64_runtime::RdramAddr::from_offset(address)),
                0x1234_56e5
            );
        }
    }

    #[test]
    fn intensity8_fill_uses_all_four_fill_register_bytes_and_ignores_coverage() {
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 6] = [
            (0xef00_0000 | (3 << 20), 0),
            // Set Color Image: arbitrary format field, public 8-bit size,
            // width four. Figure 15.5.4 defines size=8 as intensity bytes.
            (0xff00_0000 | (4 << 21) | (1 << 19) | 3, 0x400),
            (0xf700_0000, 0x1234_5678),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xe900_0000, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for row in 0..2 {
            for (column, intensity) in [0x12, 0x34, 0x56, 0x78].into_iter().enumerate() {
                assert_eq!(
                    view.read_u8(fn64_runtime::RdramAddr::from_offset(
                        0x400 + row * 4 + column as u32
                    )),
                    intensity
                );
            }
        }
        let framebuffer = backend.framebuffer().unwrap();
        assert_eq!(
            &framebuffer.pixels[..16],
            &[
                0x12, 0x12, 0x12, 255, 0x34, 0x34, 0x34, 255, 0x56, 0x56, 0x56, 255, 0x78, 0x78,
                0x78, 255
            ]
        );
        assert!(framebuffer
            .coverage
            .iter()
            .all(|coverage| *coverage == raster::Coverage::FULL));
    }

    #[test]
    fn intensity8_target_import_and_commit_share_logical_rdram_bytes() {
        let mut rdram = vec![0u8; 0x500];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, intensity) in [17, 34, 51, 68].into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(0x400 + index as u32),
                    intensity,
                );
            }
        }
        let target = gbi::ColorImage {
            format: 2,
            size: gbi::ColorImage::BITS_8,
            width: 4,
            address: 0x400,
        };
        let mut framebuffer = Framebuffer::new(4, 1);
        let mut hidden_bits = HashMap::new();
        load_color_image(&rdram, target, &mut framebuffer, &mut hidden_bits);
        assert_eq!(
            framebuffer.pixels,
            [17, 17, 17, 255, 34, 34, 34, 255, 51, 51, 51, 255, 68, 68, 68, 255]
        );

        framebuffer.pixels[0] = 0xa5;
        framebuffer.pixels[4] = 0xb6;
        framebuffer.pixels[8] = 0xc7;
        framebuffer.pixels[12] = 0xd8;
        framebuffer.coverage.fill(raster::Coverage::new(1));
        commit_color_image(&mut rdram, target, &framebuffer, &mut hidden_bits);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            (0..4)
                .map(|index| view.read_u8(fn64_runtime::RdramAddr::from_offset(0x400 + index)))
                .collect::<Vec<_>>(),
            [0xa5, 0xb6, 0xc7, 0xd8]
        );
        assert!(
            hidden_bits.is_empty(),
            "I8 ignores RDRAM hidden coverage bits"
        );
    }

    #[test]
    fn same_color_image_bytes_reinterpret_between_index8_and_rgba16() {
        const ADDRESS: u32 = 0x400;
        let rgba16 = gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_16,
            width: 2,
            address: ADDRESS,
        };
        let index8 = gbi::ColorImage {
            format: gbi::ColorImage::CI_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 4,
            address: ADDRESS,
        };
        let mut rdram = vec![0u8; 0x500];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(ADDRESS), 0xf801);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(ADDRESS + 2), 0x07c1);
        }

        let mut framebuffer = Framebuffer::new(2, 1);
        let mut hidden_bits = HashMap::new();
        load_color_image(&rdram, rgba16, &mut framebuffer, &mut hidden_bits);
        assert_eq!(&framebuffer.pixels[..8], &[255, 0, 0, 255, 0, 255, 0, 255]);

        load_color_image(&rdram, index8, &mut framebuffer, &mut hidden_bits);
        assert_eq!(
            framebuffer
                .pixels
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            [0xf8, 0x01, 0x07, 0xc1]
        );

        for (pixel, byte) in framebuffer
            .pixels
            .chunks_exact_mut(4)
            .zip([0x00, 0x3f, 0xff, 0xff])
        {
            pixel[..3].fill(byte);
        }
        commit_color_image(&mut rdram, index8, &framebuffer, &mut hidden_bits);
        load_color_image(&rdram, rgba16, &mut framebuffer, &mut hidden_bits);
        assert_eq!(
            &framebuffer.pixels[..8],
            &[0, 0, 255, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn reference_renderer_rejects_invalid_non_rgba_16bit_targets_by_name() {
        let mut rdram = vec![0u8; 0x1000];
        rdram[0x100..0x104].copy_from_slice(&0xff70_0003u32.to_ne_bytes());
        rdram[0x104..0x108].copy_from_slice(&0x400u32.to_ne_bytes());
        rdram[0x108..0x10c].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        rdram[0x10c..0x110].copy_from_slice(&0u32.to_ne_bytes());
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        let error = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap_err();
        assert!(error.to_string().contains("format=3 size=2"));
        assert!(error.to_string().contains("requires 8-bit intensity"));
    }

    #[test]
    fn f3dex2_color_writes_require_persistent_setcimg_not_output_addr() {
        const DL: usize = 0x100;
        const VERTICES: usize = 0x200;
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        let mut rdram = vec![0u8; 0x2000];
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        write_command(
            &mut rdram,
            DL,
            (u32::from(gbi::G_VTX) << 24) | (3 << 12) | (3 << 1),
            VERTICES as u32,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(gbi::G_TRI1) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        write_command(&mut rdram, DL + 16, u32::from(gbi::G_ENDDL) << 24, 0);

        let error = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0x1000,
            )
            .unwrap_err();

        assert!(error.to_string().contains("no persistent G_SETCIMG"));
        assert!(error.to_string().contains("output_addr state is not"));
    }

    #[test]
    fn reference_renderer_rejects_fillrect_outside_fill_cycle() {
        let mut rdram = vec![0u8; 0x1000];
        let commands = [
            (0xff10_0003u32, 0x400u32),
            (0xf700_0000, 0xf801_f801),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        let error = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap_err();
        assert!(error.to_string().contains("G_FILLRECT in OneCycle cycle"));
    }

    #[test]
    fn copy_texture_rectangle_samples_rgba16_into_color_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [
            0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
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
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        // Copy cycle, explicit RGBA16 destination, and RGBA16 source image.
        write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xff10_0003, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd10_0003, TEXTURE);
        offset += 8;
        // Load tile 7 is contiguous; render tile 0 supplies the row stride.
        write_command(&mut rdram, offset, 0xf510_0000, 7 << 24);
        offset += 8;
        write_command(
            &mut rdram,
            offset,
            0xf300_0000,
            (7 << 24) | (7 << 12) | 0x800,
        );
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c004);
        offset += 8;
        // Inclusive copy rectangle (0,0)..(3,1), tile 0.
        write_command(&mut rdram, offset, 0xe400_0000 | ((3 * 4) << 12) | 4, 0);
        offset += 8;
        // s=t=0; dsdx=4<<10 means one texel/pixel in copy mode, dtdy=1<<10.
        write_command(&mut rdram, offset, 0, 0x1000_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for (index, expected) in source.into_iter().enumerate() {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2
                )),
                expected,
                "copied pixel {index}"
            );
        }
    }

    #[test]
    fn copy_ci8_indices_directly_to_eight_bit_color_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, value) in [0u8, 1, 0x7f, 0xff].into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32),
                    value,
                );
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(TARGET + index as u32),
                    0xaa,
                );
            }
        }
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        // Copy cycle, no TLUT dereference, threshold alpha compare at index 1.
        write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 1);
        offset += 8;
        write_command(&mut rdram, offset, 0xf900_0000, 1);
        offset += 8;
        // Public 8-bit color image and CI8 texture image, both width four.
        write_command(&mut rdram, offset, 0xff88_0003, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd48_0003, TEXTURE);
        offset += 8;
        write_command(&mut rdram, offset, 0xf548_0000, 7 << 24);
        offset += 8;
        write_command(&mut rdram, offset, 0xf300_0000, (7 << 24) | (3 << 12));
        offset += 8;
        write_command(&mut rdram, offset, 0xf548_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c000);
        offset += 8;
        write_command(&mut rdram, offset, 0xe400_0000 | ((3 * 4) << 12), 0);
        offset += 8;
        write_command(&mut rdram, offset, 0, 0x1000_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 1)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let actual = std::array::from_fn(|index| {
            view.read_u8(fn64_runtime::RdramAddr::from_offset(TARGET + index as u32))
        });
        assert_eq!(actual, [0xaa, 1, 0x7f, 0xff]);
    }

    #[test]
    fn flipped_copy_texture_rectangle_transposes_rgba16_into_color_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [0xf801u16, 0x07c1, 0x003f, 0xffff];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in source.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
            }
        }
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xff10_0001, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd10_0001, TEXTURE);
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 7 << 24);
        offset += 8;
        write_command(&mut rdram, offset, 0xf400_0000, (7 << 24) | (4 << 12) | 4);
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_4004);
        offset += 8;
        // Inclusive 2x2 copy rectangle. FLIP makes S advance down screen Y
        // and T advance across screen X while copy-mode dsdx retains 4<<10.
        write_command(&mut rdram, offset, 0xe500_0000 | (4 << 12) | 4, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0, 0x1000_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(2, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let expected = [source[0], source[2], source[1], source[3]];
        for (index, pixel) in expected.into_iter().enumerate() {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2
                )),
                pixel,
                "transposed copy pixel {index}"
            );
        }
    }

    #[test]
    fn one_cycle_texture_rectangle_runs_combiner_into_commanded_rdram_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [
            0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
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
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        let combine_command = |rgb: [u32; 4], alpha: [u32; 4]| {
            let w0 = 0xfc00_0000
                | ((rgb[0] & 0x0f) << 20)
                | ((rgb[2] & 0x1f) << 15)
                | ((alpha[0] & 0x07) << 12)
                | ((alpha[2] & 0x07) << 9)
                | ((rgb[0] & 0x0f) << 5)
                | (rgb[2] & 0x1f);
            let w1 = ((rgb[1] & 0x0f) << 28)
                | ((rgb[1] & 0x0f) << 24)
                | ((alpha[0] & 0x07) << 21)
                | ((alpha[2] & 0x07) << 18)
                | ((rgb[3] & 0x07) << 15)
                | ((alpha[1] & 0x07) << 12)
                | ((alpha[3] & 0x07) << 9)
                | ((rgb[3] & 0x07) << 6)
                | ((alpha[1] & 0x07) << 3)
                | (alpha[3] & 0x07);
            (w0, w1)
        };

        let mut offset = DL;
        // (0-0)*0+TEXEL0 for RGBA in both programmed combiner slots.
        let (combine_w0, combine_w1) = combine_command([8, 8, 31, 1], [7, 7, 7, 1]);
        write_command(&mut rdram, offset, combine_w0, combine_w1);
        offset += 8;
        write_command(&mut rdram, offset, 0xff10_0003, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd10_0003, TEXTURE);
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0000, 7 << 24);
        offset += 8;
        write_command(
            &mut rdram,
            offset,
            0xf300_0000,
            (7 << 24) | (7 << 12) | 0x800,
        );
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c004);
        offset += 8;
        // One-cycle lower/right bounds are exclusive: (0,0)..(4,2).
        write_command(
            &mut rdram,
            offset,
            0xe400_0000 | ((4 * 4) << 12) | (2 * 4),
            0,
        );
        offset += 8;
        write_command(&mut rdram, offset, 0, 0x0400_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for (index, expected) in source.into_iter().enumerate() {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2
                )),
                expected,
                "combined pixel {index}"
            );
        }
    }

    #[test]
    fn combined_texture_rectangle_rejects_unmodeled_state_by_name() {
        let texture = gbi::Texture {
            format: 0,
            size: 2,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![255; 4]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut rectangle = gbi::TextureRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 1.0,
            lry: 1.0,
            tile: 0,
            s: 0.0,
            t: 0.0,
            dsdx: 1 << 10,
            dtdy: 1 << 10,
            flip: false,
            other_mode: gbi::OtherMode::default(),
            combiner: gbi::CombinerState::default(),
            blender: gbi::BlenderState {
                cycle_count: 1,
                ..gbi::BlenderState::default()
            },
            scissor: None,
            texture: Some(texture),
            texture1: None,
        };

        let shade_error = validate_texture_rectangle(&rectangle, None).unwrap_err();
        assert!(shade_error.to_string().contains("selects SHADE"));
        assert!(shade_error
            .to_string()
            .contains("rectangle commands carry no shade attributes"));

        let passthrough = gbi::CombinerCycle {
            rgb: [
                gbi::ColorSource::Zero,
                gbi::ColorSource::Zero,
                gbi::ColorSource::Zero,
                gbi::ColorSource::Texel0,
            ],
            alpha: [
                gbi::AlphaSource::Zero,
                gbi::AlphaSource::Zero,
                gbi::AlphaSource::Zero,
                gbi::AlphaSource::Texel0,
            ],
        };
        rectangle.combiner.mode.cycles = [passthrough; 2];
        rectangle.other_mode = gbi::OtherMode::from_raw(gbi::OtherMode::default().raw_high(), 3, 0);
        let alpha_dither_error = validate_texture_rectangle(&rectangle, None).unwrap_err();
        assert!(alpha_dither_error.to_string().contains("G_AC_DITHER"));
        assert!(alpha_dither_error
            .to_string()
            .contains("hardware pseudo-random alpha threshold"));

        rectangle.other_mode =
            gbi::OtherMode::from_raw(gbi::OtherMode::default().raw_high(), 0x10, 0);
        let depth_error = validate_texture_rectangle(&rectangle, None).unwrap_err();
        assert!(depth_error
            .to_string()
            .contains("rectangles require G_ZS_PRIM"));
    }

    #[test]
    fn copy_texture_rectangle_rejects_mismatched_memory_layouts() {
        let texture = gbi::Texture {
            format: gbi::ColorImage::CI_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![1; 4]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let rectangle = gbi::TextureRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 1.0,
            lry: 1.0,
            tile: 0,
            s: 0.0,
            t: 0.0,
            dsdx: 4 << 10,
            dtdy: 1 << 10,
            flip: false,
            other_mode: gbi::OtherMode::from_raw(2 << 20, 0, 0),
            combiner: gbi::CombinerState::default(),
            blender: gbi::BlenderState::default(),
            scissor: None,
            texture: Some(texture),
            texture1: None,
        };
        let rgba16_target = gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_16,
            width: 1,
            address: 0,
        };
        let error = validate_texture_rectangle(&rectangle, Some(rgba16_target)).unwrap_err();
        assert!(error.to_string().contains("does not match color target"));
        assert!(error.to_string().contains("format=0 size=2"));
    }
}
