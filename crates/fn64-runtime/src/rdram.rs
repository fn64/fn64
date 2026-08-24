//! The rdram buffer, its `MEM_*`-equivalent accessors, and the `RdramAddr`
//! translation newtype. See `docs/DESIGN.md` section 3.
//!
//! Semantics (byte-lane XOR, sign extension, direct-segment RDRAM aliasing,
//! and sparse non-RDRAM base subtraction) are
//! transcribed from `aki-recomp/runtime/ABI-SURFACE.md` section (c), which
//! mechanically extracted them from N64Recomp-generated C (MIT-licensed
//! recompiler output; no vendor runtime implementation was read).
//!
//! ## Correction (byte order): `MEM_*` word/halfword accessors are NATIVE-
//! ## endian, not big-endian
//!
//! A previous wave's transcription of `ABI-SURFACE.md` section (c) as "no
//! byte-lane XOR... sign-extended" for `MEM_W`, and the analogous claim for
//! `MEM_H`/`MEM_HU` after their XOR, was WRONG about byte ORDER (the XOR
//! offset math itself was correct). First caught by
//! `examples/wm2000-boot`'s actual boot run: a spawned thread's real stack
//! pointer, read through what was then `Rdram`-equivalent logic, came back
//! exactly byte-swapped (`0x70BE0480` instead of the real `0x8004BE70`).
//! Verified directly against `recomp.h` (MIT, the ABI this crate serves):
//! ```text
//! #define MEM_W(offset, reg) \
//!     (*(int32_t*)(rdram + ((((reg) + (offset))) - 0xFFFFFFFF80000000)))
//! #define MEM_H(offset, reg) \
//!     (*(int16_t*)(rdram + ((((reg) + (offset)) ^ 2) - 0xFFFFFFFF80000000)))
//! ```
//! Both are PLAIN C POINTER DEREFERENCES -- native-endian loads/stores on
//! whatever host compiles this code (little-endian, for every desktop
//! target fn64 ships on). The `^2`/`^3` byte-lane XOR on the sub-word
//! accessors exists PRECISELY BECAUSE the underlying word storage is
//! native-endian: XORing the sub-word offset is what makes a big-endian-CPU
//! address land on the correct byte within an otherwise little-endian-
//! stored word. If `MEM_W` itself were big-endian, the XOR trick would be
//! unnecessary (a real big-endian backing store needs no lane correction at
//! all). Every accessor below now uses `from_ne_bytes`/`to_ne_bytes`
//! (native), not `from_be_bytes`/`to_be_bytes`; single-byte accessors
//! (`read_b`/`write_b`/`read_bu`/`write_bu`) were already correct (no
//! multi-byte order question at 1-byte granularity).

#[cfg(not(target_endian = "little"))]
compile_error!(
    "fn64's N64Recomp ABI storage contract requires a little-endian host: MEM_W is native-endian while MEM_H/MEM_B use ^2/^3 lane mapping"
);

/// Default N64 RDRAM capacity (8 MB, the common console configuration both
/// ported games in `aki-recomp` target). A future multi-console config
/// point, not a magic constant scattered through call sites.
pub const DEFAULT_RDRAM_SIZE: usize = 8 * 1024 * 1024;

/// A call-scoped, read-only capability for the console's complete physical
/// RDRAM device.
///
/// Unlike [`RdramView`], this type does not manufacture a shared Rust slice.
/// The typed recompiler can retain its checked `&mut [u8]`-backed RDRAM view
/// while its coroutine is suspended at a device boundary, so host devices
/// must use the already-registered raw allocation without creating a
/// competing reference. The higher-ranked constructor below prevents safe
/// consumers from retaining this capability after the device call returns.
///
/// Logical reads still use the one N64Recomp native-word lane mapping owned by
/// this module. The capability is deliberately neither `Clone`, `Send`, nor
/// `Sync`: one exact executor boundary owns it.
pub struct PhysicalRdramRead<'call> {
    storage: std::ptr::NonNull<u8>,
    _call: std::marker::PhantomData<&'call [u8]>,
    _not_send_or_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl<'call> PhysicalRdramRead<'call> {
    /// Build a physical-device capability from an ordinary exact-size storage
    /// borrow. Tests and standalone embedders use this path; integrated
    /// execution uses [`with_physical_rdram_read`] instead.
    pub fn from_storage(storage: &'call [u8]) -> Self {
        assert_eq!(
            storage.len(),
            DEFAULT_RDRAM_SIZE,
            "physical RDRAM read storage must be exactly {DEFAULT_RDRAM_SIZE:#x} bytes, got {:#x}",
            storage.len()
        );
        let storage = std::ptr::NonNull::new(storage.as_ptr().cast_mut())
            .expect("physical RDRAM read storage must not be null");
        Self {
            storage,
            _call: std::marker::PhantomData,
            _not_send_or_sync: std::marker::PhantomData,
        }
    }

    pub const fn len(&self) -> usize {
        DEFAULT_RDRAM_SIZE
    }

    pub const fn is_empty(&self) -> bool {
        false
    }

    fn range(&self, addr: RdramAddr, width: usize, lane_xor: usize, op: &str) -> usize {
        let logical = addr.offset() as usize;
        let start = logical ^ lane_xor;
        let end = start.checked_add(width).unwrap_or_else(|| {
            panic!("{op}: physical RDRAM address overflow at logical offset {logical:#x}")
        });
        assert!(
            end <= DEFAULT_RDRAM_SIZE,
            "{op}: logical physical-RDRAM range {logical:#x}..{:#x} maps outside {DEFAULT_RDRAM_SIZE} bytes",
            logical.saturating_add(width)
        );
        start
    }

    pub fn read_u32(&self, addr: RdramAddr) -> u32 {
        assert!(
            addr.offset().is_multiple_of(4),
            "physical RDRAM u32 read at unaligned logical address {:#x}",
            addr.offset()
        );
        let start = self.range(addr, 4, 0, "physical read_u32");
        // SAFETY: construction proves the complete physical device remains
        // live for this call, and `range` proves the native word is in it.
        unsafe { (self.storage.as_ptr().add(start) as *const u32).read_unaligned() }
    }

    pub fn read_u16(&self, addr: RdramAddr) -> u16 {
        assert!(
            addr.offset().is_multiple_of(2),
            "physical RDRAM u16 read at unaligned logical address {:#x}",
            addr.offset()
        );
        let start = self.range(addr, 2, 2, "physical read_u16");
        // SAFETY: construction proves the complete physical device remains
        // live for this call, and `range` proves the native halfword is in it.
        unsafe { (self.storage.as_ptr().add(start) as *const u16).read_unaligned() }
    }

    pub fn read_u8(&self, addr: RdramAddr) -> u8 {
        let start = self.range(addr, 1, 3, "physical read_u8");
        // SAFETY: construction proves the complete physical device remains
        // live for this call, and `range` proves the byte is in it.
        unsafe { *self.storage.as_ptr().add(start) }
    }

    /// Expose the quarantined pointer for a synchronous foreign renderer.
    ///
    /// # Safety
    /// The callee must treat the allocation as read-only, must not retain or
    /// access the pointer after the enclosing presentation call returns, and
    /// must complete all worker access before returning to Rust.
    pub unsafe fn as_mut_ptr(&self) -> *mut u8 {
        self.storage.as_ptr()
    }
}

