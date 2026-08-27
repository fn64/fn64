//! N64 VI framebuffer -> window pixel-buffer conversion, factored out so the
//! RGBA5551->RGBA8888 unpack is unit-testable without a live window.
//!
//! The game's pixels are logical **RGBA5551 halfwords** (`RRRRRGGGGGBBBBBA`),
//! but fn64's rdram is native-endian-WORD storage: pixel `i`'s halfword
//! lives at byte offset `(2*i) ^ 2` within a word-aligned framebuffer and is
//! read native-endian -- the exact rule `examples/oot-boot`'s (fn64#1-fixed)
//! `dump_rgba5551_as_png` and the runtime's `MEM_H` accessors use. Decoding
//! flat big-endian instead scrambles the halfword pair inside every 32-bit
//! word: colors shift fields (green tint) and neighboring pixels interleave
//! (pixel noise). `pixels`
//! (wgpu's `Rgba8UnormSrgb` texture) wants a tightly-packed RGBA8888 buffer,
//! one byte each R,G,B,A in that order. This converts one into the other.

use fn64_runtime::{RdramAddr, RdramView};
use sha2::{Digest, Sha256};

/// N64 low-res NTSC framebuffer dimensions, used only as the pre-boot
/// default: the surface starts at this size and is resized to the guest's
/// own programmed geometry once VI_WIDTH and VI_V_START are latched.
///
/// **`FB_HEIGHT` is not the scanned-out line count.** The VI's active output
/// rectangle comes from V_START, and a game is free to program fewer lines
/// than 240 -- WM2000 programs 237. Rows past that rectangle were never
/// rendered into, so presenting a fixed 240 shows stale RDRAM along the
/// bottom edge. `fn64_abi::vi_output_height` is the authority; this constant
/// is only the value to use before the guest has programmed one.
pub const FB_WIDTH: usize = 320;
pub const FB_HEIGHT: usize = 240;
/// RGBA5551 is 2 bytes per pixel.
pub const FB_BYTES: usize = FB_WIDTH * FB_HEIGHT * 2;

/// Exact inputs consumed by one shell framebuffer decode.
///
/// This is deliberately byte-backed rather than hash-backed: equality is an
/// authority for suppressing a redundant window submission, so a collision
/// may not turn a changed guest framebuffer into a missed redraw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentDependency {
    start: usize,
    src_stride: usize,
    dst_width: usize,
    dst_height: usize,
    pixels: PresentPixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PresentPixels {
    Blanked,
    Rgba5551(Box<[u8]>),
}

/// Display policy which changes the submitted image without changing the VI
/// framebuffer bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentPolicy {
    overscan: u32,
    zoom_fill: bool,
}

/// Why one pump cannot form the same exact framebuffer dependency that a
/// successful ordinary presentation installs in [`PresentCache`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncacheablePresentReason {
    Overlay,
    FrameTrip,
    FrameDump,
    MissingFramebuffer,
    UnavailableFramebuffer,
    OutsideRdram,
    UnalignedFramebuffer,
}

impl UncacheablePresentReason {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Overlay => "Overlay",
            Self::FrameTrip => "FrameTrip",
            Self::FrameDump => "FrameDump",
            Self::MissingFramebuffer => "MissingFramebuffer",
            Self::UnavailableFramebuffer => "UnavailableFramebuffer",
            Self::OutsideRdram => "OutsideRdram",
            Self::UnalignedFramebuffer => "UnalignedFramebuffer",
        }
    }
}

/// Canonical identity of the bytes and VI geometry consumed by one ordinary
/// framebuffer decode. SHA-256 is the cross-run comparison identity; exact
/// redraw suppression still requires byte equality against the owned prior
/// successful dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheablePresentDependency {
    pub start: usize,
    pub src_stride: usize,
    pub dst_width: usize,
    pub dst_height: usize,
    pub blanked: bool,
    pub bytes: usize,
    pub fnv_digest: u64,
    pub sha256: [u8; 32],
}

/// Dependency observation made once for a measured pump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentDependencyObservation {
    Cacheable(CacheablePresentDependency),
    Uncacheable(UncacheablePresentReason),
}

/// What the cache experiment decided for one measured pump. `dependency` is
/// the canonical comparison identity. Mode, byte equality, and redraw policy
/// are deliberately separate so Observe/Suppress A/B logs can require equal
/// inputs without requiring equal dispositions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentDependencyReceipt {
    pub mode: PresentCacheMode,
    pub policy: PresentPolicy,
    pub dependency: PresentDependencyObservation,
    pub exact_hit: bool,
    pub suppress_redraw: bool,
    pub generation: u64,
    pub invalidations: u64,
    pub probe_ns: u64,
}

impl PresentDependencyReceipt {
    fn cacheable(
        mode: PresentCacheMode,
        policy: PresentPolicy,
        dependency: CacheablePresentDependency,
        exact_hit: bool,
        generation: u64,
        invalidations: u64,
    ) -> Self {
        Self {
            mode,
            policy,
            dependency: PresentDependencyObservation::Cacheable(dependency),
            exact_hit,
            suppress_redraw: mode.suppresses_redraw(exact_hit),
            generation,
            invalidations,
            probe_ns: 0,
        }
    }

    pub fn uncacheable(
        mode: PresentCacheMode,
        policy: PresentPolicy,
        reason: UncacheablePresentReason,
        generation: u64,
        invalidations: u64,
    ) -> Self {
        Self {
            mode,
            policy,
            dependency: PresentDependencyObservation::Uncacheable(reason),
            exact_hit: false,
            suppress_redraw: false,
            generation,
            invalidations,
            probe_ns: 0,
        }
    }

    pub fn with_probe_ns(mut self, probe_ns: u64) -> Self {
        self.probe_ns = probe_ns;
        self
    }
}

impl PresentPolicy {
    pub const fn new(overscan: u32, zoom_fill: bool) -> Self {
        Self {
            overscan,
            zoom_fill,
        }
    }

