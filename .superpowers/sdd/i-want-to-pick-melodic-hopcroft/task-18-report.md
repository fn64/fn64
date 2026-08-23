# Task 18 report: LoadBlock (0x33) DxT row-advance for row >= 1 (RGBA16/CI8)

## Summary

The six `gen-loadblock-deep-*` parity cases refused in wgpu while RT64 and
angrylion rendered them. Root cause was **not** a missing DxT row-advance on
the write side (fn64 already places LoadBlock words at their DxT-advanced
destinations, byte-identical to RT64 — verified below). It was the **reader
refusing the words the DxT sweep skips**: a LoadBlock with DXT >= 0x800
advances the destination by more than one TMEM word per source word, so it
writes scattered words (DXT=0x800 -> words 0, 2, 4, 6) and leaves the words
between them unwritten. A render tile that re-describes the block with a
`line` that reads a skipped word hit fn64's validity refusal
(`InvalidTexelByte`), where hardware and both oracles read those words back as
their prior (zero) content.

Fix: after a LoadBlock stages its scattered words, mark the whole contiguous
swept footprint valid, back-filling the skipped words with their
zero-initialised storage byte. Scoped to `TmemLoadShape::Block` so LoadTile's
strict refusal (which guards the WM2000 origin-term defect) is untouched.

## Where the refusal was

`crates/fn64-render-wgpu/src/tmem/read.rs::read_valid_byte` returns
`PhysicalTexelReadError::InvalidTexelByte` for any TMEM byte whose
latest-complete-word touch did not define it. The raw-triangle executor
(`crates/fn64-render-wgpu/src/targets/raw_triangle.rs`) samples through the
shared `sample_point`, so the refusal surfaced as
`texture rectangle texel fetch failed at pixel (x, 1): physical TMEM texel
byte 0xNN is invalid` — always at **tile row 1**, the first row the DxT sweep
advances into.

The write side was already correct: `project_tmem_transfer_word`'s Block arm
(`crates/fn64-render-wgpu/src/tmem/types.rs`) computes
`advance = (word * dxt) >> 11`, `destination = tmem + word + advance * line`,
which matches RT64's `loadToTMEMCommon<BLOCK>`
(`third_party/rt64/src/hle/rt64_rdp.cpp:399-471`) exactly. Simulated for all
six cases, fn64 and RT64 agree on every written word address (e.g. dxt800 ->
0,2,4,6; dxt400/line2 -> 0,1,4,5,8,9,12,13). So the holes are real in RT64
too; RT64 reads them from zeroed TMEM.

## The fix

- `crates/fn64-render-wgpu/src/tmem/physical.rs`: new crate-private
  `StagedTmemTransaction::mark_block_footprint_valid(low_word, high_word)` —
  marks each byte of every TMEM word in `[low, high]` valid, leaving already
  defined bytes untouched and reading skipped words back as their zero
  storage. Invisible to the sealed load's effects and proposal digest, which
  are keyed on the destination access ranges alone (`project_load` /
  `proposal_identity`), so it only widens what the in-packet reader may
  sample.
- `crates/fn64-render-wgpu/src/production.rs` (the live raw-DPC path): after
  the block's words are staged, when `load.shape() == Block`, compute the
  contiguous footprint `[min, max]` over the Linear destination words and call
  `mark_block_footprint_valid`.
- `crates/fn64-render-wgpu/src/tmem/execute/load_block.rs` (the standalone
  executor path): same fill, so the executor is self-consistent, plus the new
  unit test.
- `crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`:
  a `FN64_ONLY=<substring>` triage filter for fast targeted runs (the full
  generated corpus is slow and prone to a wgpu/Metal stall). No effect on the
  gate.

CI8 uses the same Block path; the fill covers its 8bpp footprint identically.

## Before / after (FN64_GENERATE=1, angrylion ground truth)