/// Invoke one device operation with a bounded capability over the registered
/// process RDRAM allocation, without constructing a competing Rust slice.
///
/// # Safety
/// `storage` must identify an allocation of at least `allocation_len` live
/// bytes. No other active execution may access that allocation until `use_it`
/// returns. The process/executor contract supplies that exclusion only while
/// a guest coroutine is suspended at a host device boundary.
pub unsafe fn with_physical_rdram_read<R>(
    storage: *mut u8,
    allocation_len: usize,
    use_it: impl for<'call> FnOnce(PhysicalRdramRead<'call>) -> R,
) -> R {
    assert!(
        allocation_len >= DEFAULT_RDRAM_SIZE,
        "registered RDRAM allocation length {allocation_len:#x} does not cover the required {DEFAULT_RDRAM_SIZE:#x}-byte physical device"
    );
    let storage = std::ptr::NonNull::new(storage)
        .expect("registered physical RDRAM storage pointer must not be null");
    use_it(PhysicalRdramRead {
        storage,
        _call: std::marker::PhantomData,
        _not_send_or_sync: std::marker::PhantomData,
    })
}

/// The KSEG0 base subtracted by every generated `MEM_*` macro. Per
/// ABI-SURFACE.md section (c): "The subtraction of 0xFFFFFFFF80000000 (not
/// 0x80000000) is deliberate 64-bit-safe translation math... correctly
/// cancels sign extension for both a plain unsigned 32-bit vram value...
/// and its 64-bit-sign-extended form, landing on the same rdram-relative
/// byte offset either way."
const KSEG0_BASE_SIGN_EXTENDED: u64 = 0xFFFF_FFFF_8000_0000;

/// An N64 vram/KSEG0 address as MIPS code computes it, already translated
/// to an rdram-relative byte offset. See `docs/DESIGN.md` section 3's
/// `RdramAddr` writeup for why this must be a newtype rather than a bare
/// `u32`/`u64` passed around ad hoc: the KSEG0-base translation is easy to
/// get subtly wrong for exactly half of its inputs (a 64-bit
/// sign-extended `gpr` vs. a plain 32-bit vram value) if hand-rolled at a
/// second call site.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct RdramAddr(u32);

impl RdramAddr {
    /// Construct directly from an already-resolved rdram-relative byte
    /// offset (e.g. a constant known to already be in this form).
    pub const fn from_offset(offset: u32) -> Self {
        RdramAddr(offset)
    }

    /// Construct from a MIPS `gpr` (per ABI-SURFACE.md section (b), a
    /// `recomp_context` register field is `uint64_t`, and generated code
    /// may carry a sign-extended 64-bit KSEG0 address in it). Replicates
    /// the generated `MEM_*` macros' own base-subtraction math exactly
    /// (section (c)) so this is correct for both a plain 32-bit vram value
    /// and its 64-bit sign-extended form. KSEG0 and KSEG1 addresses inside
    /// physical RDRAM share their low-29-bit offset; sparse MMIO/non-RDRAM
    /// windows retain the generated macro's historical subtraction.
    pub fn from_gpr(reg: u64) -> Self {
        let upper = reg >> 32;
        let low = reg as u32;
        let physical = low & 0x1fff_ffff;
        let direct_rdram =
            (0x8000_0000..0xc000_0000).contains(&low) && physical < DEFAULT_RDRAM_SIZE as u32;
        if direct_rdram {
            assert!(
                upper == 0 || upper == u32::MAX as u64,
                "RdramAddr::from_gpr: noncanonical 64-bit direct RDRAM address {reg:#018x}"
            );
            return RdramAddr(physical);
        }
        RdramAddr(reg.wrapping_sub(KSEG0_BASE_SIGN_EXTENDED) as u32)
    }

    pub const fn offset(self) -> u32 {
        self.0
    }

    /// The KSEG0 virtual address a MIPS `lw`/`sw` would use to reach this
    /// rdram offset -- the inverse of the `MEM_*` base subtraction for a
    /// cached (KSEG0) RDRAM address. Needed when a shim must RETURN a
    /// pointer that guest code then compares against another pointer it
    /// already holds in KSEG0 form (e.g. `osViGetCurrentFramebuffer`, whose
    /// result `Sched_HandleRetrace` compares `==` against
    /// `pendingSwapBuf1->swapBuffer`, a KSEG0 `gfxCtx->curFrameBuffer`):
    /// returning the bare physical `offset()` there would never compare
    /// equal to the game's own KSEG0 framebuffer pointer, stalling the
    /// framebuffer-swap chain. Truncated to 32 bits (the width a `void*`
    /// return lands in `$v0` as), matching how the game stores/compares it.
    pub const fn to_kseg0(self) -> u32 {
        self.0 | 0x8000_0000
    }

    /// Advance a logical guest byte address without losing its RDRAM-domain
    /// type. Host adapters should use this instead of converting back to a
    /// bare integer and hand-applying the `^2`/`^3` storage mapping.
    pub const fn checked_add(self, bytes: u32) -> Option<Self> {
        match self.0.checked_add(bytes) {
            Some(offset) => Some(Self(offset)),
            None => None,
        }
    }
}

/// Read-only view of fn64's native-word RDRAM storage.
///
/// The slice is the ABI-visible storage passed to generated code; addresses
/// accepted by this type are logical guest byte addresses. Keeping that
/// distinction in the type is the mechanism that prevents host adapters from
/// open-coding a fourth variant of the lane mapping.
#[derive(Clone, Copy)]
pub struct RdramView<'a> {
    storage: &'a [u8],
}

