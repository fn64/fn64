# Task 36 — rightmost-pixel noise: reproduce + fix (texrect right edge)

## Verdict

**Reproduced: NO.** Candidate (a) — a right-edge texel sampled PAST the tile's
loaded S extent — does **not** reproduce in the parity corpus. The wgpu texrect
sampler already computes the correct rightmost-column texel address under both
clamp and wrap addressing, matching bit-accurate angrylion (and RT64) exactly.

Per the brief's Step-1 stop condition (rightmost column matches → it's candidate
(b), live-only stale RDRAM), I did **not** force a fix. The two new corpus cases
are committed as passing regression witnesses, and a unit test pins the
right-edge texel address so the addressing math cannot silently regress.

This aligns with the user's live observation that the noise appears during the
**intro sequence** (wrestlers first shown) — a specific content frame, not a
property of the texel-address arithmetic. That is the signature of candidate (b):
a live column whose bytes are scene-correlated-but-uncovered / stale RDRAM, which
a synthetic corpus cannot express.

## What was built (deterministic, no GUI)

Two generated-corpus cases in the FN64_GENERATE triage lane
(`fn64-render-conformance-parity-runner.rs`), each a one-cycle texrect drawn
**one pixel wider than the loaded tile** so the rightmost destination column
samples texel index `TEXTURE_WIDTH` (4) — one past the loaded `[0, 3]` S extent —
with BI_LERP_0 set (via `set_bilerp0`):

- `gen-texrect-right-edge-overread-clamp` — `mask_s == 0` (forced clamp)
- `gen-texrect-right-edge-overread-wrap`  — `mask_s == 2`, WRAP mode

The two variants **discriminate**: they produce a different rightmost texel, so a
right-edge over-read would surface as a wgpu-vs-angrylion divergence no matter
which addressing the sampler produced.

## The measured rightmost columns (3-way, row 0)

Bit-accurate angrylion is ground truth. Columns 0..7 of row 0, all three engines:

```
gen-texrect-right-edge-overread-clamp
  col0..3: 0x07c1 0xf801 0x7fff 0x003f    (loaded texels)
  col4:    0xffff                          (STALE — outside the covered set)
  col5:    0x7fff                          (discriminating column: CLAMP)
  -> angrylion == wgpu == rt64 on EVERY column

gen-texrect-right-edge-overread-wrap
  col0..3: 0x07c1 0xf801 0x7fff 0x003f
  col4:    0xffff
  col5:    0xf801                          (discriminating column: WRAP -> texel 0)
  -> angrylion == wgpu == rt64 on EVERY column
```

`wgpu_vs_angrylion_diff_pixels = 0` for both; classification
`pass-all-match-hardware`. The clamp/wrap discriminating column (`0x7fff` vs
`0xf801`) proves the case is live and non-degenerate, and wgpu matches angrylion
in both — so the over-read bug is not present in this path.

## Root cause (of the non-reproduction)

The texrect addressing seam is correct:
- `texrect.rs::step_axis` (`s_at`) is an exact rational step; at one texel per
  pixel the rightmost over-wide column resolves to the past-extent texel index,
  as intended for the test.
- `tmem/sample.rs::address_axis_texel` then maps that index correctly: `mask_s
  == 0` (or clamp mode) clamps to `[0, dim-1]` — the last loaded texel — and the
  WRAP arm folds it back with `coordinate & low_mask`. Neither ever reads texel 4.

So the residual live noise the user sees is **candidate (b)**: an uncovered /
stale-RDRAM rightmost column in a live content frame, not a wrong texel fetch.

## Gates

- New cases divergent→identical on the rightmost column: **N/A — they were
  already identical** (that IS the finding). Both are `pass-all-match-hardware`.
- No regression: `python3 scripts/check_rt64_parity.py` → **PASS 33/37**
  rt64-authoritative, the 4 accounted exceptions unchanged.
- Full generated triage: 103 cases, both new cases pass; pre-existing
  counts unchanged.
- Full wgpu lib suite: **4888 passed, 0 failed.**
- Unit test pinning the right-edge texel address:
  `right_edge_one_past_extent_addresses_within_the_loaded_row` (clamp→col 3,
  wrap→col 0) — **passes.**

## Recommendation for the parent

Take a live 480-wide frame dump of the **intro sequence** (wrestlers first
shown), inspect columns 476..479 across rows. Repeated wrong-but-plausible color
→ still a texrect issue (unlikely, given this proof); varied,
scene-uncorrelated bytes → stale RDRAM in an uncovered column (candidate (b)),
fixed at the fill/clear seam (does the frame's initial fill cover column 479?),
not the sampler.

## Files touched

- `crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`
  — two generated cases + `right_edge_overread_rect` builder + `set_tile_wrap_s`.
- `crates/fn64-render-wgpu/src/tmem/sample.rs` — unit test pinning the right-edge
  texel address.
