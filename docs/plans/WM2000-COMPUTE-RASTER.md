# WM2000 exact compute-raster execution plan

Status: active architecture pivot, 2026-08-23.

## Why this is now the shortest path

The retained unprofiled lane is approximately 24.3 ms mean and 38.8--39.1 ms
p95 per drawn frame. Reliable 30 Hz still needs about 5.8 ms from p95; the
extension-headroom bar needs about 14 ms and a p95 at or below 25 ms.

Fresh phase attribution places 83.4% of slow-pump excess in raw-DPC RDP work.
Over-budget drawn frames average 26.862 ms of RDP work versus 17.135 ms within
budget, while presentation differs by only 0.023 ms. The draw census further
shows that eight textured, perspective, depth-free RGBA16 state keys cover the
observed triangle workload and that the five leading keys account for 98.4%
of raster time.

The CPU alternatives have now failed their kill tests: prepared two-cycle
combining saved only about 0.45 ms p95, incremental texture planes were slower,
lowering the Rayon cutoff was slower in both pair orders, and caching coverage
sample intervals was also slower. Direct bracket experiments retained outside
git additionally leave approximately 99% of the synthetic raster cost after
sampling, combining, blending, and coordinate math are removed. The remaining
cost scales with scalar fragment visits. Another selector or bounds-check
micro-optimization cannot credibly supply 14 ms.

## Execution shape

The production path will retain one packed RGBA16 storage buffer per resident
color target. A compute submission owns a target pixel, not a triangle:

```text
ordered packet commands
        |
        v
typed admitted draw records + TMEM snapshots
        |
        v
one compute invocation per target pixel
        |
        +-- visits affecting draws in command order
        +-- coverage -> attributes -> TMEM -> combine -> blend
        +-- updates its own packed RGBA16 value only
        |
        v
one bounded target readback at effect publication
```

Pixel ownership removes write races without atomics. Iterating a pixel's
affecting draws in command order preserves painter's order and framebuffer
reads. TMEM loads do not force a color-target barrier: each draw record binds
the immutable TMEM snapshot visible at its own stream position. A fill,
texrect, target change, unsupported state, or other CPU-only color command is
a typed batch boundary until that command kind has an exact device executor.
The boundary flushes once, continues through the existing CPU path, and may
start a later batch from the resulting resident bytes. No unsupported state is
silently approximated.

The existing diagnostic triangle render pipeline is not the implementation:
its target is RGBA8 at `RenderConfig` extent and its hardware interpolation and
coverage are not the guest-visible CPU raster's exact semantics. The compute
path instead uses the `SetColorImage` RGBA16 extent and a storage buffer, the
same representation proven by `targets/native_fill_rgba16.wgsl`.

## Ordered implementation units

1. **Replay receipt.** Capture a bounded, game-derived packet plus its exact
   guest-read sources outside git. Repeated replay must compare complete target
   bytes and effect digests against the CPU path on every iteration. This is
   the fast optimization loop; full linked-shell LTO is reserved for retained
   candidates. The older XBUS-only dump hook was measured to emit zero files
   on the production wgpu session route, so that route now has a bounded
   owned-submission capture tap with explicit source/range sidecars and a
   single-index RDRAM snapshot selector. A bounded live run captured 2,670
   packets; packet 2659 is the selected private receipt (1,032 command bytes,
   five full shaded+textured triangles, one matching 8 MiB RDRAM image). Its
   command bytes were identical at the same index across two runs. The
   `raw_dpc_replay` example now drives that receipt through the production
   XBUS plan/execute/guest-commit/publish seam, primes durable state from a
   captured prefix, supports suffix-window bisection, reports every lifecycle
   phase, and requires committed guest bytes to stay identical across every
   repeat. No captured command or RDRAM byte enters git.
2. **Dynamic RGBA16 target.** Generalize the proven native-fill storage-buffer
   and bounded-readback mechanism to a `SetColorImage` extent. A no-op compute
   round trip must reproduce arbitrary resident bytes exactly, including odd
   widths and row ends. The transport-only pipeline is now precreated with the
   triangle device and its host-GPU gate covers both a 5x3 odd-word tail and a
   full 320x240 target; production raster arithmetic and target persistence
   remain intentionally outside this completed substrate slice. Validation on
   the native Metal adapter passed 10 consecutive runs on 2026-08-23; the
   closed-profile WGSL validation and all 42 focused triangle-pipeline tests
   also passed.