impl<'a> RdramView<'a> {
    pub const fn from_storage(storage: &'a [u8]) -> Self {
        Self { storage }
    }

    pub const fn len(self) -> usize {
        self.storage.len()
    }

    pub const fn is_empty(self) -> bool {
        self.storage.is_empty()
    }

    fn range(
        self,
        addr: RdramAddr,
        width: usize,
        lane_xor: usize,
        op: &str,
    ) -> std::ops::Range<usize> {
        let logical = addr.offset() as usize;
        let start = logical ^ lane_xor;
        let end = start.checked_add(width).unwrap_or_else(|| {
            panic!("{op}: RDRAM address overflow at logical offset {logical:#x}")
        });
        assert!(
            end <= self.storage.len(),
            "{op}: logical RDRAM range {logical:#x}..{:#x} maps outside {} storage bytes",
            logical.saturating_add(width),
            self.storage.len()
        );
        start..end
    }

    pub fn read_u32(self, addr: RdramAddr) -> u32 {
        assert!(
            addr.offset().is_multiple_of(4),
            "RDRAM u32 read at unaligned logical address {:#x}",
            addr.offset()
        );
        u32::from_ne_bytes(
            self.storage[self.range(addr, 4, 0, "read_u32")]
                .try_into()
                .unwrap(),
        )
    }

    pub fn read_i32(self, addr: RdramAddr) -> i32 {
        self.read_u32(addr) as i32
    }

    pub fn read_u16(self, addr: RdramAddr) -> u16 {
        assert!(
            addr.offset().is_multiple_of(2),
            "RDRAM u16 read at unaligned logical address {:#x}",
            addr.offset()
        );
        u16::from_ne_bytes(
            self.storage[self.range(addr, 2, 2, "read_u16")]
                .try_into()
                .unwrap(),
        )
    }

    pub fn read_i16(self, addr: RdramAddr) -> i16 {
        self.read_u16(addr) as i16
    }

    pub fn read_u8(self, addr: RdramAddr) -> u8 {
        self.storage[self.range(addr, 1, 3, "read_u8").start]
    }

    pub fn read_i8(self, addr: RdramAddr) -> i8 {
        self.read_u8(addr) as i8
    }

    /// Borrow a word-aligned span of raw native-word storage, if fully mapped.
    ///
    /// Storage order, NOT logical order -- the caller is responsible for the
    /// `^3` lane mapping. Exposed for the one caller that legitimately wants
    /// the un-swizzled bytes: the mutation guard's baseline comparison, which
    /// holds its baseline pre-reversed and so can decide "unchanged" with a
    /// single `memcmp` instead of copying and reversing 1 MiB per dispatch.
    ///
    /// Deliberately narrow: it hands out a shared slice and cannot write, so
    /// it does not reopen the lane mapping to reimplementation the way a
    /// mutable or address-taking accessor would. Callers converting storage
    /// bytes to guest values must still go through the typed readers.
    pub fn storage_slice(self, start: usize, len: usize) -> Option<&'a [u8]> {
        assert!(
            start % 4 == 0 && len % 4 == 0,
            "storage_slice requires word-aligned bounds, got {start:#x}+{len:#x}"
        );
        self.storage.get(start..start.checked_add(len)?)
    }

    /// Copy a device/struct byte sequence out in logical guest order.
    /// Copy a logical byte range out, one native word at a time.
    ///
    /// The obvious implementation calls [`Self::read_u8`] per byte, and that is
    /// what this did. Each such call re-runs the bounds check and applies the
    /// lane XOR individually, so copying the 1 MiB executable region the
    /// mutation guard watches cost roughly a million checked reads -- 21.6 ms
    /// per executor step, measured, which dominated every WM2000 run.
    ///
    /// Storage holds native words, and logical byte `n` lives at storage index
    /// `n ^ 3` (`read_u8`'s lane XOR). Within one aligned word that is exactly
    /// a byte reversal, so an aligned word can be copied with a single
    /// bounds-checked read plus a reverse -- amortizing the check over four
    /// bytes instead of paying it per byte.
    ///
    /// The unaligned head and tail stay on the per-byte path: they are at most
    /// three bytes each, and getting them subtly wrong would corrupt guest
    /// memory in a way the word path would hide.
    pub fn copy_logical_bytes(self, addr: RdramAddr, out: &mut [u8]) {
        let len = u32::try_from(out.len()).expect("logical RDRAM copy length exceeds u32");
        if len == 0 {
            return;
        }
        // Prove every byte is mapped before any fast-path read, so a partial
        // copy cannot precede the panic an out-of-range copy owes.
        //
        // Checked per byte rather than over the whole span on purpose: the
        // per-byte path this replaced failed at the FIRST unmapped byte and
        // named it, and a caller diagnosing a bad copy wants that byte, not
        // the range it happened to sit in. `lle_debug_task_data_loudly_
        // rejects_an_unmapped_native_word_lane` asserts on exactly that
        // message.
        // Fast check over the whole span. A per-byte loop here would undo the
        // very cost this function exists to remove.
        let end = addr
            .offset()
            .checked_add(len)
            .expect("logical RDRAM copy overflow");
        if usize::try_from(end).expect("logical RDRAM end exceeds host indexing")
            > self.storage.len()
        {
            // Slow path only on the way to a panic: re-walk per byte so the
            // message names the FIRST unmapped byte, which is what the
            // per-byte implementation reported and what a caller diagnosing a
            // bad copy actually wants. `lle_debug_task_data_loudly_rejects_an
            // _unmapped_native_word_lane` asserts on exactly that message.
            for index in 0..len {
                let _ = self.range(
                    addr.checked_add(index)
                        .expect("logical RDRAM copy address overflow"),
                    1,
                    3,
                    "read_u8",
                );
            }
        }

        let start = addr.offset();
        let head = (4 - (start % 4)) % 4;
        let head = head.min(len);
        let body = (len - head) & !3;

        let copy_byte = |index: u32, byte: &mut u8| {
            *byte = self.read_u8(
                addr.checked_add(index)
                    .expect("logical RDRAM copy address overflow"),
            );
        };
        let head_index =
            usize::try_from(head).expect("logical RDRAM head length exceeds host indexing");
        for (index, byte) in out.iter_mut().enumerate().take(head_index) {
            copy_byte(
                u32::try_from(index).expect("logical RDRAM head index exceeds u32"),
                byte,
            );
        }
        // Bulk-copy the word-aligned body, then reverse each word in place.
        //
        // The previous form called `read_u32` per word and stored four bytes
        // individually -- 262,144 iterations for a 1 MiB watched region, on a
        // path that runs at every dispatch boundary. It profiled as the single
        // largest self-time cost in the certified lane (1,849 samples, ahead
        // of even the SHA-256).
        //
        // `read_u32` on an in-range aligned offset is a plain native-word
        // load, so the whole body is one contiguous slice; copying it and then
        // swapping each word gives byte-for-byte identical output, because
        // `native[3], native[2], native[1], native[0]` IS a 4-byte reverse.
        if body > 0 {
            let body_start = usize::try_from(start + head)
                .expect("logical RDRAM body start exceeds host indexing");
            let at = head_index;
            let bytes =
                usize::try_from(body).expect("logical RDRAM body length exceeds host indexing");
            out[at..at + bytes].copy_from_slice(&self.storage[body_start..body_start + bytes]);
            for word in out[at..at + bytes].chunks_exact_mut(4) {
                word.reverse();
            }
        }
        for index in (head + body)..len {
            let byte = &mut out
                [usize::try_from(index).expect("logical RDRAM tail index exceeds host indexing")];
            copy_byte(index, byte);
        }
    }

    /// Allocate and copy one guest-sized logical byte range.
    ///
    /// Guest address arithmetic stays in `u32`; the conversion required by
    /// the host allocator is confined to this memory-boundary method.
    pub fn read_logical_bytes(self, addr: RdramAddr, len: u32) -> Vec<u8> {
        let mut bytes =
            vec![0; usize::try_from(len).expect("logical RDRAM copy length exceeds host indexing")];
        self.copy_logical_bytes(addr, &mut bytes);
        bytes
    }
}

