//! Safe borrows of the process RDRAM allocation from an ABI base pointer.
//!
//! # The pattern this replaces
//!
//! The recompiled-code ABI hands every shim a raw `*mut u8` RDRAM base and a
//! separately-carried length. Turning that pair into something a reader can use
//! was open-coded at 31 sites in this crate as
//!
//! ```ignore
//! let view = RdramView::from_storage(unsafe {
//!     std::slice::from_raw_parts(rdram, rdram_len)
//! });
//! ```
//!
//! Each such site was individually responsible for having already proved
//! `rdram` non-null, `rdram_len` the *registered* length of that one
//! allocation, and the borrow not outliving the call. That proof was carried in
//! a prose `// SAFETY:` comment, and it was the same proof every time -- which
//! is the shape AGENTS.md's "types before audits" rule exists to eliminate.
//!
//! [`ProcessRdram`] does the check once, in one place, with one `SAFETY`
//! comment, and hands back a borrow whose lifetime is tied to the guard. The
//! call sites become safe code.
//!
//! # What is and is not checked
//!
//! Checked here, loudly: the base is non-null, the length is nonzero, and any
//! requested sub-range lies inside the length. A violation panics naming the
//! base, the length and the range -- it never truncates to what happens to fit.
//! Silent truncation is what would turn a bad length into a wrong-pixels bug a
//! thousand frames later instead of a stack trace at the fault.
//!
//! NOT checked, because it is not checkable from a pointer and a length: that
//! the pointer actually addresses that many live bytes. That obligation is
//! discharged once, at registration, by [`crate::host::register_process_rdram`]
//! -- which asserts the allocation is never replaced while live -- and is what
//! [`ProcessRdram::new`]'s own `# Safety` section requires of its caller. This
//! wrapper narrows the audit surface from 31 sites to one; it does not
//! eliminate it, and claiming otherwise would be the "defensive guard that
//! hides corruption" AGENTS.md forbids.
//!
//! # Why this is read-only
//!
//! 13 of the 31 sites want `from_raw_parts_mut`, and there is deliberately no
//! `storage_mut` here. A `&mut [u8]` claims *exclusivity*, and nothing this
//! type can check establishes it: the guard is `Copy`, two guards over one
//! allocation can coexist, and the actual exclusion is a runtime property --
//! a single runnable guest coroutine at a time -- that lives outside the type.
//! A `storage_mut` would therefore have to stay `unsafe` and would make its
//! call sites no safer, while *looking* like it had. Making the mutable half
//! genuinely safe needs the single-runnable-thread token to be a type the
//! borrow can be tied to, which is a larger change than this one; until then
//! those sites keep their open-coded form and their individual `SAFETY`
//! comments.
//!
//! What this does NOT mean is that the read-only half is exempt. A `&[u8]`
//! asserts its bytes do not change for the borrow's life, and a running guest
//! coroutine falsifies that exactly as a second `&mut` would -- the shared
//! borrows are unsound under a concurrent guest write, not merely racy. The
//! difference between the two halves is not soundness but *honesty*: the safe
//! read-only methods are safe to CALL only because [`ProcessRdram::new`]'s
//! `unsafe` contract already extracted the promise that no guest write races
//! them, whereas a `storage_mut` would have needed a second, stronger promise
//! it had no way to state. The single-runnable-coroutine property carries both.

use fn64_runtime::RdramView;

/// A checked borrow of the process RDRAM allocation.
///
/// Construct once from the ABI's `(base, len)` pair, then take safe views from
/// it. The guard's lifetime bounds every borrow it hands out, so a view cannot
/// outlive the scope in which the caller proved the allocation live.
#[derive(Clone, Copy)]
pub(crate) struct ProcessRdram {
    base: *const u8,
    len: usize,
}

impl ProcessRdram {
    /// Wrap the ABI's RDRAM base pointer and its registered length.
    ///
    /// Panics if `base` is null or `len` is zero: an ABI shim reached with no
    /// live RDRAM is a caller bug, and the loud trap names it here rather than
    /// letting the first read fault somewhere downstream.
    ///
    /// # Safety
    /// `base` must address at least `len` live, initialised bytes, and that
    /// allocation must outlive this guard. In this crate that is the process
    /// allocation registered by [`crate::host::register_process_rdram`], whose
    /// length is the `rdram_len` the ABI carries alongside the base; the
    /// registration asserts the allocation is never replaced while live.
    ///
    /// **The read-only borrows are not exempt from the exclusivity problem
    /// that kept `storage_mut` out of this type** (see the module header). A
    /// `&[u8]` handed out by [`Self::storage`], [`Self::view`] or
    /// [`Self::storage_range`] is UB if the guest writes those bytes while it
    /// is alive -- a shared reference asserts the bytes do not change, which a
    /// running guest coroutine would falsify just as a `&mut` would. Nothing
    /// in this type establishes that; the guarantee rests entirely on the
    /// single-runnable-coroutine property, i.e. the caller must hold no live
    /// borrow across a point where guest execution can resume. Every converted
    /// call site satisfies this by taking its borrow and dropping it within
    /// one host-side operation, with the guest suspended throughout.
    ///
    /// So the safety obligation here is two-part, and only the first part is
    /// discharged by registration: the allocation must be live (registration),
    /// and no guest write may race the borrow (single-runnable-coroutine).
    pub(crate) unsafe fn new(base: *const u8, len: usize) -> Self {
        assert!(!base.is_null(), "process RDRAM base pointer must be non-null");
        assert!(len > 0, "process RDRAM length must be nonzero");
        Self { base, len }
    }