3. **Typed batch admission.** Define a move-only batch containing the target
   identity/generation, ordered draw records, per-position TMEM snapshots, and
   the exact journal accesses it can publish. Admission initially accepts only
   the census-observed textured, perspective, depth-free RGBA16 subset. The
   first closed admission type now accepts only the leading exact census key
   (`fc5196a3/112cfe7f`, `0008acef/005041c8`): target generation, strict
   command order, each draw's committed/proposed TMEM identity, and every
   render-target journal access are sealed into a non-`Clone` batch. Scheduler
   integration and GPU consumption remain open; unsupported state still
   reaches no compute executor.
4. **Integer coverage and attributes.** Port this repository's
   `triangle_span` formulas into WGSL using explicit multiword signed arithmetic
   where WGSL lacks 64/128-bit integers. Exhaustively compare every covered
   sample and plane value against the CPU implementation before color work is
   enabled. The first coverage-only slice now evaluates the exact checkerboard
   eight-subsample rule with signed 64-bit edge products represented as two
   32-bit words. Its closed WGSL validates under Naga, and three triangles
   covering live WM2000 coefficients, both major-edge polarities, and negative
   slopes matched the CPU `triangle_span::pixel_coverage` oracle for 10
   consecutive runs on the native Metal adapter on 2026-08-23. The follow-on
   slice selects the first covered subsample and evaluates all seven shade and
   texture planes with an exact signed 32-by-64-bit multiply represented in
   32-bit words. Mixed-sign and extreme coefficient fixtures matched
   `attribute_sample` and `attribute_plane` for another 10 consecutive native
   Metal runs. This completes the integer coverage-and-attributes unit; the
   differential substrate is not yet connected to production dispatch.
5. **TMEM, combiner, and blend.** Reuse the repository-owned callable WGSL
   functions where they are already CPU-differentially proven. Add packed
   RGBA16 destination decode/write and the two-cycle path required by the
   census. Every one of the eight state keys must match complete CPU target
   bytes; a mutation of any stage must fail the comparison. The first
   color-producing prototype now covers the leading one-cycle key with one
   shared immutable TMEM projection: a two-pixel invocation owns each packed
   RGBA16 storage word, visits overlapping triangles in command order, and
   reuses the existing TMEM, combiner, coverage, and framebuffer-blend WGSL
   callables. Two fully overlapping draws with distinct primitive/environment
   registers matched the CPU raw-triangle executor's complete target bytes
   for 10 consecutive native Metal runs on 2026-08-23. The oracle caught and
   rejected the initial direct-count coverage encoding before the corrected
   `count - 1` stored representation passed. The prototype is now wired to an
   explicitly enabled production replay probe: it seals each admitted draw
   from the real command-time target generation, TMEM image, tile, accesses,
   and material state, runs compute, and compares the complete target against
   the CPU result. The first game-derived run rejected the shader at packet
   2648, command 6, pixel `(95,95)`. Stage tracing proved coverage and S10.5
   coordinates agreed (`S=1997`, `T=3028`) while the CPU point sampler
   produced `[f7,f7,f7,ff]` and the shader's unconditional three-nearest path
   produced `[f1,c8,cf,ff]`. A distinct point-sampling callable fixed the
   filter-selection boundary; fractional-coordinate synthetic coverage then
   passed its 10-run native Metal differential, and the 13-packet game window
   passed 10 consecutive complete-target runs (500 admitted draws). Additional
   state keys and replacement of per-draw prototype resources/readbacks remain
   open.
6. **Production A/B seam.** Add a strict same-binary CPU-versus-compute control.
   Run counterbalanced `A/B, B/A`, then re-profile. Retain only if both orders
   improve p95 and the named RDP cost falls.