/// Mutable counterpart to [`RdramView`]. All writes accept logical guest
/// addresses and perform the one canonical native-word storage mapping.
pub struct RdramViewMut<'a> {
    storage: &'a mut [u8],
}

/// Typed raw-pointer form of the same storage contract for the unavoidable
/// generated-C ABI seam. Construction and access stay unsafe because a raw
/// pointer carries no allocation length; the lane mapping itself is still
/// centralized and cannot be reimplemented differently by each shim.
#[derive(Clone, Copy)]
pub struct RdramPtr(std::ptr::NonNull<u8>);

/// Report every raw RDRAM write touching a watched physical address.
///
/// The attributed observers (`fn64_cpu_runtime::set_write_observer`) only fire on
/// declared writes, so they cannot see a writer that fails to declare -- which
/// is exactly what WM2000's `0x0009b0b3` failure requires. This sits on the raw
/// store path instead, below attribution, so nothing can bypass it.
///
/// Set `FN64_WATCH_WRITE=0x9b0b3` (hex, with or without `0x`) to arm it.
fn watch_raw_write(addr: RdramAddr, len: u32, kind: &str) {
    use std::sync::OnceLock;
    static WATCH: OnceLock<Option<u32>> = OnceLock::new();
    let watch = *WATCH.get_or_init(|| {
        std::env::var("FN64_WATCH_WRITE").ok().and_then(|value| {
            let value = value.trim().trim_start_matches("0x");
            u32::from_str_radix(value, 16).ok()
        })
    });
    let Some(watch) = watch else { return };
    let start = addr.offset();
    if start <= watch && watch < start.saturating_add(len) {
        eprintln!("[watch-write] {kind} [{start:#010x},+{len:#x}) covers {watch:#010x}");
        // Name the caller. Which shim issues this store is the whole question,
        // and a backtrace is the only thing that answers it directly.
        if std::env::var_os("FN64_WATCH_WRITE_BACKTRACE").is_some() {
            // One capture per line, tagged, so two separate stacks cannot be
            // read as one call chain -- which is exactly the mistake that
            // produced a wrong attribution for this byte.
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            for line in backtrace.lines() {
                eprintln!("[watch-bt {kind}@{start:#010x}] {line}");
            }
        }
    }
}

impl RdramPtr {
    /// # Safety
    /// `storage` must be non-null and remain valid for every logical address
    /// subsequently accessed through the returned pointer.
    pub unsafe fn from_storage_ptr(storage: *mut u8) -> Self {
        Self(std::ptr::NonNull::new(storage).expect("RDRAM storage pointer must not be null"))
    }

    /// # Safety
    /// The allocation must cover the native word at `addr.offset()`.
    pub unsafe fn read_u32(self, addr: RdramAddr) -> u32 {
        assert!(
            addr.offset().is_multiple_of(4),
            "RDRAM raw u32 read at unaligned logical address {:#x}",
            addr.offset()
        );
        unsafe { (self.0.as_ptr().add(addr.offset() as usize) as *const u32).read_unaligned() }
    }

    /// # Safety
    /// The allocation must cover the native word at `addr.offset()`.
    pub unsafe fn write_u32(self, addr: RdramAddr, value: u32) {
        assert!(
            addr.offset().is_multiple_of(4),
            "RDRAM raw u32 write at unaligned logical address {:#x}",
            addr.offset()
        );
        unsafe { (self.0.as_ptr().add(addr.offset() as usize) as *mut u32).write_unaligned(value) };
    }

    /// # Safety
    /// The allocation must cover `addr.offset() ^ 3`.
    pub unsafe fn read_u8(self, addr: RdramAddr) -> u8 {
        unsafe { *self.0.as_ptr().add((addr.offset() ^ 3) as usize) }
    }

    /// # Safety
    /// The allocation must cover `addr.offset() ^ 3`.
    pub unsafe fn write_u8(self, addr: RdramAddr, value: u8) {
        // RdramPtr is the RAW path: no bounds check, no attribution, and it is
        // NOT RdramViewMut. Instrumenting only the view missed this writer
        // entirely -- the byte write to 0x0009b0b3 produced no backtrace at
        // all, which is what revealed the two types are distinct here.
        watch_raw_write(addr, 1, "ptr_write_u8");
        unsafe { *self.0.as_ptr().add((addr.offset() ^ 3) as usize) = value };
    }

    /// # Safety
    /// The allocation must cover the native halfword at `addr.offset() ^ 2`.
    pub unsafe fn read_u16(self, addr: RdramAddr) -> u16 {
        assert!(
            addr.offset().is_multiple_of(2),
            "RDRAM raw u16 read at unaligned logical address {:#x}",
            addr.offset()
        );
        unsafe {
            (self.0.as_ptr().add((addr.offset() ^ 2) as usize) as *const u16).read_unaligned()
        }
    }

