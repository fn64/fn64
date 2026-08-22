# RT64 upstream observations

Behaviors in the pinned MIT RT64 source
(`5473732a822a4423b5696e7cb18fecc425a59875`, `docs/RT64-PORT-AUTHORITY.md`)
that fn64's port reproduces literally, each pinned by a characterization
test. Nothing here is fixed in fn64: the port's job is to match the pinned
source, and a test asserting current behavior is what will fail loudly if
we later choose to diverge.

**Confidence is stated per row and earned separately.** "Defect" means the
code cannot do what its own surrounding code implies it should. It does not
mean "differs from N64 hardware" — that needs hardware evidence we do not
have, and is a separate question from parity with RT64.

## Confirmed defects

### 1. `gEXPopMatrixGroup{,N}` loses `proj` in transit

- Encoder: `include/rt64_extended_gbi.h:334-344` places `PARAM(proj, 1, 8)`
  in `G_EX_COMMAND1`'s second argument.
- `G_EX_COMMAND1` (`:168`) forwards that argument to `G_EX_WRITECOMMAND`
  (`:162-166`), which assigns `_word1 -> word1` with no swap.
- Decoder: `src/gbi/rt64_gbi_extended.cpp:159` reads
  `proj = p0(8, 1)`, and `p0` reads **w0** (`src/gbi/rt64_gbi.cpp:32-34`).

Packing `proj = 1` and decoding yields `0`. The neighbouring field is
correct — `popCount = p1(0, 8)` matches `PARAM(count, 8, 0)` in word1 — so
one field of the pair reads the right word and the other does not.

**Why this is a defect and not a convention:** the two fields are written by
one expression into one word, and read back by two lines that disagree about
which word that was.

**Impact:** any caller of `gEXPopMatrixGroup`/`gEXPopMatrixGroupN` that sets
`proj` gets `proj = 0` behavior. Unknown whether any shipped title uses the
extended GBI path.

**Pinned by:** `round_trip_pop_matrix_group_proj_disagrees_between_encoder_and_decoder`
(`crates/fn64-render-wgpu/src/rt64_gbi_extended_decode.rs`).

**Remediation, post-parity:** one-line decoder change to `p1(8, 1)`. Cheap.
Requires deciding whether fn64 diverges from RT64 or waits for upstream.

### 2. `HistogramClearCS` clears 9 of 64 bins

`src/shaders/HistogramClearCS.hlsl` dispatches `[numthreads(8,8,1)]` and
stores with `LuminanceHistogram.Store(threadId.x * threadId.y, 0)` — the
**product** of the coordinates. `NUM_HISTOGRAM_BINS` is 64
(`LuminanceHistogramCS.hlsl:10`).

Two independent errors:

- The product of two values in `0..7` yields only 26 distinct results, and
  15 of the 64 threads compute `0`.
- `Store` takes a **byte** address, so bin *i* lives at byte `i * 4`. Only
  the 4-aligned products land on a bin boundary.

Net: 9 of 64 bins are cleared; the rest keep stale counts, and several
writes land unaligned inside a bin.

**Impact:** auto-exposure reads a histogram that is never fully cleared,
so luminance adaptation carries stale frame data. Visible as exposure that
drifts or fails to settle.

**Pinned by:** documented in `crates/fn64-render-wgpu/src/rt64_luminance_histogram.rs`
(the clear pass itself is out of that card's scope, so it is recorded, not
ported).

**Remediation, post-parity:** `Store((threadId.x * 8 + threadId.y) * 4, 0)`,
or dispatch 64x1x1 and use `threadId.x * 4`. Requires a hardware A/B to
confirm the visible effect before diverging.

### 3. `G_EX_COMMAND4` cannot compile if instantiated

`include/rt64_extended_gbi.h:192-202`. Three deviations from its own
siblings `G_EX_COMMAND1/2/3` (`:168-191`):

- It declares `GfxCommand *_cmd` but passes `cmd_` to `G_EX_WRITECOMMAND`.
  `cmd_` is undefined; its only four occurrences in the tree are these
  lines.
- Siblings offset each write (`_cmd + 0`, `+ 1`, `+ 2`). COMMAND4 passes the
  same unoffset name four times, so even with the name corrected, three of
  the four writes would clobber each other.
- It carries three redundant `(void)(cmd);` statements siblings lack.

**Impact: none.** No macro invokes it, so it is never expanded and never
compiled. The defect is latent.

**Pinned by:** named in `crates/fn64-render-wgpu/src/rt64_extended_gbi.rs`'s
Nonclaims as deliberately not ported.

**Remediation:** upstream's to make. fn64 ports no caller, so there is
nothing to fix here.

## Dead or unreachable code — no behavioral effect

Recorded so a future reader does not assume a code path exists.

### 4. `insertRegionsTMEM`'s wraparound branches are unreachable