7. **Batch-boundary widening.** Add exact GPU fill and texrect execution so a
   whole raw-DPC packet normally incurs one upload/readback pair rather than a
   pair around every triangle run. Only measured boundary frequency decides
   this order.
8. **Certification.** Require 120 live framebuffer swaps byte-identical,
   applicable differential suites, then ten consecutive clean performance
   runs with p95 <=25 ms, p99 <=28 ms, zero frames over 33.333 ms, and at least
   97% gap-two cadence.

## Kill gates

- The dynamic-target no-op must be byte-exact before shader arithmetic lands.
- A one-state prototype must remove at least 3 ms p95 in the live lane. If it
  does not, measure upload/readback and batch-boundary counts before adding
  more shader programs.
- All eight observed states plus boundary widening must reach p95 <=25 ms.
  Merely crossing 33.333 ms is not completion because it leaves no extension
  budget.
- Any byte mismatch, effect-digest mismatch, untyped fallback, per-draw
  readback, or command-order race kills the candidate rather than weakening
  the oracle.

## Remaining-gap ledger

The certification gap is 13.8 ms at p95 (`38.8 -> 25.0 ms`). Nothing is
credited before a same-binary A/B; the rows below are measured cost pools,
not promised savings. They prevent the work list from quietly adding up to
less than the target:

| Cost pool | Current evidence | Optimization that can retire it |
| --- | ---: | --- |
| Scalar triangle execution | 7.939 ms in the 13-packet replay; 48.2% of live raster time belongs to the first admitted key and 98.4% to the first five keys | Exact pixel-owned compute for keys 1--5 |
| Declared reads + copyback + commit | 2.722 ms in the replay | Keep the packed RGBA16 target resident through the burst and read back once at publication |
| Planning | 2.232 ms in the replay | Build one sealed batch from already-decoded state and remove repeated per-packet target/TMEM preparation |
| Boundary-heavy packet 2657 | 3.27 ms of its 4.405 ms total lies outside execute | Add exact fill/texrect device execution and remove CPU/GPU batch turns |

These pools overlap at packet boundaries, so they must not be summed as a
forecast. They do show why a triangle-only shader with per-draw transfers is
insufficient: even eliminating all measured scalar execution would leave the
live 13.8 ms p95 gap underfunded. The retained order is therefore:

1. finish one complete color-producing state and require a >=3 ms live p95
   win;
2. make target residency span packet-local triangle batches, measuring the
   transfer terms separately;
3. widen exact color execution through the five keys covering 98.4% of live
   raster time;
4. absorb fill and texrect boundaries, starting with packet 2657's measured
   non-execute spike;
5. reprofile after every item and stop only at the ten-run certification bar.

Headroom beyond 25 ms must come from the same ledger: a lower shader time does
not authorize spending the transfer/planning savings twice. If the complete
first state misses its 3 ms kill gate, the next action is GPU timestamp and
boundary-count attribution in `raw_dpc_replay`, not adding more state keys.

## Replay baseline (2026-08-23)

On the host Metal adapter, after priming packets 0 through 2646, a 30-repeat
replay of the 13-packet suffix ending at private receipt 2659 was byte-stable
and measured 14.132 ms mean total: 7.939 ms execute, 2.232 ms plan, 1.397 ms
declared guest reads, 1.059 ms copyback, 0.266 ms commit, and 0.239 ms for the
remaining measured phases and loop arithmetic. Suffix bisection measured
1.098 ms for two packets, 5.571 ms for four, and 9.387 ms for eight. The cost
is therefore distributed across the graphics burst rather than owned by the
last five-triangle packet alone (0.953 ms total, 0.661 ms execute).

This receipt uses the final packet's matching RDRAM snapshot to satisfy every
prefix read. It is an exact deterministic optimization benchmark for the
captured command geometry and final packet, but not a claim that historical
prefix texel bytes equal the live run at each earlier packet. Live linked-shell
profiling remains the performance authority; complete guest-byte stability is
the replay's regression oracle.

Per-packet detail over the same suffix shows ten triangle-bearing packets at
roughly 0.69--0.79 ms execute each, two state-only packets at roughly 0.04 ms,
and packet 2657 at 4.4 ms total despite only 1.13 ms execute because its
plan/read/copy boundary dominates. This selects packet-local triangle batches
as the compute unit and keeps resident-target transfer removal as the next,
independently measured optimization.