    /// # Safety
    /// The allocation must cover the native halfword at `addr.offset() ^ 2`.
    pub unsafe fn write_u16(self, addr: RdramAddr, value: u16) {
        assert!(
            addr.offset().is_multiple_of(2),
            "RDRAM raw u16 write at unaligned logical address {:#x}",
            addr.offset()
        );
        unsafe {
            (self.0.as_ptr().add((addr.offset() ^ 2) as usize) as *mut u16).write_unaligned(value)
        };
    }
}

impl<'a> RdramViewMut<'a> {
    pub fn from_storage(storage: &'a mut [u8]) -> Self {
        Self { storage }
    }

    pub fn len(&self) -> usize {
        self.storage.len()
    }

    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn as_view(&self) -> RdramView<'_> {
        RdramView::from_storage(self.storage)
    }

    pub fn write_u32(&mut self, addr: RdramAddr, value: u32) {
        watch_raw_write(addr, 4, "write_u32");
        assert!(
            addr.offset().is_multiple_of(4),
            "RDRAM u32 write at unaligned logical address {:#x}",
            addr.offset()
        );
        let range = self.as_view().range(addr, 4, 0, "write_u32");
        self.storage[range].copy_from_slice(&value.to_ne_bytes());
    }

    pub fn write_u16(&mut self, addr: RdramAddr, value: u16) {
        assert!(
            addr.offset().is_multiple_of(2),
            "RDRAM u16 write at unaligned logical address {:#x}",
            addr.offset()
        );
        let range = self.as_view().range(addr, 2, 2, "write_u16");
        self.storage[range].copy_from_slice(&value.to_ne_bytes());
    }

    pub fn write_u8(&mut self, addr: RdramAddr, value: u8) {
        watch_raw_write(addr, 1, "write_u8");
        let index = self.as_view().range(addr, 1, 3, "write_u8").start;
        self.storage[index] = value;
    }

    /// Copy flat device/host bytes into storage in logical guest order.
    pub fn write_logical_bytes(&mut self, addr: RdramAddr, data: &[u8]) {
        let len = u32::try_from(data.len()).expect("logical RDRAM copy length exceeds u32");
        watch_raw_write(addr, len, "write_logical_bytes");
        if len == 0 {
            return;
        }

        // Prove the complete logical range before the first store, so an
        // invalid tail cannot leave a valid prefix already written.
        let end = addr
            .offset()
            .checked_add(len)
            .expect("logical RDRAM copy overflow");
        if usize::try_from(end).expect("logical RDRAM end exceeds host indexing")
            > self.storage.len()
        {
            for offset in 0..len {
                let at = addr
                    .checked_add(offset)
                    .expect("logical RDRAM copy address overflow");
                let _ = self.as_view().range(at, 1, 3, "write_u8");
            }
        }

        let start = addr.offset();
        let head = ((4 - (start % 4)) % 4).min(len);
        let body = (len - head) & !3;

        for (offset, &byte) in (0..head).zip(data) {
            let logical = addr
                .checked_add(offset)
                .expect("logical RDRAM copy address overflow");
            let index = self.as_view().range(logical, 1, 3, "write_u8").start;
            self.storage[index] = byte;
        }
        if body > 0 {
            let body_start = usize::try_from(start + head)
                .expect("logical RDRAM body start exceeds host indexing");
            let data_start =
                usize::try_from(head).expect("logical RDRAM head length exceeds host indexing");
            let byte_count =
                usize::try_from(body).expect("logical RDRAM body length exceeds host indexing");
            let storage = &mut self.storage[body_start..body_start + byte_count];
            storage.copy_from_slice(&data[data_start..data_start + byte_count]);
            for word in storage.chunks_exact_mut(4) {
                word.reverse();
            }
        }
        for offset in (head + body)..len {
            let logical = addr
                .checked_add(offset)
                .expect("logical RDRAM copy address overflow");
            let index = self.as_view().range(logical, 1, 3, "write_u8").start;
            self.storage[index] =
                data[usize::try_from(offset).expect("logical RDRAM offset exceeds host indexing")];
        }
    }

    /// Copy flat device bytes into this borrowed storage in logical guest
    /// order. This is the borrowed-buffer counterpart to
    /// [`Rdram::dma_write_bytes`].
    pub fn dma_write_bytes(&mut self, offset: usize, data: &[u8]) {
        let addr =
            RdramAddr::from_offset(u32::try_from(offset).expect("DMA RDRAM offset exceeds u32"));
        self.write_logical_bytes(addr, data);
    }

    /// Read native-word storage back into flat device byte order. This is
    /// the borrowed-buffer counterpart to [`Rdram::dma_read_bytes_flat`].
    pub fn dma_read_bytes_flat(&self, offset: usize, len: usize) -> Vec<u8> {
        let addr =
            RdramAddr::from_offset(u32::try_from(offset).expect("DMA RDRAM offset exceeds u32"));
        let mut flat = vec![0; len];
        self.as_view().copy_logical_bytes(addr, &mut flat);
        flat
    }
}

/// Owns the single rdram allocation. Every consumer (`fn64-abi` shims, the
/// executor, `fn64-rt64`'s gfx task marshalling) borrows this; nothing
/// allocates a second copy. See `docs/DESIGN.md` section 3's "rdram buffer
/// ownership" writeup: this matches the ABI's own contract of "one shared
/// buffer, passed by reference to every RECOMP_FUNC," not a per-caller view.
pub struct Rdram {
    bytes: Box<[u8]>,
}

impl Rdram {
    pub fn new(size: usize) -> Self {
        Rdram {
            bytes: vec![0u8; size].into_boxed_slice(),
        }
    }

