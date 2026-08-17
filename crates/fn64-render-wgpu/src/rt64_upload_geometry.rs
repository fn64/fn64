//! Literal port of the pure size/alignment/offset arithmetic buried inside
//! `RT64::BufferUploader`, a literal port of the permitted MIT RT64 Rust-port
//! source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/render/rt64_buffer_uploader.cpp` lines 15-17 (`roundUp`), lines
//!   21-23 (`Upload::valid`), lines 74-76 (`threadUpload`'s offset/size
//!   computation), lines 82-96 (`updateResources`'s growth policy), lines
//!   145-146 (`commandListCopyResources`'s identical offset/size
//!   computation) -- whole-file SHA-256
//!   `d1add014be5f6bf1df77b6f3342c6772bb5235e834b1cb1b7a4e83de167acb40`,
//!   matching `docs/rt64-port-inventory.json`'s `files[].sources.port.sha256`
//!   for this path (confirmed independently here by `shasum -a 256` against
//!   the pinned port-commit checkout).
//! - `src/render/rt64_buffer_uploader.h` lines 37-46 (`Upload`'s field
//!   layout, cited for context only -- no new type is minted from it, see
//!   "Reuse, not new type" below) -- whole-file SHA-256
//!   `04f9d99082f7369f7183dbfa04ce67ca268b56d490d1f67e3926e5889d933673`,
//!   matching the same inventory field, confirmed the same way.
//!
//! `src/render/rt64_shader_compiler.cpp`
//! (SHA-256 `0592401533f629282d5b5037a97177a1a8147deb568f568db1ea57f383c96e9a`)
//! and `src/render/rt64_shader_compiler.h`
//! (SHA-256 `f6337185da0e79c3297bbe398d170025c6624f0442444ce19836a53d94d4037a`)
//! were both read in full and contribute **nothing** to this module -- see
//! "Nonclaims" for why.
//!
//! `docs/rt64-port-inventory.json` still records all four paths above as
//! `"port_state": "not-started"` with `"ported_as": []`, so
//! `scripts/lint-docs.py` reports a `ported_as` drift for them; that is
//! expected here and is left for the owning ticket to reconcile when the
//! inventory is regenerated, since this module's writable surface does not
//! include `docs/rt64-port-inventory.json`. Whoever performs that
//! reconciliation must not take the scan at face value:
//! `tools/rt64_port_inventory.py`'s `ported_as_for` matches whole-file
//! SHA-256 digests textually and cannot tell a citation-as-port from a
//! citation-as-refusal, so it will mechanically attribute
//! `src/render/rt64_shader_compiler.cpp` and `src/render/rt64_shader_compiler.h`
//! to this module even though they contribute **zero** ported lines -- their
//! digests appear above only to record that both were read in full and
//! refused, correctly, because they are entirely DXC/COM glue with no pure
//! computation to port. Recording those two as ported here would be a false
//! claim of 168 ported lines. The same caveat applies in weaker form to the
//! other two paths: `src/render/rt64_buffer_uploader.h` is cited for
//! `Upload`'s field layout only and contributes no ported code either, and
//! even `src/render/rt64_buffer_uploader.cpp` is a partial port of roughly 30
//! of its 175 lines, so a file-granularity `port_state` over-credits it too.
//!
//! ```text
//! // rt64_buffer_uploader.cpp:15-17
//! static uint64_t roundUp(uint64_t value, uint64_t powerOf2Alignment) {
//!     return (value + powerOf2Alignment - 1) & ~(powerOf2Alignment - 1);
//! }
//!
//! // rt64_buffer_uploader.cpp:21-23
//! bool BufferUploader::Upload::valid() const {
//!     return (srcData != nullptr) && (srcDataIndexRange.second > srcDataIndexRange.first);
//! }
//!
//! // rt64_buffer_uploader.cpp:68-80 (threadUpload; offset/size lines 74-76)
//! void BufferUploader::threadUpload(const Upload &upload) {
//!     if (!upload.valid()) {
//!         return;
//!     }
//!
//!     assert(upload.dstPair != nullptr);
//!     const size_t srcOffset = upload.srcDataIndexRange.first * upload.srcDataStride;
//!     const size_t srcSize = (upload.srcDataIndexRange.second - upload.srcDataIndexRange.first) * upload.srcDataStride;
//!     const RenderRange writtenRange(srcOffset, srcOffset + srcSize);
//!     uint8_t *dstData = static_cast<uint8_t *>(upload.dstPair->uploadBuffer->map());
//!     memcpy(dstData + srcOffset, static_cast<const uint8_t *>(upload.srcData) + srcOffset, srcSize);
//!     upload.dstPair->uploadBuffer->unmap(0, &writtenRange);
//! }
//!
//! // rt64_buffer_uploader.cpp:82-108 (updateResources; growth policy lines 84-96)
//! void BufferUploader::updateResources(RenderWorker *worker, std::vector<Upload> &blankUploads) {
//!     for (Upload &u : blankUploads) {
//!         // Ignore the reallocation of the buffer if the required size is already enough. We always create a buffer if it hasn't been created yet.
//!         const size_t requiredSize = u.srcDataIndexRange.second * u.srcDataStride;
//!         BufferPair &bufferPair = *u.dstPair;
//!         if ((bufferPair.defaultBuffer != nullptr) && (!u.valid() || (bufferPair.allocatedSize >= requiredSize))) {
//!             continue;
//!         }
//!
//!         bufferPair.defaultViews.clear();
//!
//!         // Recreate the buffer pair.
//!         const uint64_t BlockAlignment = 256;
//!         bufferPair.allocatedSize = std::max(uint64_t((requiredSize * 3) / 2), BlockAlignment);
//!         bufferPair.allocatedSize = roundUp(bufferPair.allocatedSize, BlockAlignment);
//!         bufferPair.uploadBuffer = worker->device->createBuffer(RenderBufferDesc::UploadBuffer(bufferPair.allocatedSize));
//!         bufferPair.defaultBuffer = worker->device->createBuffer(RenderBufferDesc::DefaultBuffer(bufferPair.allocatedSize, u.bufferFlags));
//!
//!         bufferPair.defaultViews.reserve(u.formatViews.size());
//!         for (RenderFormat format : u.formatViews) {
//!             bufferPair.defaultViews.emplace_back(bufferPair.defaultBuffer->createBufferFormattedView(format));
//!         }
//!
//!         // Since the buffers had to be recreated, reupload all the data by modifying the source upload.
//!         u.srcDataIndexRange.first = 0;
//!     }
//! }
//!
//! // rt64_buffer_uploader.cpp:139-149 (commandListCopyResources; offset/size lines 145-146)
//! void BufferUploader::commandListCopyResources(RenderWorker *worker) {
//!     for (const Upload &u : pendingUploads) {
//!         if (!u.valid()) {
//!             continue;
//!         }
//!
//!         const uint64_t srcOffset = u.srcDataIndexRange.first * u.srcDataStride;
//!         const uint64_t srcSize = (u.srcDataIndexRange.second - u.srcDataIndexRange.first) * u.srcDataStride;
//!         worker->commandList->copyBufferRegion(u.dstPair->defaultBuffer->at(srcOffset), u.dstPair->uploadBuffer->at(srcOffset), srcSize);
//!     }
//! }
//! ```
//!
//! **Reuse, not new type.** `roundUp` is a free function in the source with
//! no receiver type at all -- ported as a free function
//! [`round_up_pow2`] here, not a method on any struct. The growth policy
//! ([`grown_capacity`]) and the offset/size pair ([`upload_span`]) are
//! likewise ported as free functions taking their operands directly, rather
//! than reconstructing `Upload`/`BufferPair` as Rust structs: those two
//! source types carry `RenderBuffer`/`RenderBufferFormattedView`/thread/mutex
//! fields with no pure-arithmetic content of their own (see "Nonclaims" for
//! the full list of what stays unrepresented). `rt64_framebuffer_storage.rs`
//! ports a *different* RT64 file's `(rdramUsed * 3) / 2` capacity-growth
//! multiply-then-divide form; this file's growth policy is related but
//! **not identical** -- it additionally takes a `max` against a fixed
//! `BlockAlignment` floor and then rounds up to that same alignment, a
//! two-step policy `FramebufferStorage::grown_capacity` does not have -- so
//! this module ports its own [`grown_capacity`] rather than reusing or
//! aliasing that one.
//!
//! ## Admitted domain
//!
//! - **Growth formula's exact form and truncation (hazard: multiply-first
//!   vs. `n + n/2`).** The source computes `uint64_t((requiredSize * 3) / 2)`
//!   -- multiply by 3 first, then integer-divide by 2 -- from `requiredSize`
//!   itself (not from the *previous* `allocatedSize`; unlike
//!   `FramebufferStorage`'s counter-based growth, this is a stateless
//!   function of the caller's requested size on each call). This port's
//!   [`grown_capacity`] preserves that literal `(required_size * 3) / 2`
//!   operation order. `grown_capacity_matches_hand_computed_multiply_by_three_divide_by_two`
//!   and the odd-value tests below pin `required_size = 7`: hand-computed
//!   `(7*3)/2 = 21/2 = 10` (not `7 + 7/2` evaluated differently -- the two
//!   forms are algebraically identical for non-overflowing unsigned inputs,
//!   per the identity `floor(3n/2) == n + floor(n/2)`, but this port
//!   preserves the source's literal multiply-then-divide order regardless of
//!   that coincidence, matching `rt64_framebuffer_storage.rs`'s established
//!   precedent for the same hazard in a different file).
//! - **The `BlockAlignment` floor (`std::max(..., 256)`) runs *before* the
//!   `roundUp` call, not after or instead of it.** `grown_capacity` computes
//!   `max((required_size * 3) / 2, 256)` and returns that; a *separate* call
//!   to [`round_up_pow2`] with the same `256` alignment is the second,
//!   independent step the source performs on the next line
//!   (`bufferPair.allocatedSize = roundUp(bufferPair.allocatedSize,
//!   BlockAlignment)`). This port keeps them as two separate functions
//!   (mirroring the source's two separate statements) rather than fusing
//!   them into one, so a caller composes `round_up_pow2(grown_capacity(n),
//!   256)` exactly as the source's two consecutive assignments do.
//!   `grown_capacity_floor_applies_before_any_alignment_rounding` below
//!   exercises a `required_size` small enough that `(n*3)/2` alone would be
//!   below `256` (proving the floor, not alignment, is what raises it), and
//!   a separate test composes both steps to show the final value can differ
//!   from either step alone.
//! - **`roundUp`'s bit-trick round-up-to-power-of-2 formula: `(value +
//!   alignment - 1) & ~(alignment - 1)`.** This is *not* a division-based
//!   `((value + alignment - 1) / alignment) * alignment` -- it is a bitmask
//!   operation that is only correct when `alignment` is an exact power of 2
//!   (the function's own parameter name, `powerOf2Alignment`, documents this
//!   precondition; the source never checks it). This port's
//!   [`round_up_pow2`] uses the identical bitmask form, not the
//!   division-based equivalent, preserving exact operation order and the
//!   silent wrong-answer-on-non-power-of-2 behavior the source has (see the
//!   divide-by-zero/precondition bullet below).
//! - **`roundUp`'s grow-threshold has no separate "already aligned" branch
//!   or comparison at all** -- unlike the two comparison-strictness hazards
//!   named in the brief (which are `if` checks deciding *whether* to grow),
//!   `roundUp` always executes its one-line formula unconditionally on every
//!   call; there is no `>`/`>=` branch inside it to get the strictness of
//!   wrong. `round_up_pow2_of_a_value_already_aligned_is_a_no_op` and
//!   `round_up_pow2_of_a_value_one_over_alignment_rounds_up_to_the_next_multiple`
//!   below test both sides of the *implicit* boundary (exactly-aligned vs.
//!   one-over) that the formula itself produces, not a branch.
//! - **`updateResources`'s reallocation-skip comparison strictness:
//!   `allocatedSize >= requiredSize` skips (i.e. an exact fit does NOT
//!   reallocate).** The `if` guard is `(bufferPair.defaultBuffer != nullptr)
//!   && (!u.valid() || (bufferPair.allocatedSize >= requiredSize))` --
//!   `continue`s (skips growth) when the existing capacity is
//!   greater-than-**or-equal-to** the requirement. This port does not
//!   reproduce the `defaultBuffer != nullptr` / `Upload::valid()` control
//!   flow (that requires the RHI buffer-lifetime state this module
//!   explicitly excludes, see "Nonclaims"), but isolates the pure
//!   size-comparison question as [`fits_without_growth`], testing the exact
//!   boundary both ways:
//!   `fits_without_growth_is_true_at_the_exact_boundary` (equal sizes: no
//!   growth needed, matching the source's `>=`) and
//!   `fits_without_growth_is_false_one_byte_over` (capacity one less than
//!   required: growth needed).
//! - **`Upload::valid()`'s range comparison is strict `>`, not `>=`.**
//!   `srcDataIndexRange.second > srcDataIndexRange.first` -- an index range
//!   with `second == first` (zero-length range) is **not** valid, and
//!   neither is an inverted range (`second < first`). This port's
//!   [`upload_is_valid`] uses the identical strict `>`, tested at
//!   `upload_is_valid_false_when_range_is_exactly_equal` (zero-length:
//!   invalid) and `upload_is_valid_true_when_range_is_one_wide` (the
//!   smallest valid range) and `upload_is_valid_false_when_range_is_inverted`.
//! - **`threadUpload`'s and `commandListCopyResources`'s offset/size
//!   computation is byte-identical across both call sites** (`srcOffset =
//!   indexRange.first * stride`; `srcSize = (indexRange.second -
//!   indexRange.first) * stride`) -- ported once as [`upload_span`] rather
//!   than duplicated, since both source call sites compute the exact same
//!   two values from the exact same three inputs with no other differing
//!   state. The subtraction `indexRange.second - indexRange.first` assumes
//!   `second >= first` (guaranteed by `Upload::valid()`'s `second > first`
//!   check having already gated execution at both call sites in the source
//!   -- both `threadUpload` and `commandListCopyResources` early-return/skip
//!   on `!upload.valid()` before reaching this arithmetic); this port's
//!   [`upload_span`] does not re-validate that precondition (matching the
//!   source, which also does not re-check it inside the arithmetic itself),
//!   and callers are expected to have checked [`upload_is_valid`] first --
//!   see the overflow/underflow bullet below for what happens if a caller
//!   doesn't.
//!
//! - **Integer overflow in `required_size * 3`, `value + alignment - 1`,
//!   `first * stride`, and `(second - first) * stride` is unsigned-wraparound
//!   behavior in the source, not a guarded error.** C++ `size_t`/`uint64_t`
//!   arithmetic is defined modulo 2^64 (`size_t` is `uint64_t`-width on the
//!   64-bit targets RT64 ships), so every multiply and add above can wrap
//!   silently on sufficiently large inputs, and the source adds no overflow
//!   check anywhere in this cluster. This port uses `wrapping_mul` /
//!   `wrapping_add` / `wrapping_sub` throughout [`grown_capacity`],
//!   [`round_up_pow2`], and [`upload_span`] to reproduce that exact
//!   modulo-2^64 wraparound rather than Rust's default checked/panicking
//!   arithmetic (which would diverge from the C++ behavior on overflowing
//!   inputs) -- not exercised by a dedicated near-`u64::MAX` test, since
//!   reaching those magnitudes needs multi-exabyte buffer sizes, out of this
//!   port's characterization scope (matching `rt64_framebuffer_storage.rs`'s
//!   "document, don't contrive an unreachable-scale test" precedent for the
//!   analogous `u32` case).
//! - **`upload_span`'s subtraction `second - first` underflows (wraps to a
//!   huge value) if called with `second < first`, and the source has no
//!   guard against this at the arithmetic site** -- the only thing
//!   preventing it in the source is that both call sites are gated behind
//!   `Upload::valid()` first (see above). This port's [`upload_span`] uses
//!   `wrapping_sub` to reproduce the identical wraparound if a caller
//!   bypasses that precondition, rather than adding a check, `Option`
//!   return, or panic the source does not have. This frontier is
//!   deliberately reported, not guarded: `upload_span` is a pure arithmetic
//!   helper with no way to *observe* whether its caller validated the range
//!   first, exactly like the C++ free function it mirrors.
//! - **`roundUp`'s divide-by-zero/precondition frontier: `powerOf2Alignment
//!   == 0`.** `~(alignment - 1)` with `alignment = 0` computes `~(u64::MAX)
//!   = 0` (via wrapping subtraction, `0u64.wrapping_sub(1) == u64::MAX`), so
//!   `round_up_pow2(value, 0)` returns `(value + u64::MAX) & 0`, which is
//!   `0` for every `value` -- **not** a divide-by-zero panic (there is no
//!   division in this formula at all, unlike a `(v + a - 1) / a * a` form
//!   would have), but a silent, well-defined-in-both-languages wrong answer
//!   for a caller that violates the "power of 2" precondition the parameter
//!   name documents but the source never checks. This port does not add a
//!   guard, matching the source; `round_up_pow2_of_a_zero_alignment_is_the_sources_documented_frontier`
//!   below asserts this exact `0` result at `alignment = 0` as the reported
//!   frontier, not a panic. A non-power-of-2 alignment (e.g. `100`) is a
//!   related, separately-reported frontier: the bitmask formula silently
//!   rounds up to the next multiple of the next-lower power of 2 implied by
//!   the alignment's bit pattern, not to a multiple of the alignment itself
//!   -- `round_up_pow2_of_a_non_power_of_2_alignment_is_the_sources_documented_frontier`
//!   below documents one such case with a hand-computed expectation rather
//!   than asserting any "correctness" property.
//! - **No private-helper visibility gap was hit.** `roundUp` is a
//!   file-local `static` free function in the source (not a class member),
//!   already at file scope with nothing further to reach into; the growth
//!   policy and offset/size arithmetic are inlined directly inside
//!   `updateResources`/`threadUpload`/`commandListCopyResources` with no
//!   named private helper hiding behind class-private access.
//!
//! ## Nonclaims
//!
//! No GPU, RHI, or production wiring (this module is not called from
//! anywhere yet and is not registered on any public crate surface beyond its
//! own `mod` declaration; dead-code warnings on its unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. `assert(upload.dstPair != nullptr)` in `threadUpload`
//! is a release-mode-stripped (`NDEBUG`) precondition on RHI object
//! lifetime, not exercised here since this module carries no `dstPair`-like
//! type at all.
//!
//! Deliberately left in the RHI/thread/GPU bulk, not ported:
//!
//! - **All of `rt64_shader_compiler.cpp`/`.h` (127 + 41 lines, whole file in
//!   both cases).** Every function (`ShaderCompiler::ShaderCompiler`,
//!   `~ShaderCompiler`, the file-local `checkResultForError`, `compile`,
//!   `link`) is a sequence of Windows COM calls (`DxcCreateInstance`,
//!   `IDxcUtils::CreateBlobFromPinned`, `IDxcCompiler::Compile`,
//!   `IDxcLinker::RegisterLibrary`/`Link`, `IDxcBlob*::Release`/`GetResult`/
//!   `GetStatus`/`GetErrorBuffer`) plus `fprintf`/exception-throwing error
//!   handling and one `std::string`-building error-message assembly loop
//!   (`memcpy` into a `std::vector<char>` for null-termination, not a
//!   general-purpose arithmetic algorithm). There is no size/alignment/
//!   offset/hash computation anywhere in this file -- the entire file is
//!   RHI-adjacent glue around a DirectX Shader Compiler COM object with no
//!   arithmetic content to characterize. This is a `#if defined(_WIN32)`
//!   Windows-only file with a `typedef void* ShaderCompiler` stub on other
//!   platforms; even the stub carries no behavior.
//! - `BufferUploader`'s constructor/destructor/`threadLoop` (lines 27-66):
//!   `std::thread` spawn/join, `std::condition_variable::wait`/`notify_all`,
//!   `std::mutex` locking -- thread/mutex machinery, explicitly out of
//!   scope per this task's hazard list.
//! - `threadUpload`'s `map()`/`memcpy`/`unmap()` calls (lines 77-79):
//!   RHI buffer mapping and the actual byte copy -- `map`/`unmap` are named
//!   out of scope; this module ports only the offset/size *arithmetic* that
//!   feeds those calls (`upload_span`), not the calls themselves.
//! - `updateResources`'s `RenderBuffer`/`RenderBufferFormattedView`
//!   creation (`worker->device->createBuffer(...)`,
//!   `bufferPair.defaultBuffer->createBufferFormattedView(format)`) and
//!   `std::vector::reserve`/`emplace_back` bookkeeping (lines 91, 97-103) --
//!   GPU device/resource-manifest calls, and a plain container-capacity
//!   hint with no arithmetic to characterize beyond what `std::vector`
//!   itself already guarantees.
//! - `submit`'s mutex-guarded queue swap and `commandListBeforeBarriers`/
//!   `commandListAfterBarriers`'s `RenderBufferBarrier`/
//!   `commandList->barriers(...)` construction (lines 110-137, 151-167) --
//!   barrier/command-list recording, explicitly out of scope.
//! - `wait()` (lines 169-174): a condition-variable wait predicate, thread
//!   machinery.
//! - `Upload`'s and `BufferPair`'s full struct definitions
//!   (`rt64_buffer_uploader.h:16-46`, `BufferPair::get`/`getView`) are not
//!   reconstructed as Rust types here: every field beyond the three scalar
//!   inputs this module's free functions take (`srcDataIndexRange`,
//!   `srcDataStride`, and the derived `requiredSize`/`allocatedSize`) is
//!   either a raw pointer (`srcData`, `dstPair`), an RHI handle
//!   (`uploadBuffer`, `defaultBuffer`, `defaultViews`,
//!   `RenderBufferFlags`/`RenderFormat`), or a `std::vector` of RHI handles
//!   -- none of it is pure arithmetic, and reconstructing the structs
//!   whole just to host these three free functions as methods would be
//!   exactly the "struct definitions and pass-through wrappers" this task's
//!   brief says not to manufacture. `BufferUploader`'s own thread/mutex/
//!   `RenderDevice*`/`std::vector<Upload>` fields (`rt64_buffer_uploader.h:
//!   48-56`) are RHI/thread state with nothing pure inside them either.
//!
//! Combined: of `rt64_buffer_uploader.cpp`'s 175 lines, roughly 30 lines
//! (the `roundUp` function body, `Upload::valid`, and the two small
//! arithmetic slivers inside `updateResources`/`threadUpload`/
//! `commandListCopyResources`) are pure and ported here; the remaining
//! ~145 lines are thread/mutex machinery, RHI buffer/view/barrier/
//! command-list calls, or `map`/`unmap`/`memcpy`, all explicitly out of
//! scope. `rt64_buffer_uploader.h`'s 69 lines contribute no ported code at
//! all (cited for field-layout context only). `rt64_shader_compiler.cpp`/
//! `.h`'s combined 168 lines contribute zero -- the whole pair is RHI/COM
//! glue with no arithmetic. Total genuinely pure, testable arithmetic found
//! across all four files: under 40 lines, all from one file.