    pub const fn overscan(self) -> u32 {
        self.overscan
    }

    pub const fn zoom_fill(self) -> bool {
        self.zoom_fill
    }
}

/// Runtime disposition of the exact presentation dependency experiment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresentCacheMode {
    Disabled,
    Observe,
    #[default]
    Suppress,
}

impl PresentCacheMode {
    pub fn from_env_value(value: Option<&str>) -> Self {
        match value {
            None | Some("1") => Self::Suppress,
            Some("0") => Self::Disabled,
            Some("observe") => Self::Observe,
            Some(value) => {
                panic!("FN64_PRESENT_CACHE={value:?} is invalid; expected 0, observe, or 1")
            }
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Observe => "observe",
            Self::Suppress => "suppress",
        }
    }

    pub const fn samples_dependencies(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// Convert an exact dependency comparison into the pump's redraw answer.
    /// Observe intentionally records the hit while refusing to skip the draw.
    pub const fn suppresses_redraw(self, exact_hit: bool) -> bool {
        matches!(self, Self::Suppress) && exact_hit
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PresentCacheStats {
    pub requests: u64,
    pub hits: u64,
    pub misses: u64,
    pub successful_presents: u64,
    pub failed_presents: u64,
    pub invalidations: u64,
    pub dependency_samples: u64,
    pub dependency_bytes: u64,
    pub logical_digest: u64,
}

#[derive(Debug)]
struct CachedPresent {
    generation: u64,
    dependency: PresentDependency,
}

/// Exact presentation reuse authority plus its experiment accounting.
///
/// A generation closes the class of stale-window bugs where framebuffer bytes
/// remain unchanged while composition, display policy, or surface state
/// changes. Only a successful submission installs an authority in the current
/// generation; a failed submission invalidates it so the next pump retries.
#[derive(Debug, Default)]
pub struct PresentCache {
    generation: u64,
    policy: Option<PresentPolicy>,
    last: Option<CachedPresent>,
    stats: PresentCacheStats,
}

impl PresentCache {
    pub fn synchronize_policy(&mut self, policy: PresentPolicy) {
        match self.policy.replace(policy) {
            Some(previous) if previous != policy => self.invalidate(),
            _ => {}
        }
    }

    pub fn invalidate(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("present cache generation overflow");
        self.stats.invalidations = self.stats.invalidations.saturating_add(1);
    }

    pub fn record_uncacheable_request(
        &mut self,
        mode: PresentCacheMode,
        policy: PresentPolicy,
        reason: UncacheablePresentReason,
    ) -> PresentDependencyReceipt {
        self.stats.requests = self.stats.requests.saturating_add(1);
        self.stats.misses = self.stats.misses.saturating_add(1);
        PresentDependencyReceipt::uncacheable(
            mode,
            policy,
            reason,
            self.generation,
            self.stats.invalidations,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn probe(
        &mut self,
        mode: PresentCacheMode,
        policy: PresentPolicy,
        rdram: &[u8],
        start: usize,
        src_stride: usize,
        dst_width: usize,
        dst_height: usize,
        blanked: bool,
    ) -> PresentDependencyReceipt {
        self.stats.requests = self.stats.requests.saturating_add(1);
        let comparable = self.last.as_ref().and_then(|cached| {
            (cached.generation == self.generation).then_some(&cached.dependency)
        });
        let (dependency, hit) = observe_dependency(
            rdram, start, src_stride, dst_width, dst_height, blanked, comparable,
        );
        self.stats.dependency_samples = self.stats.dependency_samples.saturating_add(1);
        self.stats.dependency_bytes = self
            .stats
            .dependency_bytes
            .saturating_add(u64::try_from(dependency.bytes).unwrap_or(u64::MAX));
        self.stats.logical_digest = fnv_u64(self.stats.logical_digest, dependency.fnv_digest);
        if hit {
            self.stats.hits = self.stats.hits.saturating_add(1);
        } else {
            self.stats.misses = self.stats.misses.saturating_add(1);
        }
        PresentDependencyReceipt::cacheable(
            mode,
            policy,
            dependency,
            hit,
            self.generation,
            self.stats.invalidations,
        )
    }

    #[cfg(test)]
    fn is_current(
        &mut self,
        rdram: &[u8],
        start: usize,
        src_stride: usize,
        dst_width: usize,
        dst_height: usize,
        blanked: bool,
    ) -> bool {
        self.probe(
            PresentCacheMode::Observe,
            self.policy.unwrap_or(PresentPolicy::new(0, false)),
            rdram,
            start,
            src_stride,
            dst_width,
            dst_height,
            blanked,
        )
        .exact_hit
    }

    pub fn record_success(&mut self, dependency: PresentDependency) {
        self.stats.successful_presents = self.stats.successful_presents.saturating_add(1);
        self.last = Some(CachedPresent {
            generation: self.generation,
            dependency,
        });
    }

    pub fn record_failure(&mut self) {
        self.stats.failed_presents = self.stats.failed_presents.saturating_add(1);
        self.invalidate();
    }

    pub fn stats(&self) -> PresentCacheStats {
        self.stats
    }
}

impl PresentDependency {
    pub fn capture(
        rdram: &[u8],
        start: usize,
        src_stride: usize,
        dst_width: usize,
        dst_height: usize,
        blanked: bool,
    ) -> Self {
        let pixels = if blanked {
            PresentPixels::Blanked
        } else {
            PresentPixels::Rgba5551(
                decoded_storage(rdram, start, src_stride, dst_width, dst_height).into(),
            )
        };
        Self {
            start,
            src_stride,
            dst_width,
            dst_height,
            pixels,
        }
    }

    pub fn matches(
        &self,
        rdram: &[u8],
        start: usize,
        src_stride: usize,
        dst_width: usize,
        dst_height: usize,
        blanked: bool,
    ) -> bool {
        if self.start != start
            || self.src_stride != src_stride
            || self.dst_width != dst_width
            || self.dst_height != dst_height
        {
            return false;
        }
        match (&self.pixels, blanked) {
            (PresentPixels::Blanked, true) => true,
            (PresentPixels::Rgba5551(saved), false) => {
                saved.as_ref() == decoded_storage(rdram, start, src_stride, dst_width, dst_height)
            }
            _ => false,
        }
    }
}

fn fnv_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

fn fnv_u64(hash: u64, value: u64) -> u64 {
    fnv_bytes(hash, &value.to_le_bytes())
}

#[cfg(test)]
thread_local! {
    static DEPENDENCY_BYTE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn observe_dependency(
    rdram: &[u8],
    start: usize,
    src_stride: usize,
    dst_width: usize,
    dst_height: usize,
    blanked: bool,
    comparable: Option<&PresentDependency>,
) -> (CacheablePresentDependency, bool) {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    let mut sha256 = Sha256::new();
    sha256.update(b"fn64.present-dependency.v1\0");
    for value in [start, src_stride, dst_width, dst_height] {
        let value = u64::try_from(value).unwrap_or(u64::MAX);
        hash = fnv_u64(hash, value);
        sha256.update(value.to_le_bytes());
    }
    hash = fnv_bytes(hash, &[u8::from(blanked)]);
    sha256.update([u8::from(blanked)]);
    let same_shape = comparable.is_some_and(|saved| {
        saved.start == start
            && saved.src_stride == src_stride
            && saved.dst_width == dst_width
            && saved.dst_height == dst_height
    });
    let (bytes, exact_hit) = if blanked {
        let hit = same_shape
            && comparable.is_some_and(|saved| matches!(saved.pixels, PresentPixels::Blanked));
        (0, hit)
    } else {
        let pixels = decoded_storage(rdram, start, src_stride, dst_width, dst_height);
        let expected = comparable.and_then(|saved| match &saved.pixels {
            PresentPixels::Rgba5551(bytes) if same_shape && bytes.len() == pixels.len() => {
                Some(bytes.as_ref())
            }
            _ => None,
        });
        let equal = if let Some(expected) = expected {
            let mut equal = true;
            for (byte, saved_byte) in pixels.iter().copied().zip(expected.iter().copied()) {
                sha256.update([byte]);
                #[cfg(test)]
                DEPENDENCY_BYTE_VISITS.with(|visits| visits.set(visits.get() + 1));
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
                equal &= saved_byte == byte;
            }
            equal
        } else {
            for byte in pixels.iter().copied() {
                sha256.update([byte]);
                #[cfg(test)]
                DEPENDENCY_BYTE_VISITS.with(|visits| visits.set(visits.get() + 1));
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
            }
            false
        };
        (pixels.len(), equal)
    };
    (
        CacheablePresentDependency {
            start,
            src_stride,
            dst_width,
            dst_height,
            blanked,
            bytes,
            fnv_digest: hash,
            sha256: sha256.finalize().into(),
        },
        exact_hit,
    )
}

fn decoded_storage(
    rdram: &[u8],
    start: usize,
    src_stride: usize,
    dst_width: usize,
    dst_height: usize,
) -> &[u8] {
    assert!(
        start.is_multiple_of(4),
        "framebuffer start is not word-aligned"
    );
    let copy_width = dst_width.min(src_stride.max(1));
    let pixels = dst_height
        .saturating_sub(1)
        .checked_mul(src_stride.max(1))
        .and_then(|prefix| prefix.checked_add(copy_width))
        .expect("framebuffer dependency footprint overflow");
    let bytes = pixels
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(3))
        .map(|bytes| bytes & !3)
        .expect("framebuffer dependency byte count overflow");
    let available = rdram.len().saturating_sub(start);
    &rdram[start..start + bytes.min(available)]
}

/// Expand a 5-bit channel (0..=31) to 8-bit (0..=255) with rounding, the same
/// `(v*255+15)/31` expansion oot-boot uses -- so a byte-for-byte identical
/// image to the PNG dumps.
#[inline]
fn expand5(v: u16) -> u8 {
    ((v * 255 + 15) / 31) as u8
}

/// Convert one N64 RGBA5551 framebuffer region into `dst` as RGBA8888.
///
/// `dst` must be exactly `FB_WIDTH * FB_HEIGHT * 4` bytes (the size of
/// `pixels`' frame buffer for a 320x240 surface). `src` is the raw rdram
/// slice at the VI framebuffer offset; if it's shorter than [`FB_BYTES`]
/// (e.g. a truncated capture near an rdram bound), the missing pixels are
/// left black rather than reading out of bounds. Returns the number of
/// destination pixels actually written.
///
/// `src_stride` is the framebuffer's real line width in pixels (VI_WIDTH,
/// from `fn64_abi::vi_width`). The source advances by `src_stride` per row
/// while `dst` stays `dst_width`-wide, so a game whose framebuffer line width
/// differs from the presented 320 no longer shears/offsets each scanline.
///
/// `dst_height` is the guest's own active output line count
/// (`fn64_abi::vi_output_height`), NOT a fixed 240. Reading past it walks
/// into rows the game never rendered, which is visible as a band of stale or
/// uninitialized pixels along the bottom of the window.
pub fn rgba5551_to_rgba8888(
    rdram: RdramView<'_>,
    start: RdramAddr,
    src_stride: usize,
    dst_width: usize,
    dst_height: usize,
    dst: &mut [u8],
) -> usize {
    debug_assert_eq!(dst.len(), dst_width * dst_height * 4);
    assert!(
        start.offset().is_multiple_of(4),
        "RGBA5551 framebuffer base must be word-aligned, got {:#x}",
        start.offset()
    );
    let src_stride = src_stride.max(1);
    // The `^ 2` halfword swizzle in read_u16 touches the whole containing
    // word, so a trailing sub-word remnant is not a readable pixel. Count
    // pixels at word granularity, matching the previous `(available/4)*2`.
    let available_pixels = (rdram.len().saturating_sub(start.offset() as usize) / 4) * 2;
    // Present the full framebuffer line when the surface is sized to it
    // (`dst_width == src_stride`); if the surface is narrower, show the left
    // `dst_width` columns of each row, still at the correct per-row offset.
    let copy_width = dst_width.min(src_stride);
    let mut written = 0;
    for row in 0..dst_height {
        let row_first = row * src_stride;
        for col in 0..copy_width {
            let i = row_first + col;
            if i >= available_pixels {
                return written; // ran off the rdram-backed region
            }
            let byte_offset = u32::try_from(i * 2).expect("framebuffer byte offset exceeds u32");
            let addr = start
                .checked_add(byte_offset)
                .expect("framebuffer logical address overflow");
            let px = rdram.read_u16(addr);
            let o = (row * dst_width + col) * 4;
            dst[o..o + 4].copy_from_slice(&fn64_render::presented_rgba5551_to_rgba8888(px));
            written += 1;
        }
    }
    written
}

/// Copy the left `dst_width` columns of each complete source-field row into
/// the shell's tightly packed display buffer. This is the existing overscan
/// crop expressed over an already-decoded, generation-bound source field.
pub fn copy_presented_source_field(
    source: &fn64_render::PresentedSourceField,
    dst_width: usize,
    dst_height: usize,
    dst: &mut [u8],
) {
    let stride = source.stride_pixels() as usize;
    assert_eq!(dst_height, source.height() as usize);
    assert!(dst_width <= stride);
    assert_eq!(dst.len(), dst_width * dst_height * 4);
    for row in 0..dst_height {
        let source_start = row * stride * 4;
        let destination_start = row * dst_width * 4;
        dst[destination_start..destination_start + dst_width * 4]
            .copy_from_slice(&source.rgba8()[source_start..source_start + dst_width * 4]);
    }
}

/// True if every byte in `region` is identical -- a blank/uniform frame the
/// game hasn't rendered into yet. Mirrors oot-boot's `uniform` check so the
/// shell can report "blank" honestly instead of implying content.
pub fn is_uniform(region: &[u8]) -> bool {
    match region.first() {
        Some(&first) => region.iter().all(|&b| b == first),
        None => true,
    }
}

/// Replace a decoded framebuffer with opaque black VI output.
pub fn fill_opaque_black(rgba: &mut [u8]) {
    assert!(
        rgba.len().is_multiple_of(4),
        "RGBA buffer length is not pixel-aligned"
    );
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0, 0, 0, 255]);
    }
}

/// Stable diagnostic fingerprint of the decoded RGBA frame. Unlike the old
/// `NON-BLANK` label this makes two capture paths mechanically comparable;
/// it is evidence of equality, not a claim that either image is faithful.
pub fn rgba_hash(rgba: &[u8]) -> u64 {
    rgba.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_POLICY: PresentPolicy = PresentPolicy::new(1, false);

    fn blank_dst() -> Vec<u8> {
        vec![0u8; FB_WIDTH * FB_HEIGHT * 4]
    }

    #[test]
    fn presented_source_crop_preserves_rows_and_omits_only_right_overscan() {
        let mut words = [0_u32; fn64_render::ViScanoutRegisters::WORD_COUNT];
        words[0] = 2;
        words[1] = 0x1000;
        words[2] = 3;
        words[9] = 3;
        words[10] = 4;
        let presentation = fn64_render::ViPresentation {
            scanout: fn64_render::ViScanoutState::Registers(
                fn64_render::ViScanoutRegisters::from_words(words),
            ),
            ..Default::default()
        };
        let source = fn64_render::PresentedSourceField::rgba5551(
            presentation,
            0x1000,
            3,
            2,
            (0_u8..24).collect(),
        )
        .unwrap();
        let mut cropped = vec![0; 2 * 2 * 4];
        copy_presented_source_field(&source, 2, 2, &mut cropped);
        assert_eq!(
            cropped,
            [0, 1, 2, 3, 4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 18, 19]
        );
    }

    #[test]
    fn present_cache_mode_parses_only_the_documented_values() {
        assert_eq!(PresentCacheMode::default(), PresentCacheMode::Suppress);
        assert_eq!(
            PresentCacheMode::from_env_value(None),
            PresentCacheMode::Suppress
        );
        assert_eq!(
            PresentCacheMode::from_env_value(Some("0")),
            PresentCacheMode::Disabled
        );
        assert_eq!(
            PresentCacheMode::from_env_value(Some("observe")),
            PresentCacheMode::Observe
        );
        assert_eq!(
            PresentCacheMode::from_env_value(Some("1")),
            PresentCacheMode::Suppress
        );
    }

    #[test]
    #[should_panic(expected = "expected 0, observe, or 1")]
    fn present_cache_mode_rejects_ambiguous_values() {
        PresentCacheMode::from_env_value(Some("true"));
    }

    #[test]
    fn observe_records_exact_hits_without_suppressing_redraws() {
        let rdram = vec![0x5a; 64];
        let mut cache = PresentCache::default();
        cache.record_success(PresentDependency::capture(&rdram, 0, 8, 8, 2, false));

        let receipt = cache.probe(
            PresentCacheMode::Observe,
            TEST_POLICY,
            &rdram,
            0,
            8,
            8,
            2,
            false,
        );
        assert!(receipt.exact_hit);
        assert!(!receipt.suppress_redraw);
        let receipt = cache.probe(
            PresentCacheMode::Suppress,
            TEST_POLICY,
            &rdram,
            0,
            8,
            8,
            2,
            false,
        );
        assert!(receipt.exact_hit);
        assert!(receipt.suppress_redraw);
        assert!(matches!(
            receipt.dependency,
            PresentDependencyObservation::Cacheable(CacheablePresentDependency {
                start: 0,
                src_stride: 8,
                dst_width: 8,
                dst_height: 2,
                blanked: false,
                bytes: 32,
                ..
            })
        ));
        assert_eq!(cache.stats().hits, 2);
    }

    #[test]
    fn one_probe_traversal_computes_digest_and_exact_byte_hit() {
        let mut rdram = vec![0x5a; 64];
        let mut cache = PresentCache::default();
        cache.record_success(PresentDependency::capture(&rdram, 0, 8, 8, 2, false));
        DEPENDENCY_BYTE_VISITS.with(|visits| visits.set(0));

        let first = cache.probe(
            PresentCacheMode::Observe,
            TEST_POLICY,
            &rdram,
            0,
            8,
            8,
            2,
            false,
        );
        assert!(first.exact_hit);
        let PresentDependencyObservation::Cacheable(first_dependency) = first.dependency else {
            panic!("ordinary probe must be cacheable");
        };
        let mut expected_sha256 = Sha256::new();
        expected_sha256.update(b"fn64.present-dependency.v1\0");
        for value in [0_u64, 8, 8, 2] {
            expected_sha256.update(value.to_le_bytes());
        }
        expected_sha256.update([0]);
        expected_sha256.update(&rdram[..32]);
        assert_eq!(
            first_dependency.sha256,
            <[u8; 32]>::from(expected_sha256.finalize())
        );
        assert_eq!(DEPENDENCY_BYTE_VISITS.with(std::cell::Cell::get), 32);
        rdram[7] ^= 1;
        let second = cache.probe(
            PresentCacheMode::Observe,
            TEST_POLICY,
            &rdram,
            0,
            8,
            8,
            2,
            false,
        );
        assert!(!second.exact_hit);
        assert_ne!(first.dependency, second.dependency);
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(DEPENDENCY_BYTE_VISITS.with(std::cell::Cell::get), 64);
    }

    #[test]
    #[ignore = "release-only diagnostic; not a correctness or landing gate"]
    fn release_probe_core_chunked_sha_timing() {
        use std::hint::black_box;
        use std::time::Instant;

        const BYTES: usize = 227_520;
        const REPEATS: usize = 40;
        let pixels = (0..BYTES)
            .map(|index| (index as u8).wrapping_mul(37))
            .collect::<Vec<_>>();
        let expected = pixels.clone();

        let probe_core = |per_byte_sha: bool| {
            let mut sha256 = Sha256::new();
            let mut fnv = 0xcbf2_9ce4_8422_2325;
            let mut equal = true;
            if per_byte_sha {
                for (index, byte) in pixels.iter().copied().enumerate() {
                    sha256.update([byte]);
                    fnv = (fnv ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
                    equal &= expected[index] == byte;
                }
            } else {
                for (chunk, saved_chunk) in pixels.chunks(4 * 1024).zip(expected.chunks(4 * 1024)) {
                    sha256.update(chunk);
                    for (byte, saved_byte) in chunk.iter().copied().zip(saved_chunk.iter().copied())
                    {
                        fnv = (fnv ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
                        equal &= saved_byte == byte;
                    }
                }
            }
            (sha256.finalize(), fnv, equal)
        };

        assert_eq!(probe_core(true), probe_core(false));
        let mut legacy_samples = Vec::with_capacity(REPEATS);
        let mut chunked_samples = Vec::with_capacity(REPEATS);
        let measure = |per_byte_sha, samples: &mut Vec<u128>| {
            let started = Instant::now();
            black_box(probe_core(per_byte_sha));
            samples.push(started.elapsed().as_nanos());
        };
        for repeat in 0..REPEATS {
            if repeat.is_multiple_of(2) {
                measure(true, &mut legacy_samples);
                measure(false, &mut chunked_samples);
            } else {
                measure(false, &mut chunked_samples);
                measure(true, &mut legacy_samples);
            }
        }
        legacy_samples.sort_unstable();
        chunked_samples.sort_unstable();
        let legacy_ns = legacy_samples[REPEATS / 2];
        let chunked_ns = chunked_samples[REPEATS / 2];
        eprintln!(
            "227520-byte probe core: legacy_per_byte_sha_ns={legacy_ns} \
             chunked_sha_ns={chunked_ns}"
        );
    }

    #[test]
    fn uncacheable_reasons_are_typed_and_never_suppress() {
        let mut cache = PresentCache::default();
        let reasons = [
            UncacheablePresentReason::Overlay,
            UncacheablePresentReason::FrameTrip,
            UncacheablePresentReason::FrameDump,
            UncacheablePresentReason::MissingFramebuffer,
            UncacheablePresentReason::UnavailableFramebuffer,
            UncacheablePresentReason::OutsideRdram,
            UncacheablePresentReason::UnalignedFramebuffer,
        ];
        for reason in reasons {
            let receipt =
                cache.record_uncacheable_request(PresentCacheMode::Suppress, TEST_POLICY, reason);
            assert_eq!(
                receipt.dependency,
                PresentDependencyObservation::Uncacheable(reason)
            );
            assert!(!receipt.exact_hit);
            assert!(!receipt.suppress_redraw);
            assert!(!reason.name().is_empty());
        }
        assert_eq!(cache.stats().requests, reasons.len() as u64);
        assert_eq!(cache.stats().misses, reasons.len() as u64);
    }

    #[test]
    fn present_dependency_matches_only_exact_decoded_inputs() {
        let mut rdram = vec![0u8; 128];
        let saved = PresentDependency::capture(&rdram, 16, 8, 7, 3, false);
        assert!(saved.matches(&rdram, 16, 8, 7, 3, false));

        // Last decoded pixel is row 2, column 6. Its containing word belongs
        // to the dependency and must invalidate reuse.
        rdram[60] = 1;
        assert!(!saved.matches(&rdram, 16, 8, 7, 3, false));

        // Bytes after the final containing word are not read by the decoder.
        rdram[64] = 1;
        let saved = PresentDependency::capture(&rdram, 16, 8, 7, 3, false);
        rdram[100] = 1;
        assert!(saved.matches(&rdram, 16, 8, 7, 3, false));
        assert!(!saved.matches(&rdram, 16, 8, 6, 3, false));
        assert!(!saved.matches(&rdram, 20, 8, 7, 3, false));
    }

    #[test]
    fn blank_dependency_ignores_rdram_but_not_output_state() {
        let mut rdram = vec![0u8; 64];
        let saved = PresentDependency::capture(&rdram, 0, 8, 8, 2, true);
        rdram.fill(0xff);
        assert!(saved.matches(&rdram, 0, 8, 8, 2, true));
        assert!(!saved.matches(&rdram, 0, 8, 8, 2, false));
        assert!(!saved.matches(&rdram, 0, 8, 8, 3, true));
    }

    fn install_test_present(cache: &mut PresentCache, rdram: &[u8]) {
        cache.record_success(PresentDependency::capture(rdram, 0, 8, 8, 2, false));
    }

    #[test]
    fn overlay_close_generation_cannot_reuse_composited_frame() {
        let rdram = vec![0x5a; 64];
        let mut cache = PresentCache::default();
        install_test_present(&mut cache, &rdram);
        assert!(cache.is_current(&rdram, 0, 8, 8, 2, false));

        // Closing the overlay changes the window image but not these guest
        // bytes. The transition's generation bump must force one clean draw.
        cache.invalidate();
        assert!(!cache.is_current(&rdram, 0, 8, 8, 2, false));
    }

    #[test]
    fn video_policy_changes_invalidate_but_identical_observations_do_not() {
        let rdram = vec![0x5a; 64];
        let mut cache = PresentCache::default();
        cache.synchronize_policy(PresentPolicy::new(1, false));
        install_test_present(&mut cache, &rdram);

        cache.synchronize_policy(PresentPolicy::new(1, false));
        assert!(cache.is_current(&rdram, 0, 8, 8, 2, false));
        cache.synchronize_policy(PresentPolicy::new(1, true));
        assert!(!cache.is_current(&rdram, 0, 8, 8, 2, false));

        install_test_present(&mut cache, &rdram);
        cache.synchronize_policy(PresentPolicy::new(2, true));
        assert!(!cache.is_current(&rdram, 0, 8, 8, 2, false));
    }

    #[test]
    fn failed_submission_forces_retry_of_identical_dependency() {
        let rdram = vec![0x5a; 64];
        let mut cache = PresentCache::default();
        install_test_present(&mut cache, &rdram);
        assert!(cache.is_current(&rdram, 0, 8, 8, 2, false));

        cache.record_failure();
        assert!(!cache.is_current(&rdram, 0, 8, 8, 2, false));
        assert_eq!(cache.stats().failed_presents, 1);
    }

    #[test]
    fn cache_stats_keep_skipped_requests_in_the_denominator_and_digest() {
        let mut rdram = vec![0x5a; 64];
        let mut cache = PresentCache::default();
        install_test_present(&mut cache, &rdram);
        assert!(cache.is_current(&rdram, 0, 8, 8, 2, false));
        let first = cache.stats();
        assert_eq!(first.requests, 1);
        assert_eq!(first.hits, 1);
        assert_eq!(first.misses, 0);
        assert_eq!(first.dependency_samples, 1);
        assert_eq!(first.dependency_bytes, 32);

        rdram[0] ^= 1;
        assert!(!cache.is_current(&rdram, 0, 8, 8, 2, false));
        cache.record_uncacheable_request(
            PresentCacheMode::Observe,
            TEST_POLICY,
            UncacheablePresentReason::Overlay,
        );
        let final_stats = cache.stats();
        assert_eq!(final_stats.requests, 3);
        assert_eq!(final_stats.hits, 1);
        assert_eq!(final_stats.misses, 2);
        assert_eq!(final_stats.dependency_samples, 2);
        assert_eq!(final_stats.dependency_bytes, 64);
        assert_ne!(first.logical_digest, final_stats.logical_digest);
    }

    #[test]
    fn blank_logical_sample_hashes_policy_key_without_rdram_bytes() {
        let mut rdram = vec![0; 64];
        let mut cache = PresentCache::default();
        cache.record_success(PresentDependency::capture(&rdram, 0, 8, 8, 2, true));
        assert!(cache.is_current(&rdram, 0, 8, 8, 2, true));
        let first = cache.stats();
        rdram.fill(0xff);
        assert!(cache.is_current(&rdram, 0, 8, 8, 2, true));
        let second = cache.stats();
        assert_eq!(second.dependency_samples, 2);
        assert_eq!(second.dependency_bytes, 0);
        assert_ne!(first.logical_digest, 0);
        assert_ne!(first.logical_digest, second.logical_digest);
    }

    /// Build a word-aligned framebuffer holding `px` values at pixels 0..n,
    /// in fn64's native-word storage: pixel i at byte `(2*i) ^ 2`, native-endian.
    fn fb_with(pixels_in: &[u16]) -> Vec<u8> {
        let words = pixels_in.len().div_ceil(2);
        let mut buf = vec![0u8; words * 4];
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut buf);
        for (i, px) in pixels_in.iter().enumerate() {
            view.write_u16(RdramAddr::from_offset((i * 2) as u32), *px);
        }
        buf
    }

    fn decode(src: &[u8], dst: &mut [u8]) -> usize {
        // Default helper: stride == dst_width == FB_WIDTH (the 320 case).
        rgba5551_to_rgba8888(
            RdramView::from_storage(src),
            RdramAddr::from_offset(0),
            FB_WIDTH,
            FB_WIDTH,
            FB_HEIGHT,
            dst,
        )
    }

    #[test]
    fn pure_red_pixel_unpacks_to_ff0000ff() {
        // RGBA5551 pure red: R=31,G=0,B=0,A=1 -> 0b11111_00000_00000_1 = 0xF801.
        let src = fb_with(&[0xF801]);
        let mut dst = blank_dst();
        assert!(decode(&src, &mut dst) >= 1);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn pure_green_and_blue() {
        // Green: G=31 -> 0x07C0. Blue: B=31 -> 0x003E.
        let mut dst = blank_dst();
        decode(&fb_with(&[0x07C0]), &mut dst);
        assert_eq!(&dst[0..4], &[0, 255, 0, 255]);
        let mut dst = blank_dst();
        decode(&fb_with(&[0x003E]), &mut dst);
        assert_eq!(&dst[0..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn word_swizzle_pairs_pixels_correctly() {
        // Regression for the green-tinted/noisy N64 logo: pixels 0 and 1
        // (red then blue) share one word with their halfwords SWAPPED in
        // storage. A flat big-endian walk decodes them in the wrong order
        // and with scrambled fields; the `^ 2` native read must yield
        // red at pixel 0 and blue at pixel 1.
        let src = fb_with(&[0xF801, 0x003E]);
        let mut dst = blank_dst();
        assert!(decode(&src, &mut dst) >= 2);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255], "pixel 0 must decode red");
        assert_eq!(&dst[4..8], &[0, 0, 255, 255], "pixel 1 must decode blue");
    }

    #[test]
    fn alpha_always_opaque() {
        // Even a pixel with the 1-bit alpha clear presents opaque.
        let mut dst = blank_dst();
        decode(&fb_with(&[0xF800]), &mut dst); // red, A=0
        assert_eq!(dst[3], 255);
    }

    #[test]
    fn full_width_surface_presents_the_whole_line_at_its_real_stride() {
        // WM2000's case: stride == dst_width == 480. Row 1 begins at source
        // pixel `stride`, and the surface is wide enough to show the whole
        // line (no crop). Confirm both rows land fully and at the right offset.
        let width = 480usize;
        let mut src_px = vec![0u16; width * 2];
        src_px[0] = 0xF801; // row 0, col 0 = red
        src_px[width - 1] = 0x07C0; // row 0, last col = green (only visible if full width shown)
        src_px[width] = 0x003E; // row 1, col 0 = blue (at the real stride)
        let src = fb_with(&src_px);
        let mut dst = vec![0u8; width * FB_HEIGHT * 4];
        rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            width, // src_stride
            width, // dst_width (surface sized to the real width)
            FB_HEIGHT,
            &mut dst,
        );
        assert_eq!(&dst[0..4], &[255, 0, 0, 255], "row 0 col 0 red");
        let last = (width - 1) * 4;
        assert_eq!(
            &dst[last..last + 4],
            &[0, 255, 0, 255],
            "row 0 last col green -- full width presented, not cropped to 320"
        );
        let row1 = width * 4;
        assert_eq!(
            &dst[row1..row1 + 4],
            &[0, 0, 255, 255],
            "row 1 col 0 blue -- read from offset `stride`, no shear"
        );
    }

    #[test]
    fn cropping_the_overscan_column_leaves_the_kept_columns_identical() {
        // The VI-overscan crop: the guest fills a 480-wide line but the
        // scanout only addresses columns 0..479 (visible width 479), so the
        // presenter passes dst_width = 479 while the stride stays 480. The
        // kept columns must be byte-for-byte the same as a full-width present,
        // and the stale overscan column must never be read into the surface.
        const STRIDE: usize = 480;
        const VISIBLE: usize = 479;
        const ROWS: usize = 4;

        // Fill the whole framebuffer with a per-column shade, then poison the
        // last column of every row with a distinct "stale RDRAM" value so its
        // absence in the cropped surface is a real assertion.
        let stale = 0x0843u16;
        let mut src_px = vec![0u16; STRIDE * ROWS];
        for row in 0..ROWS {
            for col in 0..STRIDE {
                let shade = ((col as u16) & 0x1F) << 11 | 1;
                src_px[row * STRIDE + col] = shade;
            }
            src_px[row * STRIDE + (STRIDE - 1)] = stale; // overscan column
        }
        let src = fb_with(&src_px);

        let mut full = vec![0u8; STRIDE * ROWS * 4];
        rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            STRIDE,
            STRIDE, // full-width present (the pre-fix behavior)
            ROWS,
            &mut full,
        );

        let mut cropped = vec![0u8; VISIBLE * ROWS * 4];
        rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            STRIDE,  // stride unchanged -- row offsets stay correct
            VISIBLE, // present only the scanned-out columns
            ROWS,
            &mut cropped,
        );

        // Every kept column of every row is identical to the full present.
        for row in 0..ROWS {
            let full_row = &full[row * STRIDE * 4..row * STRIDE * 4 + VISIBLE * 4];
            let cropped_row = &cropped[row * VISIBLE * 4..(row + 1) * VISIBLE * 4];
            assert_eq!(
                cropped_row, full_row,
                "row {row}: cols 0..{VISIBLE} must be pixel-identical to the full present"
            );
        }

        // The stale overscan value expands to a distinct RGBA; it must appear
        // in the full present (proving the fixture exercises it) and never in
        // the cropped surface.
        let stale_rgba = [
            expand5((stale >> 11) & 0x1F),
            expand5((stale >> 6) & 0x1F),
            expand5((stale >> 1) & 0x1F),
            255,
        ];
        assert!(
            full.chunks_exact(4).any(|px| px == stale_rgba),
            "the full present shows the overscan column (fixture is meaningful)"
        );
        assert!(
            !cropped.chunks_exact(4).any(|px| px == stale_rgba),
            "the cropped present must never read the overscan column"
        );
    }

    #[test]
    fn narrow_surface_crops_but_keeps_row_alignment() {
        // If the surface stays 320 while the source is wider, each row still
        // reads from its real stride (no shear), just cropped to the left 320.
        let stride = 480usize;
        let mut src_px = vec![0u16; stride * 2];
        src_px[0] = 0xF801; // row 0 red
        src_px[stride] = 0x003E; // row 1 blue at real stride
        src_px[320] = 0x07C0; // beyond the 320 crop -- must not appear
        let src = fb_with(&src_px);
        let mut dst = vec![0u8; FB_WIDTH * FB_HEIGHT * 4];
        rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            stride,
            FB_WIDTH, // narrow surface
            FB_HEIGHT,
            &mut dst,
        );
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
        let row1 = FB_WIDTH * 4;
        assert_eq!(&dst[row1..row1 + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn truncated_source_leaves_rest_black_no_panic() {
        // One word of source into a full-frame dst: pixels 0-1 set, rest
        // untouched; a sub-word remnant is skipped, never read OOB.
        let mut dst = vec![7u8; FB_WIDTH * FB_HEIGHT * 4];
        let n = decode(&fb_with(&[0xF801, 0xF801]), &mut dst);
        assert_eq!(n, 2);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
        assert_eq!(dst[8], 7);
        // 2-byte remnant (half a word): zero pixels, no panic.
        assert_eq!(decode(&[0xF8, 0x01], &mut dst), 0);
    }

    /// The bug this pins: the presenter used to read a fixed `FB_HEIGHT`
    /// (240) rows regardless of what the guest programmed, so a game whose
    /// VI active window is shorter had rows of never-rendered RDRAM blitted
    /// into the window as an edge band.
    ///
    /// WM2000's measured V_START is `0x002501ff` -- half-lines 37..511, i.e.
    /// **237** output lines (the same decode `fn64_render::ViActiveWindow`
    /// asserts). The fixture below fills 237 rows with a known value and the
    /// three rows past them with a *different* known value, so reading one
    /// row too many is a visible wrong answer rather than a silent one.
    #[test]
    fn conversion_stops_at_the_programmed_output_height() {
        const STRIDE: usize = 480;
        const OUT_H: usize = 237; // WM2000's V_START decode
        const PAST: usize = 3; // rows the guest never rendered

        // In-image pixels are white; the rows past the active window hold a
        // distinct "stale RDRAM" value that must never be presented.
        let in_image = 0xFFFFu16;
        let stale = 0x0843u16;
        let mut src_px = vec![in_image; STRIDE * (OUT_H + PAST)];
        for px in src_px.iter_mut().skip(STRIDE * OUT_H) {
            *px = stale;
        }
        let src = fb_with(&src_px);

        let mut dst = vec![0u8; STRIDE * OUT_H * 4];
        let written = rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            STRIDE,
            STRIDE,
            OUT_H,
            &mut dst,
        );
        assert_eq!(
            written,
            STRIDE * OUT_H,
            "every pixel of the programmed rectangle must be written"
        );

        // Positive control: the fixture really does exercise the last row.
        // Without this a test that read 236 rows would also pass.
        let last_row = (OUT_H - 1) * STRIDE * 4;
        assert_eq!(
            &dst[last_row..last_row + 4],
            &[255, 255, 255, 255],
            "row {} (the last programmed line) must be present and in-image",
            OUT_H - 1
        );

        // The stale value expands to a distinct RGBA, so its absence is a
        // real assertion rather than a coincidence of two identical colors.
        let stale_rgba = [
            expand5((stale >> 11) & 0x1F),
            expand5((stale >> 6) & 0x1F),
            expand5((stale >> 1) & 0x1F),
            255,
        ];
        assert_ne!(
            stale_rgba,
            [255, 255, 255, 255],
            "fixture is only meaningful if the stale rows differ from the image"
        );
        assert!(
            !dst.chunks_exact(4).any(|px| px == stale_rgba),
            "no pixel past the programmed {OUT_H}-line output rectangle may be presented"
        );
    }

    /// The other half of the same defect: the presenter must read from
    /// VI_ORIGIN, and a base one row low shifts the whole image and drags a
    /// never-rendered row in at the bottom. This pins the arithmetic that
    /// makes the two bases differ by exactly one line.
    #[test]
    fn a_base_one_row_low_shifts_every_presented_row() {
        const STRIDE: usize = 480;
        const OUT_H: usize = 8;

        // Row r is filled with the value r+1, so a one-row shift is visible
        // as an off-by-one in the decoded red channel rather than as noise.
        let mut src_px = vec![0u16; STRIDE * (OUT_H + 1)];
        for row in 0..=OUT_H {
            let shade = (row as u16 + 1) & 0x1F;
            for col in 0..STRIDE {
                src_px[row * STRIDE + col] = (shade << 11) | 1;
            }
        }
        let src = fb_with(&src_px);

        let read_at = |start_pixels: usize| {
            let mut dst = vec![0u8; STRIDE * OUT_H * 4];
            rgba5551_to_rgba8888(
                RdramView::from_storage(&src),
                RdramAddr::from_offset((start_pixels * 2) as u32),
                STRIDE,
                STRIDE,
                OUT_H,
                &mut dst,
            );
            dst
        };

        let correct = read_at(STRIDE); // VI_ORIGIN: one row into the buffer
        let one_row_low = read_at(0); // the buffer base the RDP renders to

        assert_eq!(
            correct[0],
            expand5(2),
            "reading from VI_ORIGIN must start at the second stored row"
        );
        assert_eq!(
            one_row_low[0],
            expand5(1),
            "reading from the render base starts one row earlier"
        );
        assert_ne!(
            correct, one_row_low,
            "a one-row base error must change the presented image"
        );
    }

    #[test]
    fn uniform_detects_blank() {
        assert!(is_uniform(&[0, 0, 0, 0]));
        assert!(is_uniform(&[]));
        assert!(!is_uniform(&[0, 0, 1, 0]));
    }

    #[test]
    fn blank_vi_output_is_opaque_black() {
        let mut rgba = vec![0x55; 3 * 4];
        fill_opaque_black(&mut rgba);
        assert_eq!(rgba, [0, 0, 0, 255].repeat(3));
    }

    #[test]
    fn rgba_hash_is_stable_and_content_sensitive() {
        assert_eq!(rgba_hash(&[]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(rgba_hash(&[0, 1, 2, 3]), 0x4475_327f_98e0_5411);
        assert_ne!(rgba_hash(&[0, 1, 2, 3]), rgba_hash(&[0, 1, 3, 2]));
    }
}