    /// `Rdram::new`, but with the buffer extended (if needed) to cover the
    /// `0xA4xxxxxx` hardware-register window `mmio.rs` models
    /// (`RDRAM_MMIO_WINDOW_END`) -- what a caller that expects generated
    /// code to issue RAW `MEM_W`/`MEM_H`/`MEM_B` loads/stores against MMIO
    /// addresses (not exclusively through an `osXxx_recomp` shim) must use
    /// instead of plain `new`, per `mmio.rs`'s module doc: a raw guest load
    /// at e.g. `AI_STATUS` is a real, out-of-bounds pointer dereference
    /// against a buffer sized only for RDRAM proper (see
    /// `docs/BOOT-NOTES-WM2000.md`'s exact LLDB-confirmed crash this
    /// constructor exists to make survivable). `size` should still be at
    /// least `DEFAULT_RDRAM_SIZE` for normal RDRAM content; this only grows
    /// the buffer, never shrinks it below `size`.
    ///
    /// The caller is still responsible for calling
    /// `MmioSpace::sync_into_rdram(rdram.as_mut_ptr())` at the right points
    /// (see that method's doc comment) -- this constructor only guarantees
    /// the byte range is safely addressable, not that it holds live values
    /// yet (a fresh buffer reads as zero, which happens to already satisfy
    /// `AI_STATUS`'s "not busy, not full" idle default -- see
    /// `mmio.rs::AiRegs::status` -- but callers should not rely on that
    /// coincidence for registers whose idle-zero value isn't itself the
    /// correct default. Timed SP state is owned by `DeviceFabric` and is not
    /// mirrored into this legacy allocation.
    pub fn new_with_mmio(size: usize) -> Self {
        let size = size.max(crate::mmio::RDRAM_MMIO_WINDOW_END as usize);
        Rdram::new(size)
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// `MEM_W`: int32_t, word-aligned, no byte-lane XOR, NATIVE byte order,
    /// sign-extended (native `int32_t`, so sign is inherent, not a separate
    /// step). See this module's doc comment for why native, not big-endian.
    pub fn read_w(&self, addr: RdramAddr) -> i32 {
        RdramView::from_storage(&self.bytes).read_i32(addr)
    }

    pub fn write_w(&mut self, addr: RdramAddr, value: i32) {
        RdramViewMut::from_storage(&mut self.bytes).write_u32(addr, value as u32);
    }

    /// `MEM_H`: int16_t, byte-lane XOR `offset ^ 2`, NATIVE byte order at
    /// the corrected offset, sign-extended.
    pub fn read_h(&self, addr: RdramAddr) -> i16 {
        RdramView::from_storage(&self.bytes).read_i16(addr)
    }

    pub fn write_h(&mut self, addr: RdramAddr, value: i16) {
        RdramViewMut::from_storage(&mut self.bytes).write_u16(addr, value as u16);
    }

    /// `MEM_HU`: uint16_t, byte-lane XOR `offset ^ 2`, NATIVE byte order,
    /// zero-extended.
    pub fn read_hu(&self, addr: RdramAddr) -> u16 {
        RdramView::from_storage(&self.bytes).read_u16(addr)
    }

    pub fn write_hu(&mut self, addr: RdramAddr, value: u16) {
        RdramViewMut::from_storage(&mut self.bytes).write_u16(addr, value);
    }

    /// `MEM_B`: int8_t, byte-lane XOR `offset ^ 3`, sign-extended.
    pub fn read_b(&self, addr: RdramAddr) -> i8 {
        RdramView::from_storage(&self.bytes).read_i8(addr)
    }

    pub fn write_b(&mut self, addr: RdramAddr, value: i8) {
        RdramViewMut::from_storage(&mut self.bytes).write_u8(addr, value as u8);
    }

    /// `MEM_BU`: uint8_t, byte-lane XOR `offset ^ 3`, zero-extended.
    pub fn read_bu(&self, addr: RdramAddr) -> u8 {
        RdramView::from_storage(&self.bytes).read_u8(addr)
    }

    pub fn write_bu(&mut self, addr: RdramAddr, value: u8) {
        RdramViewMut::from_storage(&mut self.bytes).write_u8(addr, value);
    }

    /// Raw pointer to the start of the buffer, for `fn64-abi` to hand to
    /// generated C's `uint8_t* rdram` parameter. The only sanctioned
    /// escape hatch for the "one shared buffer" rule in `docs/DESIGN.md`
    /// section 3 — generated code's own calling convention requires a raw
    /// pointer, not a Rust reference.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.bytes.as_mut_ptr()
    }

    /// Flat bulk byte copy at a plain rdram-relative offset -- no byte-lane
    /// swizzle. For a caller that already holds bytes in THIS buffer's
    /// native-endian-word layout, i.e. one that has word-swapped a
    /// big-endian source image itself before writing. Cartridge PI DMA does
    /// NOT use this -- see `dma_write_bytes`.
    pub fn write_bytes(&mut self, offset: usize, data: &[u8]) {
        self.bytes[offset..offset + data.len()].copy_from_slice(data);
    }

    /// Bulk DMA write of big-endian cartridge bytes, swizzled to this buffer's
    /// native-endian-WORD layout so a later sub-word `MEM_*` read lands on the
    /// correct byte.
    ///
    /// `MEM_W` is `from_ne_bytes` and sub-word accessors carry the `^2`/`^3`
    /// byte-lane XOR precisely because storage is native-endian-word (see this
    /// module's header). So a big-endian source word `b0 b1 b2 b3` must be
    /// stored `b3 b2 b1 b0`, as if the guest had done `MEM_W(word)` -- else
    /// `MEM_W` reads it byteswapped and `MEM_BU(word+k)` reads byte `3-k`.
    ///
    /// Caught by OoT boot: `Locale_Init` DMAs the ROM header and `lbu`s the
    /// region byte; a flat copy delivered `'L'` instead of `'J'`, so the
    /// region check matched neither E nor J and the game deliberately
    /// `LogUtils_HungupThread`'d. Same class as the CPU-load byteswap this
    /// module already fixed for `MEM_*` -- DMA-in was the remaining hole.
    ///
    /// Per-BYTE swizzle (`dst[(offset+k) ^ 3] = data[k]`), not per-word: this
    /// is the general form that matches `MEM_BU`'s own `^3` lane XOR for ANY
    /// offset and length, so it stays correct for the sub-word / non-word-
    /// aligned transfers OoT's `DmaMgr_DmaRomToRam` actually issues (e.g.
    /// `len=0x86`). For a word-aligned run it is identical to reversing each
    /// 4-byte group; for a partial tail it swizzles each byte to its own lane
    /// (a bare word-chunk loop would drop or misplace the leftover bytes).
    pub fn dma_write_bytes(&mut self, offset: usize, data: &[u8]) {
        let addr =
            RdramAddr::from_offset(u32::try_from(offset).expect("DMA RDRAM offset exceeds u32"));
        RdramViewMut::from_storage(&mut self.bytes).write_logical_bytes(addr, data);
    }