/// `roundUp(value, powerOf2Alignment)` (`rt64_buffer_uploader.cpp:15-17`): a
/// free function in the source, not a method. Rounds `value` up to the
/// nearest multiple of `power_of_2_alignment` via the bitmask trick, which
/// is only correct when `power_of_2_alignment` is an exact power of 2 --
/// see the module doc's "Admitted domain" for the documented-but-unchecked
/// precondition and its frontier at `0` / non-power-of-2 inputs.
pub fn round_up_pow2(value: u64, power_of_2_alignment: u64) -> u64 {
    let mask = power_of_2_alignment.wrapping_sub(1);
    value.wrapping_add(mask) & !mask
}

/// `updateResources`'s growth formula (`rt64_buffer_uploader.cpp:95`):
/// `std::max(uint64_t((requiredSize * 3) / 2), BlockAlignment)`, isolated
/// as its own function for testability of the multiply-then-divide
/// truncation and the `max`-floor step, in that literal order. The
/// `BlockAlignment` constant (`256` in the source) is threaded as a
/// parameter rather than hardcoded, since it is a named `const` in the
/// source, not a magic literal baked into the formula.
pub fn grown_capacity(required_size: u64, block_alignment: u64) -> u64 {
    let scaled = required_size.wrapping_mul(3) / 2;
    scaled.max(block_alignment)
}