| case | before | after |
|------|--------|-------|
| gen-loadblock-deep-rgba16-dxt400-triangle | wgpu refused (`InvalidTexelByte 0x014` @ (0,1)) | **diff 0**, pass-all-match-hardware |
| gen-loadblock-deep-rgba16-dxt800-triangle | wgpu refused (`0x00c` @ (0,1)) | **diff 0**, pass-all-match-hardware |
| gen-loadblock-deep-rgba16-dxt-fractional-triangle | wgpu refused (`0x01c` @ (4,1)) | **diff 0**, pass-all-match-hardware |
| gen-loadblock-deep-ci8-dxt400-triangle | wgpu refused (`0x014` @ (0,1)) | wgpu completes; **diff 9** (residual, see below) |
| gen-loadblock-deep-ci8-dxt800-triangle | wgpu refused (`0x00c` @ (0,1)) | wgpu completes; **diff 8** (residual) |
| gen-loadblock-deep-ci8-dxt-fractional-triangle | wgpu refused (`0x014` @ (0,1)) | wgpu completes; **diff 8** (residual) |

All three RGBA16 cases reach byte-exact parity with angrylion.

## Residual (honest, NOT suppressed): CI8

The three CI8 cases no longer refuse — wgpu now completes and produces output
**byte-identical to RT64** (wgpu_vs_angrylion diff == rt64_vs_angrylion diff:
9, 8, 8). Their first divergence from angrylion is at **pixel (0,0), tile
row 0** — angrylion `0x0843` vs wgpu/RT64 `0x07c1` — which is *before* any
row-advance and therefore outside this task's scope. It is a pre-existing
shared CI8 + TLUT color-decode divergence between the RT64-lineage HLE (which
fn64 matches by design — see memory `c-oracle-is-the-parity-target`) and
bit-accurate angrylion; it is present at row 0 and identical across dxt400 /
dxt800 / fractional, confirming the DxT row-advance itself is correct for CI8
(wgpu tracks RT64 across every row). This is not the fractional-tail residual
the brief anticipated; it is a row-0 palette-decode residual, and it is the
same 8-9 pixels RT64 itself diverges by.

Because these CI8 cases are `Authority`-neutral generated cases (not
`rt64-authoritative`), they do not enter the gate's pass/fail accounting; the
gate compares the hand corpus only.

## Verification

- `cargo build -p fn64-render-conformance --features parity-runner
  --bin fn64-render-conformance-parity-runner --offline`: OK.
- `cargo test -p fn64-render-wgpu --lib`: **4884 passed, 0 failed** (includes
  the WM2000 origin-term guard and every `InvalidTexelByte` refusal test —
  the fix is Block-scoped and does not weaken LoadTile strictness).
- New unit test
  `tmem::execute::load_block::tests::dxt_row_advance_leaves_skipped_words_valid_as_zero`:
  stages a DXT=0x800 block, asserts word 0 holds its data, the **skipped
  destination word 1 reads back valid as zero**, and word 2 holds the
  row-advanced (odd-row-exchanged) second word. Fails if the back-fill is
  reverted.
- Gate: `check_rt64_parity.py < gate.json` — see below.

## Unit test

`crates/fn64-render-wgpu/src/tmem/execute/load_block.rs`:
`dxt_row_advance_leaves_skipped_words_valid_as_zero`.

## Gate result

`python3 scripts/check_rt64_parity.py < gate.json` -> **PASS** (exit 0):

```
RT64 PARITY GATE: PASS -- 33/37 rt64-authoritative cases byte-identical to the RT64 C++ oracle
  scissor-narrower-than-rect: RT64_DEFECT asserted differs
  textured-rect-yuv16: FN64_CAPABILITY_GAP asserted one-refused
  perspective-textured-triangle-negative-w: BROKEN_FIXTURE asserted differs
  two-cycle-textured: FN64_CAPABILITY_GAP asserted differs
```

The four exceptions are the pre-existing accounted entries, unchanged. Both
hand-corpus LoadBlock cases (`textured-rect-loadblock-linear`,
`textured-rect-loadblock-dxt-row-advance`) stay `verdict: identical` /
`wgpu_matches_key: true` — the footprint back-fill marks only words the
render tile does not sample, so their output is byte-for-byte unchanged.

Environmental note: the full-corpus runner intermittently stalls in
wgpu/Metal device init (0% CPU, no progress) — observed at different case
counts across runs, independent of this change (it stalled before any edit
too). A re-run gets through; the gate above is a clean completed run.

## Commit

`fix(render-wgpu): LoadBlock (0x33) DxT row-advance for row >= 1 (RGBA16/CI8)`
— hash recorded on commit (see final message).