    /// Inverse of `dma_write_bytes`: read `len` bytes out of native-word rdram
    /// back to FLAT (device/save) byte order, `out[k] = bytes[(offset+k) ^ 3]`.
    /// For a FromRdram (save-write) DMA whose source is guest rdram. Same
    /// per-byte lane XOR, so it too is correct for any offset/length.
    pub fn dma_read_bytes_flat(&self, offset: usize, len: usize) -> Vec<u8> {
        let addr =
            RdramAddr::from_offset(u32::try_from(offset).expect("DMA RDRAM offset exceeds u32"));
        let mut flat = vec![0; len];
        RdramView::from_storage(&self.bytes).copy_logical_bytes(addr, &mut flat);
        flat
    }

    pub fn read_bytes(&self, offset: usize, len: usize) -> &[u8] {
        &self.bytes[offset..offset + len]
    }
}

impl Default for Rdram {
    fn default() -> Self {
        Rdram::new(DEFAULT_RDRAM_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static_assertions::assert_not_impl_any!(PhysicalRdramRead<'static>: Clone, Send, Sync);

    /// `storage_slice` must expose exactly the bytes `copy_logical_bytes`
    /// reads, in storage order -- the property the mutation guard's baseline
    /// comparison relies on to skip building a snapshot. If the two ever
    /// disagreed, the guard would silently accept a changed region.
    #[test]
    fn storage_slice_is_the_word_reverse_of_the_logical_copy() {
        let mut storage = vec![0u8; 256];
        for (index, byte) in storage.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(7).wrapping_add(3);
        }
        let view = RdramView::from_storage(&storage);
        for &(start, len) in &[(0usize, 64usize), (16, 32), (64, 4), (0, 256)] {
            let mut logical = vec![0u8; len];
            view.copy_logical_bytes(RdramAddr::from_offset(start as u32), &mut logical);
            let raw = view.storage_slice(start, len).expect("span is mapped");
            let mut reversed = logical.clone();
            for word in reversed.chunks_exact_mut(4) {
                word.reverse();
            }
            assert_eq!(
                reversed, raw,
                "storage_slice must equal the word-reversed logical copy at {start:#x}+{len:#x}"
            );
        }
    }

    #[test]
    fn storage_slice_reports_an_unmapped_span_rather_than_panicking() {
        let storage = vec![0u8; 64];
        let view = RdramView::from_storage(&storage);
        assert!(view.storage_slice(32, 32).is_some());
        assert!(view.storage_slice(32, 64).is_none());
        assert!(view.storage_slice(64, 4).is_none());
    }

    #[test]
    fn new_with_mmio_covers_the_real_crash_address() {
        // The exact address docs/BOOT-NOTES-WM2000.md's LLDB backtrace
        // named: a raw guest lw at AI_STATUS (0xA450000C) must be an
        // in-bounds read, not a panic, once backed by new_with_mmio.
        let rdram = Rdram::new_with_mmio(DEFAULT_RDRAM_SIZE);
        let addr = RdramAddr::from_gpr(0xA450_000C);
        assert!(
            (addr.offset() as usize) + 4 <= rdram.len(),
            "new_with_mmio must size the buffer to cover the real AI_STATUS offset"
        );
        // Does not panic -- this is the actual regression test.
        let _ = rdram.read_w(addr);
    }

    #[test]
    fn new_with_mmio_never_shrinks_below_requested_size() {
        let rdram = Rdram::new_with_mmio(64 * 1024 * 1024);
        assert!(rdram.len() >= 64 * 1024 * 1024);
    }

    #[test]
    fn rdram_addr_from_gpr_matches_plain_32_bit() {
        // A plain 32-bit vram value with no sign extension.
        let plain = RdramAddr::from_gpr(0x8000_1234);
        assert_eq!(plain.offset(), 0x1234);
    }

    #[test]
    fn rdram_addr_from_gpr_matches_sign_extended_64_bit() {
        // The same logical address, sign-extended to 64 bits the way a
        // gpr may carry it (per ABI-SURFACE.md section (b)).
        let extended = RdramAddr::from_gpr(0xFFFF_FFFF_8000_1234);
        assert_eq!(extended.offset(), 0x1234);
    }

    #[test]
    fn rdram_addr_from_gpr_aliases_kseg0_and_kseg1_only_inside_rdram() {
        for address in [
            0x0000_0000_8000_1234,
            0xffff_ffff_8000_1234,
            0x0000_0000_a000_1234,
            0xffff_ffff_a000_1234,
        ] {
            assert_eq!(RdramAddr::from_gpr(address).offset(), 0x1234);
        }
        assert_eq!(
            RdramAddr::from_gpr(0xffff_ffff_a450_000c).offset(),
            0x2450_000c,
            "MMIO must retain its sparse backing offset"
        );
        assert_ne!(RdramAddr::from_gpr(0x1234).offset(), 0x1234);
        assert_ne!(RdramAddr::from_gpr(0xffff_ffff_c000_1234).offset(), 0x1234);
        assert!(std::panic::catch_unwind(|| RdramAddr::from_gpr(0x0000_0001_8000_1234)).is_err());
    }

    /// Regression: `to_kseg0` must round-trip the KSEG0 form that a
    /// `from_gpr(sign-extended)` came from, AND the value a shim RETURNS in
    /// `$v0` from it must SIGN-extend to match a guest-side `MEM_W` load of
    /// the same pointer. `osViGetCurrentFramebuffer` returns this; OoT's
    /// `Sched_HandleRetrace` (funcs_41.c 0x800A3288, `bnel $v1, $v0`)
    /// compares it against `pendingSwapBuf1->swapBuffer` loaded via `MEM_W`
    /// (`*(int32_t*)`, sign-extended). A KSEG0 framebuffer like 0x803B5000
    /// has bit 31 set, so the return value MUST become 0xFFFFFFFF_803B5000,
    /// not 0x00000000_803B5000 -- the zero-extended form made the `bnel`
    /// mismatch and froze the framebuffer-swap chain at exactly 1 swap.
    #[test]
    fn to_kseg0_high_bit_set_sign_extends_like_mem_w() {
        // 0x3B5000 physical -> 0x803B5000 KSEG0 (bit 31 set).
        let fb = RdramAddr::from_offset(0x003B_5000);
        assert_eq!(fb.to_kseg0(), 0x803B_5000);
        // Inverse of from_gpr's sign-extended form must be consistent.
        assert_eq!(
            RdramAddr::from_gpr(0xFFFF_FFFF_803B_5000).offset(),
            0x003B_5000
        );
        // The $v0 return a shim computes (`to_kseg0() as i32 as u64`) must
        // equal what a guest MEM_W load of the same stored pointer yields
        // (i32 sign-extended into a u64 gpr) -- distinguishable from the
        // buggy zero-extended value.
        let returned = fb.to_kseg0() as i32 as u64;
        assert_eq!(returned, 0xFFFF_FFFF_803B_5000);
        assert_ne!(
            returned, 0x0000_0000_803B_5000,
            "zero-extended return is the bug: it never compares equal to a MEM_W-loaded fb ptr"
        );
    }

    #[test]
    fn word_read_write_roundtrip() {
        let mut rdram = Rdram::new(64);
        let addr = RdramAddr::from_offset(0x10);
        rdram.write_w(addr, -1);
        assert_eq!(rdram.read_w(addr), -1);
        assert_eq!(&rdram.bytes[0x10..0x14], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn halfword_byte_lane_xor() {
        let mut rdram = Rdram::new(64);
        // offset 0 XOR 2 = 2: the halfword at word-offset 0 within a
        // big-endian-ADDRESSED word lands at byte offset 2, not 0 -- but
        // the 2 bytes stored there are in NATIVE (little-endian) order,
        // per this module's corrected doc comment (MEM_H is a native
        // int16_t* dereference, not a big-endian assembly).
        rdram.write_h(RdramAddr::from_offset(0), 0x1234);
        assert_eq!(&rdram.bytes[2..4], &[0x34, 0x12]);
        assert_eq!(rdram.read_h(RdramAddr::from_offset(0)), 0x1234);
    }

    #[test]
    fn byte_lane_xor_and_zero_extension() {
        let mut rdram = Rdram::new(64);
        // offset 0 XOR 3 = 3: the byte at word-offset 0 lands at byte 3.
        rdram.write_bu(RdramAddr::from_offset(0), 0xFE);
        assert_eq!(rdram.bytes[3], 0xFE);
        assert_eq!(rdram.read_bu(RdramAddr::from_offset(0)), 0xFE);
        // read_b sign-extends; read_bu zero-extends -- same bits, different result.
        assert_eq!(rdram.read_b(RdramAddr::from_offset(0)), -2i8);
    }

    #[test]
    fn one_logical_value_agrees_across_word_halfword_and_byte_accesses() {
        let mut storage = [0u8; 8];
        let mut view = RdramViewMut::from_storage(&mut storage);
        view.write_u32(RdramAddr::from_offset(0), 0x1122_3344);
        let view = view.as_view();

        assert_eq!(view.read_u32(RdramAddr::from_offset(0)), 0x1122_3344);
        assert_eq!(view.read_u16(RdramAddr::from_offset(0)), 0x1122);
        assert_eq!(view.read_u16(RdramAddr::from_offset(2)), 0x3344);
        assert_eq!(
            (0..4)
                .map(|offset| view.read_u8(RdramAddr::from_offset(offset)))
                .collect::<Vec<_>>(),
            [0x11, 0x22, 0x33, 0x44]
        );
        assert_eq!(storage[..4], [0x44, 0x33, 0x22, 0x11]);
    }

    #[test]
    fn logical_bulk_copy_roundtrips_across_unaligned_word_boundaries() {
        let logical = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76];
        let start = RdramAddr::from_offset(1);
        let mut storage = [0u8; 12];
        RdramViewMut::from_storage(&mut storage).write_logical_bytes(start, &logical);

        let mut copied = [0u8; 7];
        RdramView::from_storage(&storage).copy_logical_bytes(start, &mut copied);
        assert_eq!(copied, logical);
    }

    #[test]
    fn invalid_logical_bulk_write_rejects_before_mutating_a_valid_prefix() {
        let mut storage = [0x5au8; 8];
        let before = storage;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            RdramViewMut::from_storage(&mut storage)
                .write_logical_bytes(RdramAddr::from_offset(6), &[1, 2, 3]);
        }));
        assert!(outcome.is_err());
        assert_eq!(storage, before);
    }