/// `updateResources`'s reallocation-skip comparison
/// (`rt64_buffer_uploader.cpp:87`): `bufferPair.allocatedSize >=
/// requiredSize`. Returns `true` when the existing `allocated_size` already
/// covers `required_size` without needing to grow (an exact fit counts as
/// "fits" -- see the module doc's "Admitted domain" for the boundary
/// tests).
pub fn fits_without_growth(allocated_size: u64, required_size: u64) -> bool {
    allocated_size >= required_size
}

/// `BufferUploader::Upload::valid()` (`rt64_buffer_uploader.cpp:21-23`):
/// `(srcData != nullptr) && (srcDataIndexRange.second >
/// srcDataIndexRange.first)`. `src_data_is_null` stands in for the C++
/// `srcData != nullptr` pointer check (this module carries no pointer/slice
/// type to check non-null-ness of directly -- callers pass whether their
/// own source data handle is present). The range comparison is strict `>`,
/// not `>=` -- see the module doc's "Admitted domain".
pub fn upload_is_valid(src_data_is_null: bool, range_first: u64, range_second: u64) -> bool {
    !src_data_is_null && (range_second > range_first)
}

/// The offset/size pair computed identically at both
/// `threadUpload` (`rt64_buffer_uploader.cpp:74-76`) and
/// `commandListCopyResources` (`rt64_buffer_uploader.cpp:145-146`):
/// `srcOffset = indexRange.first * stride`; `srcSize = (indexRange.second -
/// indexRange.first) * stride`. Returns `(src_offset, src_size)`. Callers
/// are expected to have already checked [`upload_is_valid`] (`range_second
/// > range_first`); this function does not re-check that precondition,
/// matching the source -- see the module doc's underflow/wraparound bullet
/// for what `range_second < range_first` produces here.
pub fn upload_span(range_first: u64, range_second: u64, stride: u64) -> (u64, u64) {
    let src_offset = range_first.wrapping_mul(stride);
    let src_size = range_second.wrapping_sub(range_first).wrapping_mul(stride);
    (src_offset, src_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- round_up_pow2 -----------------------------------------------------

    #[test]
    fn round_up_pow2_of_a_value_already_aligned_is_a_no_op() {
        // 256 is already a multiple of 256: (256 + 255) & !255 = 511 & !255
        // = 256.
        assert_eq!(round_up_pow2(256, 256), 256);
    }

    #[test]
    fn round_up_pow2_of_a_value_one_over_alignment_rounds_up_to_the_next_multiple() {
        // 257: (257 + 255) & !255 = 512 & !255 = 512.
        assert_eq!(round_up_pow2(257, 256), 512);
    }

    #[test]
    fn round_up_pow2_of_zero_value_is_zero() {
        // (0 + 255) & !255 = 255 & 0xFFFF...FF00 = 0.
        assert_eq!(round_up_pow2(0, 256), 0);
    }

    #[test]
    fn round_up_pow2_of_a_value_one_under_alignment_rounds_up_to_alignment() {
        // 255: (255 + 255) & !255 = 510 & !255 = 256.
        assert_eq!(round_up_pow2(255, 256), 256);
    }

    #[test]
    fn round_up_pow2_with_alignment_one_is_always_a_no_op() {
        // mask = 0, so value & !0 == value, for any value.
        assert_eq!(round_up_pow2(0, 1), 0);
        assert_eq!(round_up_pow2(12345, 1), 12345);
        assert_eq!(round_up_pow2(u64::MAX, 1), u64::MAX);
    }

    #[test]
    fn round_up_pow2_of_a_zero_alignment_is_the_sources_documented_frontier() {
        // alignment = 0: mask = 0u64.wrapping_sub(1) = u64::MAX.
        // value.wrapping_add(u64::MAX) & !u64::MAX == (value - 1) & 0 == 0,
        // for every value (not a divide-by-zero panic -- there is no
        // division in this formula).
        assert_eq!(round_up_pow2(0, 0), 0);
        assert_eq!(round_up_pow2(12345, 0), 0);
        assert_eq!(round_up_pow2(u64::MAX, 0), 0);
    }

    #[test]
    fn round_up_pow2_of_a_non_power_of_2_alignment_is_the_sources_documented_frontier() {
        // alignment = 100 (not a power of 2): mask = 99 = 0b0110_0011.
        // !mask clears only those low bits, not "round to a multiple of
        // 100" -- this is the silent-wrong-answer frontier the module doc
        // names, documented with a hand-computed value, not a correctness
        // claim.
        // value = 150: (150 + 99) & !99 = 249 & !99.
        // 249 = 0b1111_1001, !99 (low byte) = 0b1001_1100.
        // 0b1111_1001 & 0b1001_1100 = 0b1001_1000 = 152.
        assert_eq!(round_up_pow2(150, 100), 152);
    }

    #[test]
    fn round_up_pow2_large_alignment_matches_hand_computed_value() {
        // alignment = 4096, value = 5000.
        // (5000 + 4095) & !4095 = 9095 & !4095.
        // 4096*2 = 8192, 4096*3 = 12288; 9095 is between them, so rounds
        // up to 8192.
        assert_eq!(round_up_pow2(5000, 4096), 8192);
    }

    #[test]
    fn round_up_pow2_wraps_on_overflow_near_u64_max() {
        // value = u64::MAX, alignment = 256: mask = 255.
        // value.wrapping_add(255) wraps around to 254 (u64::MAX + 255 mod
        // 2^64 == 254, since u64::MAX + 1 == 0).
        // 254 & !255 == 254 & 0xFFFF...FF00 == 0.
        let mask: u64 = 255;
        let expected = (u64::MAX.wrapping_add(mask)) & !mask;
        assert_eq!(round_up_pow2(u64::MAX, 256), expected);
        assert_eq!(round_up_pow2(u64::MAX, 256), 0);
    }

    // -- grown_capacity ------------------------------------------------------

    #[test]
    fn grown_capacity_matches_hand_computed_multiply_by_three_divide_by_two() {
        // required_size=4: (4*3)/2=6, max(6,256)=256 (floor dominates).
        assert_eq!(grown_capacity(4, 256), 256);
        // Use a small block_alignment (1) to observe the raw formula
        // without the floor masking it.
        assert_eq!(grown_capacity(4, 1), 6);
        assert_eq!(grown_capacity(10, 1), 15);
    }

    #[test]
    fn odd_required_size_seven_truncates_down_not_up() {
        // (7*3)/2 = 21/2 = 10 (truncating, not 10.5 rounded any other
        // way).
        assert_eq!(grown_capacity(7, 1), 10);
    }

    #[test]
    fn odd_required_size_five_truncates_down_not_up() {
        // (5*3)/2 = 15/2 = 7 (truncating, not 7.5 -> 8).
        assert_eq!(grown_capacity(5, 1), 7);
    }

    #[test]
    fn odd_required_size_one_truncates_to_one() {
        // (1*3)/2 = 3/2 = 1 (truncating, not 1.5 -> 2).
        assert_eq!(grown_capacity(1, 1), 1);
    }

    #[test]
    fn required_size_zero_with_floor_of_one_returns_the_floor() {
        // (0*3)/2 = 0, max(0, 1) = 1 -- the block_alignment floor still
        // applies even to a zero required_size.
        assert_eq!(grown_capacity(0, 1), 1);
    }

    #[test]
    fn required_size_zero_with_floor_of_zero_is_zero() {
        // (0*3)/2 = 0, max(0, 0) = 0 -- only a zero floor lets this
        // through as zero.
        assert_eq!(grown_capacity(0, 0), 0);
    }

    #[test]
    fn grown_capacity_floor_applies_before_any_alignment_rounding() {
        // required_size = 10: (10*3)/2 = 15, which is below the 256
        // BlockAlignment floor -- so the *floor*, not the formula, is what
        // produces the raw grown_capacity value here. This is distinct
        // from round_up_pow2, which is a separate step applied afterward
        // by the caller (see round_up_pow2_composes_after_grown_capacity_floor).
        assert_eq!(grown_capacity(10, 256), 256);
    }

    #[test]
    fn grown_capacity_formula_dominates_when_it_exceeds_the_floor() {
        // required_size = 1000: (1000*3)/2 = 1500, which exceeds the 256
        // floor -- so the formula's result, not the floor, wins.
        assert_eq!(grown_capacity(1000, 256), 1500);
    }

    #[test]
    fn round_up_pow2_composes_after_grown_capacity_floor() {
        // Full two-step composition as the source performs it:
        // grown_capacity(10, 256) = 256 (floor wins), then
        // round_up_pow2(256, 256) = 256 (already aligned).
        let grown = grown_capacity(10, 256);
        let final_size = round_up_pow2(grown, 256);
        assert_eq!(grown, 256);
        assert_eq!(final_size, 256);

        // A required_size whose formula result exceeds the floor but is
        // not yet 256-aligned: required_size = 200.
        // (200*3)/2 = 300, max(300,256) = 300.
        // round_up_pow2(300, 256): (300+255) & !255 = 555 & !255 = 512.
        let grown2 = grown_capacity(200, 256);
        let final_size2 = round_up_pow2(grown2, 256);
        assert_eq!(grown2, 300);
        assert_eq!(final_size2, 512);
    }

    #[test]
    fn grown_capacity_wraps_on_multiply_overflow_near_u64_max() {
        let required = u64::MAX / 2; // large enough that *3 overflows.
        let expected_scaled = required.wrapping_mul(3) / 2;
        assert_eq!(grown_capacity(required, 1), expected_scaled.max(1));

        // Hand-computed, so a shared error in the recomputation above cannot
        // hide: required = 2^63 - 1 = 9223372036854775807; the exact product
        // 3 * (2^63 - 1) = 27670116110564327421 exceeds 2^64 by
        // 9223372036854775805, which is what `wrapping_mul` leaves behind;
        // 9223372036854775805 / 2 truncates to 4611686018427387902, already
        // above the block_alignment floor of 1.
        assert_eq!(grown_capacity(required, 1), 4_611_686_018_427_387_902);
    }

    // -- fits_without_growth --------------------------------------------------

    #[test]
    fn fits_without_growth_is_true_at_the_exact_boundary() {
        // allocated_size == required_size: source's `>=` treats this as
        // fitting (no growth needed).
        assert!(fits_without_growth(256, 256));
    }

    #[test]
    fn fits_without_growth_is_false_one_byte_over() {
        // allocated_size one less than required_size: must grow.
        assert!(!fits_without_growth(255, 256));
    }

    #[test]
    fn fits_without_growth_is_true_when_allocated_size_is_generously_larger() {
        assert!(fits_without_growth(1_000_000, 256));
    }

    #[test]
    fn fits_without_growth_zero_required_size_always_fits() {
        assert!(fits_without_growth(0, 0));
        assert!(fits_without_growth(256, 0));
    }

    // -- upload_is_valid -------------------------------------------------------

    #[test]
    fn upload_is_valid_false_when_range_is_exactly_equal() {
        // second == first: strict `>` fails, so this is NOT valid (a
        // zero-length range).
        assert!(!upload_is_valid(false, 5, 5));
    }

    #[test]
    fn upload_is_valid_true_when_range_is_one_wide() {
        assert!(upload_is_valid(false, 5, 6));
    }

    #[test]
    fn upload_is_valid_false_when_range_is_inverted() {
        assert!(!upload_is_valid(false, 6, 5));
    }

    #[test]
    fn upload_is_valid_false_when_src_data_is_null_even_with_a_valid_range() {
        assert!(!upload_is_valid(true, 0, 10));
    }

    #[test]
    fn upload_is_valid_false_when_both_conditions_fail() {
        assert!(!upload_is_valid(true, 5, 5));
    }

    #[test]
    fn upload_is_valid_true_for_a_wide_range_with_non_null_data() {
        assert!(upload_is_valid(false, 0, u64::MAX));
    }

    // -- upload_span -------------------------------------------------------

    #[test]
    fn upload_span_zero_offset_full_stride() {
        // range = [0, 4), stride = 8: offset = 0*8=0, size=(4-0)*8=32.
        assert_eq!(upload_span(0, 4, 8), (0, 32));
    }

    #[test]
    fn upload_span_nonzero_offset() {
        // range = [2, 5), stride = 4: offset = 2*4=8, size=(5-2)*4=12.
        assert_eq!(upload_span(2, 5, 4), (8, 12));
    }

    #[test]
    fn upload_span_single_element_range() {
        // range = [3, 4), stride = 16: offset=48, size=16.
        assert_eq!(upload_span(3, 4, 16), (48, 16));
    }

    #[test]
    fn upload_span_zero_stride_yields_zero_offset_and_zero_size() {
        assert_eq!(upload_span(5, 10, 0), (0, 0));
    }

    #[test]
    fn upload_span_zero_width_range_yields_zero_size_but_nonzero_offset() {
        // Not a valid Upload per upload_is_valid (second == first), but
        // upload_span itself does not check that -- matching the source,
        // which relies on the caller having checked first.
        assert_eq!(upload_span(3, 3, 8), (24, 0));
    }

    #[test]
    fn upload_span_inverted_range_wraps_on_subtraction() {
        // second < first: (second - first) underflows/wraps, matching the
        // source's unchecked size_t subtraction. Not a valid Upload per
        // upload_is_valid, but upload_span reproduces the wraparound if
        // called anyway.
        let (_offset, size) = upload_span(10, 4, 1);
        // wrapping_sub(4, 10) on u64 = u64::MAX - 5.
        let expected_diff = 4u64.wrapping_sub(10);
        assert_eq!(size, expected_diff.wrapping_mul(1));
        assert_eq!(size, u64::MAX - 5);
    }

    #[test]
    fn upload_span_large_stride_matches_hand_computed_values() {
        // range = [16, 20), stride = 1024: offset=16*1024=16384,
        // size=(20-16)*1024=4096.
        assert_eq!(upload_span(16, 20, 1024), (16384, 4096));
    }

    #[test]
    fn upload_span_offset_and_size_wrap_on_multiply_overflow() {
        let first = u64::MAX / 2;
        let stride = 3u64;
        let expected_offset = first.wrapping_mul(stride);
        let (offset, _size) = upload_span(first, first, stride);
        assert_eq!(offset, expected_offset);

        // Hand-computed for the same reason as above: 3 * (2^63 - 1) =
        // 27670116110564327421, which is 2^64 + 9223372036854775805, so the
        // wrapped offset is 9223372036854775805.
        assert_eq!(offset, 9_223_372_036_854_775_805);
    }
}