`src/hle/rt64_framebuffer_manager.cpp:517-566` sets
`tmemBarrier = tmemEnd` and `tmemCursor = tmemEnd` on consecutive lines, so
they are equal at loop entry. The first branch requires
`tmemCursor > tmemBarrier` and can never fire; `wordsLeft <= tmemCursor`
always holds, so the `else` runs and zeroes `wordsLeft` after exactly one
iteration. Verified algebraically and by a 200,000-sample randomized sweep.

**Consequence worth knowing:** the function never clamps or splits a region
to TMEM bounds. A region ending at the boundary, crossing it, or longer than
TMEM itself is emitted as one whole unclamped node.

**Pinned by:** `crates/fn64-render-wgpu/src/rt64_tmem_regions.rs`, which
ports the unreachable branches literally.

### 5. `TextureMap::incrementLock` / `decrementLock` declared, never defined

Declared at `src/render/rt64_texture_cache.h:148-149`. No definition exists;
only the unrelated `TextureCache::` pair at `.h:254-255` has bodies
(`.cpp:1721,1726`). Legal C++ while uncalled.

**Pinned by:** named in `crates/fn64-render-wgpu/src/rt64_texture_map_lru.rs`'s
Nonclaims.

### 6. `insertRegionsTMEM`'s `byteShift` is computed and never read

Same function, `:528`. Ported as an intentionally unused local.

### 7. 5-7 byte framebuffer swaps are skipped entirely

`src/hle/rt64_framebuffer.cpp:150-162` branches on
`if (bytesToSwap >= 4) ... else`. The `>= 4` branch computes
`wordsToSwap = bytesToSwap / 4`, truncating; the `i ^ 3` tail runs only in
the `else`, when `bytesToSwap < 4`. For 5-7 bytes the trailing 1-3 bytes are
swapped by neither branch.

Whether this is reachable depends on caller geometry, which this card did
not establish — so it is recorded here rather than claimed as a live defect.

**Pinned by:** `word_swap_non_multiple_of_four_truncates_via_integer_division`
(`crates/fn64-render-wgpu/src/rt64_framebuffer_geometry.rs`).

## Asymmetries that are probably deliberate — do not "fix"

These read like bugs and are not. They are recorded so nobody tidies them.

### 8. `lerpTransforms` inverts the slerp weight

`src/common/rt64_math.cpp:447` calls `slerp(a, b, 1.0f - weight)` three
lines above `:450`'s `lerp(a, b, weight)`. At `weight == 0` the slerp path
evaluates toward `b` while the lerp path evaluates toward `a`.

Plausibly intentional: slerp conventions differ on argument order, and a
caller tuned against this behavior would break if it were "corrected."

### 9. `RSPSmoothNormal` accumulates one face normal per welding corner

`src/shaders/RSPSmoothNormalCS.hlsl:27-41`. The inner corner loop has no
`break`, so a triangle with two or three corners inside the weld radius
contributes its face normal two or three times. This is weighting, not
double-counting, and deduplicating it would change every smoothed normal.

### 10. `PresetDrawCall::matches` rejects Combine only when BOTH halves differ

`src/preset/rt64_preset_draw_call.cpp:166`:

```cpp
if ((key.colorCombiner.L != otherKey.colorCombiner.L) &&
    (key.colorCombiner.H != otherKey.colorCombiner.H)) return false;
```

It is `&&`. An L-only or H-only Combine difference does NOT reject, so two
draw calls with different colour combiners can match. The OtherMode check
three lines below uses two independent masked comparisons, either of which
rejects on its own — opposite semantics for adjacent fields in the same
function.

Plausibly deliberate: a preset that should apply across a combiner variant
would want the looser test. But it is equally consistent with a `&&` that
should have been `||`, and nothing in the source says which.

**Impact:** a preset matches more draw calls than a strict reading would
predict. Whether that is desired behavior or a latent over-match is not
determinable from the source alone.

**Pinned by:** three tests in
`crates/fn64-render-wgpu/src/rt64_preset_draw_call_match.rs` (L-only
accepts, H-only accepts, both-differ rejects), so a later "make these
consistent" edit fails loudly.

**Remediation:** none proposed. Resolving this needs upstream intent or a
preset corpus showing which behavior real presets rely on.

## Withdrawn — not findings

### `GaussianFilterRGB3x3CS` interior weights sum to 1.0000020

Reported earlier in this effort as a "DC gain finding." It is ordinary f32
rounding residue in a hand-tuned weight table; essentially no float weight
table sums to exactly 1.0. Recorded here only to retract it. The port
preserves the literal constants regardless, which is correct for a different
reason: they are the values RT64 uses.

## Method note

Every row above was re-verified directly against the pinned source, not
carried forward from a subagent's report. Three claims were downgraded in
that pass: the gaussian sum (withdrawn entirely), the 5-7 byte swap
(reachability not established), and `G_EX_COMMAND4` (upgraded to a certain
defect but with impact corrected to none). Earlier session summaries
described all of these as "defects found"; that was an overstatement of
confidence, and this document is the corrected record.