    #[test]
    fn raw_abi_pointer_agrees_with_bounded_views() {
        let mut storage = [0u8; 8];
        let raw = unsafe { RdramPtr::from_storage_ptr(storage.as_mut_ptr()) };

        unsafe {
            raw.write_u16(RdramAddr::from_offset(0), 0x1234);
            raw.write_u8(RdramAddr::from_offset(2), 0x56);
        }

        let view = RdramView::from_storage(&storage);
        assert_eq!(view.read_u16(RdramAddr::from_offset(0)), 0x1234);
        assert_eq!(view.read_u8(RdramAddr::from_offset(2)), 0x56);
        assert_eq!(unsafe { raw.read_u16(RdramAddr::from_offset(0)) }, 0x1234);
        assert_eq!(unsafe { raw.read_u8(RdramAddr::from_offset(2)) }, 0x56);
    }

    /// The word-wise `copy_logical_bytes` must agree with the per-byte reader
    /// it replaced, at every offset and length -- including the unaligned head
    /// and tail that take the slow path.
    ///
    /// This is a differential test against the obvious implementation, because
    /// the fast path's correctness rests on one non-obvious fact: logical byte
    /// `n` lives at storage `n ^ 3`, which within an aligned word is a byte
    /// reversal. Getting that backwards would still produce plausible-looking
    /// bytes, so asserting against a reference is worth more than asserting
    /// against hand-written expectations.
    #[test]
    fn word_wise_copy_matches_the_per_byte_reader_at_every_alignment() {
        let mut storage = vec![0u8; 256];
        for (index, byte) in storage.iter_mut().enumerate() {
            // Distinct per byte, and not a function of the low two bits alone,
            // so a lane-order mistake cannot coincidentally match.
            *byte = (index as u8).wrapping_mul(7).wrapping_add(index as u8 >> 3);
        }
        let view = RdramView::from_storage(&storage);

        for start in 0..16u32 {
            for len in 0..=24usize {
                if start as usize + len > storage.len() {
                    continue;
                }
                let addr = RdramAddr::from_offset(start);
                let reference: Vec<u8> = (0..len)
                    .map(|index| view.read_u8(addr.checked_add(index as u32).unwrap()))
                    .collect();
                let mut actual = vec![0u8; len];
                view.copy_logical_bytes(addr, &mut actual);
                assert_eq!(
                    actual, reference,
                    "copy_logical_bytes disagreed at start={start} len={len}"
                );
            }
        }
    }
}