The first production-probe timing intentionally includes the unoptimized
diagnostic mechanism: 10 repetitions of the same 13-packet window averaged
53.386 ms total, of which 35.697 ms was 50 per-draw compute calls creating
buffers/bind groups and synchronously reading back both the full target and a
four-word-per-pixel stage trace. This is not a candidate performance result;
it quantifies the setup/readback class the plan already requires production to
remove. The byte differential is now trustworthy enough to optimize that
mechanism without weakening correctness.

Removing that successful-path stage trace was the first isolated optimization.
The same 13-packet window, with two warmups and ten measured repetitions, kept
the exact `1ac409f336397652` effect digest and completed 500 admitted draws
without a target mismatch. Mean compute-probe time fell from 35.697 ms to
22.267 ms (-13.430 ms, 37.6%), while mean total window time fell from 53.386 ms
to 39.396 ms (-13.990 ms, 26.2%). The remaining call still creates nine GPU
buffers and two bind groups, submits and synchronously waits once, and maps the
status and target readbacks for every draw. Persistent high-water resources are
therefore the next independently measured mechanism; this result does not yet
credit the compute path with an end-to-end live-frame win.

The second isolated optimization retained one high-water set of those nine
buffers and its two bind groups. A required Metal differential proves ten
identical submissions create exactly one resource generation. On the same
two-warmup, ten-repeat game window, all 500 draws again matched and the effect
digest remained `1ac409f336397652`; mean compute-probe time fell from 22.267 ms
to 16.556 ms (-5.711 ms, 25.6%), and mean total window time fell from 39.396 ms
to 33.714 ms (-5.682 ms, 14.4%). The remaining probe still performs 50 ordered
submit/wait/status-map/target-map cycles and uploads the complete target and
TMEM for each call. Packet-local compatible batching is now the selected
mechanism; resource reuse alone is retained but is not a production A/B.

The third isolated optimization kept one builder across adjacent raw-triangle
commands and flushed only at a non-triangle command or a typed incompatibility
in target generation, program, TMEM identity/image, or tile binding. The replay
receipt now reports actual GPU batch count rather than merely whether a packet
had a receipt. The same game window reduced 50 draws to 30 batches per repeat;
all 500 draws matched, the effect digest remained `1ac409f336397652`, and mean
compute-probe time fell from 16.556 ms to 11.501 ms (-5.055 ms, 30.5%). Mean
total window time fell from 33.714 ms to 27.717 ms (-5.997 ms, 17.8%), with
p95 28.934 ms and max 29.485 ms. A mismatch still names the exact packet and
pixel, but honestly reports the sealed command/triangle range rather than
misattributing a batched result to its first draw. This diagnostic timing still
includes the complete CPU raster before compute and is not a live replacement
A/B.

The fourth isolated optimization allocates one persistent resource slot per
typed batch but encodes all of a packet's independent seeded dispatches and
readback copies into one command buffer and waits once. The receipt now
distinguishes submissions from typed batches. Across the same ten repeats this
was 100 submissions for 300 batches and 500 draws; every complete target
matched and the effect digest remained `1ac409f336397652`. Mean compute-probe
time fell from 11.501 ms to 8.339 ms (-3.162 ms, 27.5%), and mean total window
time fell from 27.717 ms to 24.652 ms (-3.065 ms, 11.1%), with p95 25.722 ms
and max 26.676 ms. This is still a diagnostic double execution. Its next gate
is a same-binary replacement A/B that removes the 7.939 ms CPU raster cost
rather than adding more prototype work around it.

## Sources and nonclaims

The semantic oracle is fn64's existing CPU raster and its cited allowed
sources: public N64 documentation, pinned MIT RT64, and the repository's own
behavioral specs. The plan does not read or use a GPL runtime. It does not
claim that existing RGBA8 diagnostic GPU output is guest-correct, nor that a
GPU result is portable until the same byte-identity gates pass on another
supported adapter.