    /// Borrow the whole allocation as native-word storage bytes.
    ///
    /// Storage order, not logical guest order -- this is the same slice
    /// [`RdramView::from_storage`] expects, and the lane mapping stays the
    /// view's business.
    pub(crate) fn storage(&self) -> &[u8] {
        // SAFETY: `new`'s contract requires `base` to address `len` live
        // initialised bytes for at least this guard's lifetime, and `new`
        // rejected a null base. The returned borrow is tied to `&self`, so it
        // cannot outlive the guard, and `ProcessRdram` hands out no `&mut`, so
        // no aliasing `&mut [u8]` can coexist with it through this type.
        unsafe { std::slice::from_raw_parts(self.base, self.len) }
    }

    /// A bounds-checked [`RdramView`] over the whole allocation.
    ///
    /// This is the replacement for the open-coded
    /// `RdramView::from_storage(unsafe { from_raw_parts(rdram, rdram_len) })`.
    pub(crate) fn view(&self) -> RdramView<'_> {
        RdramView::from_storage(self.storage())
    }

    /// Borrow `len` storage bytes starting at storage offset `start`.
    ///
    /// Panics if the range runs past the allocation, naming the range and the
    /// length. It never returns a short slice: a caller asking for bytes that
    /// are not there has a bug, and handing back fewer would hide it.
    pub(crate) fn storage_range(&self, start: usize, len: usize) -> &[u8] {
        let end = start.checked_add(len).unwrap_or_else(|| {
            panic!("process RDRAM range {start:#x}+{len:#x} overflows usize")
        });
        assert!(
            end <= self.len,
            "process RDRAM range {start:#x}+{len:#x} (end {end:#x}) runs past \
             the registered length {:#x}",
            self.len,
        );
        &self.storage()[start..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> Vec<u8> {
        (0..64u8).collect()
    }

    #[test]
    fn storage_borrows_the_whole_registered_length() {
        let bytes = storage();
        // SAFETY: `bytes` outlives the guard and covers `bytes.len()`.
        let rdram = unsafe { ProcessRdram::new(bytes.as_ptr(), bytes.len()) };
        assert_eq!(rdram.storage().len(), 64);
        assert_eq!(rdram.storage(), &bytes[..]);
    }

    #[test]
    fn a_range_inside_the_allocation_is_returned_exactly() {
        let bytes = storage();
        // SAFETY: as above.
        let rdram = unsafe { ProcessRdram::new(bytes.as_ptr(), bytes.len()) };
        assert_eq!(rdram.storage_range(8, 4), &[8, 9, 10, 11]);
    }

    #[test]
    fn the_view_reads_the_same_bytes_the_open_coded_form_did() {
        let bytes = storage();
        // SAFETY: as above.
        let rdram = unsafe { ProcessRdram::new(bytes.as_ptr(), bytes.len()) };
        // SAFETY: same allocation, same length; this is the shape the wrapper
        // replaces, kept here as an independent oracle for one round.
        let open_coded = RdramView::from_storage(unsafe {
            std::slice::from_raw_parts(bytes.as_ptr(), bytes.len())
        });
        let addr = fn64_runtime::RdramAddr::from_offset(4);
        assert_eq!(rdram.view().read_u32(addr), open_coded.read_u32(addr));
    }

    /// The inclusive boundary. `storage_range` admits `end == len` and rejects
    /// `end == len + 1`, so the check is `end <= self.len` and not `end <`.
    ///
    /// Mutation-checked, measured: planting `end < self.len` in
    /// `storage_range` gave 8 passed / 1 failed, and the one failure was this
    /// test. Every other case in this module -- including
    /// `a_range_past_the_end_panics_rather_than_truncating` -- stayed green
    /// under the mutant, which is the point: an off-by-one still satisfies
    /// them, so this is the only case that pins the boundary itself.
    #[test]
    fn a_range_ending_exactly_at_the_registered_length_is_admitted() {
        let bytes = storage();
        // SAFETY: as above.
        let rdram = unsafe { ProcessRdram::new(bytes.as_ptr(), bytes.len()) };
        let exact = rdram.storage_range(60, 4);
        assert_eq!(exact, &[60, 61, 62, 63]);
        // The whole allocation is also an exact fit.
        assert_eq!(rdram.storage_range(0, 64).len(), 64);
    }

    #[test]
    #[should_panic(expected = "runs past the registered length")]
    fn a_range_ending_one_past_the_registered_length_panics() {
        let bytes = storage();
        // SAFETY: as above.
        let rdram = unsafe { ProcessRdram::new(bytes.as_ptr(), bytes.len()) };
        rdram.storage_range(60, 5);
    }

    #[test]
    #[should_panic(expected = "runs past the registered length")]
    fn a_range_past_the_end_panics_rather_than_truncating() {
        let bytes = storage();
        // SAFETY: as above.
        let rdram = unsafe { ProcessRdram::new(bytes.as_ptr(), bytes.len()) };
        rdram.storage_range(60, 8);
    }

    #[test]
    #[should_panic(expected = "overflows usize")]
    fn a_range_whose_end_overflows_panics() {
        let bytes = storage();
        // SAFETY: as above.
        let rdram = unsafe { ProcessRdram::new(bytes.as_ptr(), bytes.len()) };
        rdram.storage_range(usize::MAX, 2);
    }

    #[test]
    #[should_panic(expected = "must be non-null")]
    fn a_null_base_panics_at_construction() {
        // SAFETY: the call is expected to panic before reading the pointer.
        unsafe { ProcessRdram::new(std::ptr::null(), 8) };
    }

    #[test]
    #[should_panic(expected = "must be nonzero")]
    fn a_zero_length_panics_at_construction() {
        let bytes = storage();
        // SAFETY: the call is expected to panic before reading the pointer.
        unsafe { ProcessRdram::new(bytes.as_ptr(), 0) };
    }
}
