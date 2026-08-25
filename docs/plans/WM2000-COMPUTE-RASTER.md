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
6. **Production A/B seam.** The strict same-binary CPU-versus-compute control
   is implemented and runs counterbalanced `A/B, B/A` with exact committed
   guest-byte comparison. Packet-local replacement failed its performance
   gate: CPU averaged 14.221 ms total and compute averaged 17.844 ms, so the
   replacement remains opt-in. The next A/B must move the synchronization
   boundary across packets rather than add more work inside this boundary.
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

1. keep the exact packet-local replacement as the counterfactual oracle, but
   do not enable it: its counterbalanced A/B loses 3.623 ms;
2. prove the first real guest or VI consumer after each graphics burst and
   keep the typed packed target device-resident until that boundary;
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

The wider synchronization/transfer sweep then tested whether typed state
boundaries still required host round trips. An opt-in ordered-chain probe now
uploads the packet's initial target once, dispatches each typed batch in RDP
order, copies the target between persistent device slots, retains one status
buffer per batch, and reads the final target once. Keeping status per batch is
load-bearing: a later successful dispatch must not overwrite an earlier TMEM
refusal. A required native Metal differential ran both the ordinary two-draw
batch and a forced two-dispatch chain ten times each against the CPU target.
On the same 13-packet game replay, counterbalanced independent/chained and
chained/independent ten-repeat runs kept effect digest
`1ac409f336397652`. Independent compute means were 9.797 and 9.690 ms;
chained means were 8.533 and 8.436 ms, a paired-order average reduction from
9.744 to 8.485 ms (-1.259 ms, 12.9%). Total means fell from 24.413 to
23.337 ms (-1.077 ms, 4.4%). This proves intermediate target transport is a
real cost class and supplies the ordered device mechanism production needs;
it remains diagnostic double execution and earns no CPU-raster savings yet.

The transaction-integrated replacement gate then made compute bytes, rather
than the CPU oracle, supply the typed completion, declared guest writes,
effect digests, copyback, and publication. Ten repeated fixed-mode executions
were byte-exact. Three additional transport changes were retained in order:
dispatching only journal-proven rows reduced target-pixel work from 3,456,000
to 691,200 per burst; consolidating all batch statuses into one mapped
high-water readback reduced fixed-mode GPU work from 6.102 to 5.622 ms; and
mutating one shared device target across ordered passes removed intermediate
full-target copies and reduced it again to 5.133 ms. A bind-group cache
regressed that result to 5.316 ms and was removed; reusing an existing TMEM
projection did not improve the measured result and was also removed.

The stricter same-process alternating replacement A/B exposed the remaining
boundary cost that a continuously hot GPU loop concealed. CPU-first measured
CPU execute/total means of 8.039/14.231 ms and compute means of
11.813/17.979 ms. Reversing the order measured 8.019/14.211 ms and
11.567/17.708 ms. Every CPU and compute completion had identical committed
guest bytes, but the paired-order total averages were 14.221 ms for CPU and
17.844 ms for compute: packet-local replacement is 3.623 ms slower and fails
the kill gate. Fixed-mode compute's 5.133 ms was not representative of the
live-like alternating idle cadence, where compute work averaged about
7.67 ms.

This negative result changes the next execution boundary. Ten packet-level
upload/submit/wait/readback/copyback transactions per burst overwhelm the
shader saving; optimizing another packet-local setup term cannot fund the
remaining gap. The next mechanism must keep the target device-resident across
packets and synchronize once at the first access-journal-proven guest or VI
consumer. Correctness requires that consumer boundary to be represented in
the transaction types; timing alone cannot authorize delayed guest writes.

A task-shape census and synthetic replay control then separated transport-call
count from device lifetime. The captured task suffix contains nine consecutive
all-triangle submissions (45 draws, 27 typed batches), followed by a three-
submission state/fill boundary and one more triangle submission. Executing the
nine submissions through packet-local replacement took 13.355 ms total in a
single sizing run versus 8.540 ms on CPU. Concatenating the same nine command
streams into one non-certifying task-scoped control retained the final RDRAM
SHA-256 and reduced the GPU path to roughly 8.84 ms, recovering about 4.5 ms
of repeated synchronization while only reaching CPU parity. This is evidence
for task-scoped residency, not authority to concatenate production journals.

The first lower-level optimization after that control bounds every even-width
compute dispatch to the two-pixel-aligned horizontal union of its journal-
proven writes as well as its existing row band. Odd-width targets retain the
contiguous row-band mapping because a packed word can cross a row boundary;
two row-local invocations must never race on that word. The focused native
Metal differential matched complete CPU target bytes for 10 consecutive runs.
In counterbalanced 40-repeat legs over the nine-submission task control, the
candidate reduced GPU target-pixel visits from 10,598,400 to 2,119,680. Paired-
order GPU total fell from 8.915 to 8.329 ms (-0.586 ms, 6.6%), and candidate
GPU total was 0.752 ms below its paired CPU control. The final RDRAM SHA-256
remained unchanged. `FN64_COMPUTE_RASTER_COLUMN_BOUNDS=0` is the same-binary
measurement control; absent or `1` enables the bounded path while compute
replacement itself remains opt-in.

The 80% invocation reduction yielding only a 6.6% total improvement moves the
next profile below pixel coverage: quantify fixed uploads, bind/encode work,
GPU execution, wait, status map, and target readback across the remaining 23
typed dispatches before widening another shader state. Task residency and
horizontal bounds are retained mechanisms, but together they do not yet fund
the 13.8 ms certification gap.

`FN64_COMPUTE_CHAIN_TIMING=1` supplies that diagnostic with clocks entirely
absent when disabled. Thirty measured candidate repetitions attributed a
2.492 ms mean chain call as follows: 1.995 ms queue/GPU completion wait
(80.1%), 0.272 ms uploads, 0.138 ms command encoding, 0.037 ms submit, 0.033
ms status mapping, 0.004 ms target mapping, 0.011 ms bind-group creation, and
approximately 0.001 ms preparation/resource checks. The surrounding replay
reported 2.555 ms compute work and retained the same RDRAM SHA-256.

This rejects bind-group caching, readback removal, and host-side check deletion
as closure candidates. The next profiler must use device timestamps around the
23 ordered dispatches, if the adapter exposes the required feature, to split
shader execution from queue latency. The next optimization must then reduce
dispatch/state-boundary GPU work; host setup is too small to fund the gap.

`FN64_COMPUTE_GPU_TIMING=1` now requests `TIMESTAMP_QUERY` explicitly and
places beginning/end timestamps around every ordered compute pass. An adapter
without that feature fails with its name instead of silently substituting a
host clock. The diagnostic rejects zero or non-monotonic query pairs and
leaves unused sentinel slots at both physical ends of the query set; Metal
occasionally returned an invalid final used slot even with that padding, so
wrapping subtraction would otherwise manufacture a many-hour duration.
Disabled mode requests no extra device feature and creates no query resources.

A five-warmup, twenty-measurement native Metal run of the nine-submission task
control produced 19 fully valid traces out of 25 timestamped calls. The
23-pass span had a 1.889 ms median and 1.023--1.958 ms range; the lower mode
also reduced every pass together and is consistent with device clock changes,
not a different workload. In the dominant mode, the first one-triangle pass
cost about 0.056--0.065 ms and each following two-triangle pass clustered
around 0.079--0.085 ms despite the different draw state. The pass-local sum
accounted for almost the complete device span. No repository test can rederive
the private capture, but this run checked final RDRAM SHA-256 on every
iteration; no test owns `4af275f0aa78f5453eeb82ebc7b821b14fd994c0b8e32c041bd3f719acf954c6`.
This rules out one pathological dispatch and selects exact state-preserving
pass fusion: upload the immutable TMEM/tile state visible to each draw, retain
draw order, and scan the bounded target once. Per-pass shader arithmetic
tuning comes after that architectural candidate is byte-proven.

The state-boundary census then found 23 TMEM/tile runs across 23 dispatches,
so adjacent-state coalescing cannot reduce this receipt. The retained fusion
instead uploads all 23 immutable states and tags each triangle with its state
index. A sparse host-built worklist gives each packed RGBA16 target word one
GPU owner and lists only the triangles whose journal-proven rectangle reaches
that word, in original painter order. The shader selects the tagged state
immediately before sampling and packs the first TMEM error's state index with
its status, preserving loud per-dispatch attribution through the single pass.

A full-target one-pass prototype was byte-exact but regressed GPU execution to
2.536 ms because it evaluated every draw across all 115,200 pixels; it was
removed. The sparse form retained 105,984 pixel visits and reduced 23 passes
to one. In a same-binary counterbalanced `A/B, B/A` run with five warmups and
50 measured repetitions per leg, all 200 measured completions retained the
same private-capture identity. No test can rederive final RDRAM SHA-256
`4af275f0aa78f5453eeb82ebc7b821b14fd994c0b8e32c041bd3f719acf954c6`; no test owns that private artifact.
Paired means fell from 3.362 to 2.324 ms for compute work (-1.038 ms, 30.9%)
and from 8.794 to 7.740 ms total (-1.054 ms, 12.0%); paired p95 fell from
9.573 to 7.913 ms. That experiment used a historical same-binary multi-pass
control. The control was removed when packet-checkpoint execution adopted
sparse boundary events: the old multi-pass shape cannot produce those
boundaries through the same typed output mechanism and is no longer a runtime
mode.

The focused native differential retains three independent contracts. The
ordinary two-draw compute batch matches the CPU raster's complete target
bytes. A forced typed boundary with an observably different second TMEM state
matches the pre-fusion sequential GPU result, proving state selection and
painter order rather than merely repeating one texture. A first state with no
valid TMEM bytes followed by a successful state still reports the first
state's invalid-byte refusal and target pixel. Ten consecutive native test
processes passed; each process repeats all three GPU contracts ten times.
Splitting the valid synthetic draws into separate GPU submissions does not
match the CPU oracle even without fusion; that pre-existing sequential-
submission discrepancy is therefore not attributed to this optimization.
Game-derived replay bytes remain exact, but production task residency and a
fresh live linked-shell certification are still required before this work can
claim the 30 Hz goal.

The fresh linked-shell census explains why packet-local replacement cannot be
the shipping policy. With replacement forced for every eligible packet, 800
measured pumps averaged 13.885 ms and p95 was 36.314 ms. The same-binary CPU
control averaged 13.140 ms with a 36.388 ms p95. Phase counters attributed
7.029 s of 8.512 s raw-DPC session time to 30,745 tiny execute calls (0.229
ms/submission); the fixed synchronous GPU boundary exceeded the raster work.
The CPU draw census measured the dominant fragment classes at 31--50
ns/pixel, so removing correctness checks or tuning shader arithmetic is not
the closure mechanism for these packets.

Production replacement now admits a chain only when its exact sum of
journal-derived, column-bounded dispatch rectangles reaches 16,384 target
pixels. `FN64_COMPUTE_RASTER_MIN_TARGET_PIXELS` accepts a decimal `u32` for
same-binary crossover sweeps; absent selects 16,384. A 4,096-pixel live run
still behaved like all-GPU replacement (13.733 ms mean, 35.654 ms p95), while
16,384 returned to the CPU control population (13.177 ms mean, 35.576 ms
p95). Thus small packets retain the exact CPU path while the larger replay
chain retains access to the fused GPU mechanism. This prevents a measured
regression; it does not by itself establish 30 Hz headroom.

A decoder-journal cache was also tested and removed. Although a cache hit
still ran the authoritative decode and journal comparison, WM2000's command
payload identities include changing triangle parameters and rarely repeated.
In identical phase-counted 800-pump runs, cache-off versus cache-on plan time
was 1.379 versus 1.402 s and p95 was 35.661 versus 35.734 ms. The rejected
candidate narrows the remaining work to transaction-level residence/batching
or another measured CPU-path mechanism; content-key caching is not retained.

The texture-plane microbenchmark itself then exposed a harness defect. Its
`BenchTmem::snapshot` constructed a fresh `PhysicalTmemState` on every sampled
pixel, so the earlier roughly 500 ns/pixel result primarily measured a heap
allocation that production never performs. Giving the fixture one stable,
draw-constant snapshot identity reduced the corrected control to 5.277--5.460
ns/pixel. Exact incremental S/T/W stepping measured 5.057--5.080 ns/pixel,
confirming a small 4--7.5% isolated win rather than the prior null result. The
production toggle remains `FN64_INCREMENTAL_TEXTURE_PLANES`; live linked-shell
controls found only a modest roughly 0.34 ms all-pump p95 and 0.73 ms slow-pump
p95 improvement, so it is retained but cannot close the architectural gap.

An opt-in planning census (`FN64_RAW_DPC_PLAN_CENSUS=1`) next split the two
decoder passes from journal/ticket construction and final plan admission. In
the same audio-enabled linked-shell run as the execution census, 20,000 plans
accounted for 957.442 ms: probe prepare 31.115 ms, probe decode 142.926 ms,
real prepare 497.609 ms, real decode 167.310 ms, and admission/seal 118.483
ms. The complete 24,873-submission session attributed 1,171.6 ms to planning
and 5,511.8 ms to execution. Removing the probe decode could therefore save
only about 0.24 ms per measured pump, while the slow population spent 22.692
ms/pump in raw-DPC work. Double decode is real redundant work, but it is not
the render-starvation root cause.

Audio telemetry closes the integration diagnosis independently. The host
callback reported zero late callbacks and a maximum callback gap of 10.736
ms, yet the 250 ms output ring reached zero as the heavy scene began and
underrun sample slots rose from 2,334 to 165,224. Synthesis and resampling are
therefore not blocking the device callback; synchronous per-submission render
execution prevents the emulation producer from replenishing it. A larger ring
would postpone that sustained deficit. Closure requires task-scoped render
residency/batching that keeps every source run's journal, fabric token, guest
commit, and final FullSync identity authoritative while amortizing raster
submission and target synchronization at the task consumer boundary.

The first task-transport proof now retains the nine color-producing packet
boundaries before private packet 2657. It collects the already-typed compute
fixtures while ordinary CPU execution, guest commits, copyback, and
publication remain packet-local, encodes every packet pass and target
checkpoint copy into one command buffer, waits once, and rejects any missing
color executor or discontinuous resident postimage. Packet 2657 is rejected
loudly because its real fill/texture work is not representable by the current
triangle executor; it is the next widening boundary, not an implicit fallback.

The first correct checkpoint implementation exposed another host-side
algorithmic cost. Building and stable-sorting roughly five million
`(target-word, triangle)` pairs took 14.924 ms while timestamped GPU work took
3.225 ms. A direct word-major construction preserved painter order by
construction and reduced ten-run mean checkpoint time from 21.571 to 11.342
ms. The observed task rectangles cover the full target, so a further typed
dense mode now dispatches target words directly and indexes the five ordered
triangles without materializing any worklist. Across ten consecutive native
Metal replays, all 450 draws retained committed digest `1ac409f336397652` and
postimage SHA-256 `a1b7712dff9a3fc605aa2112849a1e7ace2d3baa7124771f3a5b12cc5d72f2a8` (not tested by the repository; this is a historical observation).
Mean checkpoint time fell again to 5.175 ms (p95 5.668 ms), versus 8.478 ms
mean CPU execution for the complete 13-packet replay. This proves a measured
approximately 3.3 ms execution advantage for the task-scoped triangle
segment even while retaining nine intermediate target images.

Packet-local replacement remains rejected. With its replay threshold lowered
to zero so the A/B actually exercised compute, ten counterbalanced GPU legs
averaged 17.926 ms total versus 14.046 ms for CPU; compute submitted ten times
per leg and averaged 7.809 ms before admission/effect overhead. A 16,384-pixel
threshold run produced zero replacement receipts and therefore was not a GPU
A/B. The evidence selects task-scoped execution and exact fill/texture
widening; it does not authorize enabling packet-local replacement.

Packet 2657's command stream also exposed the same full-buffer ownership bug
class on the CPU boundary: its long `FillRectangle` run moved the accumulated
target between commands, but `execute_fill_rectangle` immediately cloned the
entire buffer again. The production-only owned form now consumes and mutates
that allocation; the public borrowed executor remains unchanged. A pointer-
identity unit test proves allocation reuse. Ten exact replay runs with the
ownership-disabled control averaged 14.260 ms total and 8.114 ms execute;
the owned path averaged 13.638 ms total and 7.481 ms execute, retaining the
same committed digest and postimage SHA. The 0.622 ms total gain is retained,
but packet 2657 still blocks a triangle-only task chain and still needs an
exact device fill/texture executor.

The fresh linked rs+wgpu authority run retained the replay direction but
confirms that this is not closure by itself. Over 700 measured audio-enabled
pumps, mean drawn-frame time improved from 25.785 to 25.338 ms, p95 from
39.214 to 38.423 ms, and the over-33.333 ms share from 24.1% to 22.1%.
Slow pumps still averaged 22.329 ms in raw-DPC work, accounting for 84.2% of
their excess. The audio callback again had zero late callbacks and a 10.739 ms
maximum gap, while the ring reached zero and underrun sample slots climbed to
146,792. The remaining failure is still producer starvation under synchronous
render execution; the local ownership win merely narrows it.

## Task-transport closure and live gate (2026-08-24)

The production task path now batches every coalesced raw-DPC member under one
transaction and enables the task-scoped compute replacement by default.
`FN64_RAW_DPC_TASK_BATCH=0` and `FN64_RAW_DPC_TASK_COMPUTE=0` retain explicit
same-binary controls. Two host-side mechanisms closed the remaining transport
gap without weakening guest-memory authority:

- An exact-range task arena captures one immutable RDRAM payload and digest
  once, then binds it to each independently ordered read descriptor. A phase
  census found 1,666,802,336 requested bytes but only 35,204,064 exact-unique
  bytes within tasks (97.9% duplicate). Guest capture fell from 178.1 ms to
  16.2 ms over 120 tasks, a 91% reduction; total task-batch time fell from
  1,287.6 ms to 1,133.9 ms.
- Bulk logical copyback validates the complete guest range before its first
  store, copies aligned native-word bodies in one slice operation, reverses
  each word to the canonical guest byte order, and leaves only the at-most
  three-byte head and tail on the scalar path. Copyback fell from 127.6 ms to
  39.0 ms over the same 120-task window, a 69% reduction.

The ABI no longer converts a `PhysicalRange` length into a host index merely
to allocate capture storage. `RdramView::read_logical_bytes` owns the one
checked guest-length-to-allocator conversion; task transport retains
`PhysicalRange`, `RdramAddr`, and `u32` byte lengths until that boundary.
`ValidatedGuestCopyback` likewise carries an `RdramAddr` and borrowed bytes,
not precomputed host slice endpoints.

The ordinary no-opt-in launcher path resolved wgpu, reached 68 compute
segments (1,012 compute and 514 CPU members across 120 tasks), retained swap
hashes `3686c6ccce4d3853` at swap 60 and `9f23f803b308b6b4` at swap 120, and
measured 15.521 ms p95 / 18.793 ms max. The final rebuilt-binary gate then
passed ten consecutive native Metal/CoreAudio runs. Across that streak,
180-pump p95 was 15.447--15.965 ms and max was 18.699--19.465 ms; both hashes
were identical in every run, each swap-120 audio window added zero underrun
sample slots, late callbacks remained zero, and every run reached the bounded
clean-exit path. The max remains far inside the 33.333 ms 30 Hz drawn-frame
budget.

Hash stability is repeatability evidence, not a semantic oracle. The native
Metal `required_host_hot_compute_color_matches_ordered_cpu_bytes_ten_times`
test independently compared complete compute output with the ordered CPU
raster bytes ten times, and
`task_compute_batches_two_raw_triangle_packets_and_publishes_each_generation`
proved that both private checkpoints publish in task order; both passed after
the live gate.

The broad ABI gate exposed one validation-boundary regression: optional GPU
diagnostic-draw extent and pipeline checks were running even when the backend
used the authoritative CPU raster without a prior window/device `create()`.
Renderer-neutral packet, TMEM-projection, draw-state, and blend validation
still runs unconditionally; only GPU fixture construction and
`TriangleDrawBeforeCreate` are conditional on enabling that diagnostic draw.
All 51 raw-DPC integration tests and ten consecutive direct repetitions of
the formerly failing fill+TMEM+triangle case pass. The combined ABI,
render-IR, and runtime nextest gate is 843/844; its sole failure is the
pre-existing generated `docs/COMPLETENESS.md` NMR-surface drift, while the
real C ABI smoke test passes.

The typing audit's framebuffer-clone hypothesis was measured rather than
assumed. Across 120 tasks, registry clones copied 54.6 MB in 0.991 ms total
and task-shadow commits copied 91.5 MB in 1.565 ms total, about 0.021 ms per
task combined. Removing broad `Clone` authority remains a worthwhile type-
system cleanup, but the census disproves it as material performance headroom.
`FN64_TASK_COMPUTE_CENSUS=1` retains these clocks for future ownership work.

The longer 800-pump route exposed a second admission boundary that the
180-pump gate did not reach. A packet containing raw triangles was previously
marked by a boolean as compute-shaped before the exact program, draw, access,
and ordering checks ran. Later rejection was therefore indistinguishable from
executor corruption. Task planning now carries a move-only execution
disposition. Exact admission produces the compute capability; an expected
refusal carries a stable reason into the ordered CPU member; executor errors
remain loud. The census reports member count and elapsed CPU time for every
reason. Program-bit and cycle-type rejection additionally report the exact
four RDP program words, so widening decisions can be ranked from the sustained
route rather than inferred from an earlier draw corpus.

The reporting-only taxonomy separates packets with no raw triangle, packets
mixing triangles with fill or texrect commands, disabled compute, completion
shapes that cannot be deferred, structural admission failures, and exact
program-bit failures. It does not catch a generic task error and fall back.
TMEM or tile changes that only require another dispatch are also distinct from
admission refusal. This keeps the diagnostic denominator closed without
weakening the task transaction or its command ordering.

The sustained-tail census further partitions the former `NoRawTriangle`
bucket into fill, texrect, TMEM-load, their mixed shapes, sync/state-only, and
no-op-only packets. Compute segments are attributed to their exact shader
program or to a typed mixed-program bucket, with segment, member, and elapsed
totals that close against the existing census denominator. These labels are
reporting metadata derived from the already-decoded command stream; they do
not change execution or add per-draw clocks when the census is disabled.

The first 800-pump keyed capture measured the sustained-route candidates at
task 570. The one-cycle program `fc309661/552eff7f/0008ecef/00504240` consumed
298.559 ms across 657 CPU members. Two-cycle programs consumed 287.059 ms
across 589 members for `fc1596a3/f0fffe38/0018ac8f/00504240`, 205.864 ms
across 514 members for `fc1596a3/f0fffe38/0018acef/00504240`, and 350.735 ms
across 5,095 members for `fc15fea3/f00ff23f/0018acff/0f0a7008`. The last pool
has the largest aggregate but only 0.069 ms per member, so its dispatch fixed
cost makes it a poor first widening. The measured one-cycle key is first: it
has comparable recoverable CPU time to the leading two-cycle key while reusing
the current one-cycle execution shape. Each widening still requires complete
CPU/GPU target-byte identity and a same-binary performance kill gate.

The later 1,600-pump route changed the optimization boundary. Over-budget
frames executed 32,519 CPU members of
`fc15fea3/f00ff23f/0018acff/0f0a7008`, averaging 5.733 ms per over-budget frame
but only 0.074 ms per member. An exact shader widening passed ten separate
4,300-packet production-path CPU/GPU byte differentials, but failed its live
performance kill gate: p95 rose to 63.738 ms and 59.7 percent of 792 drawn
frames exceeded 33.333 ms. The widening is therefore not admitted. This class
is a repetition problem, not an individually expensive draw problem: the
current compute chain preserves each packet generation by repeating dispatch
and checkpoint work. The next optimization must reduce that per-member cost
while retaining exact ordered packet publications; admitting more members to
the existing mechanism is known-worse. Checkpoint images are redeemed through
one move-only exact-cardinality value, so a missing or extra device result is
rejected before candidate or guest-effect mutation instead of being silently
truncated by iterator pairing.

Cross-checkpoint fusion then tested whether packet-sized GPU fixed cost was
the whole failure. The chain now emits one word-major ordered event stream,
uses checkpoint markers to capture deterministic sparse packed-word outputs,
and submits one compute pass per chain. The host reconstructs the existing
ordered full images, so packet generations and publication semantics are
unchanged. On the 800-pump instrumented route this reduced 9,876 passes to
742, GPU-valid time from 1,259 to 527 ms, and planner preparation from its
first fused implementation's 1,595 to 483 ms. The latter improvement came
from a two-pass dispatch-major planner and reusable target-sized scratch
storage rather than a word-by-dispatch scan or nested per-row vectors.

Fusion was not enough to make the many-small-member program profitable. On
the same 2,190-task/80,156-member tail, admitting it moved 43,869 members to
compute, added 10,616 ms of compute work, and removed only 3,575 ms of CPU
work: 7,041 ms more serialized task work, or 8.81 ms per drawn frame. Clean
1,600-pump p95 was 57.248 ms. Refusing only that exact program while retaining
fusion restored p95 to 43.410 ms (mean 31.069, p99 47.256, max 48.884). It
remains rejected; the next widening candidates are ranked by recovered CPU
time per member, not aggregate member count.

The next high-cost-per-member candidate,
`fc15fea3/f00ff23f/0018ac8f/0f0a7008`, also passed the native Metal
CPU/GPU byte differential ten times and one complete 4,300-packet
production-state differential. Its committed digest and final postimage hash
were unchanged. It nevertheless failed the sustained live kill gate: against
the exact id3-off 1,600-pump baseline, p95 rose from 43.410 to 45.335 ms,
mean from 31.069 to 32.161 ms, and the over-budget share from 50.7 to 57.2
percent. It is therefore not live-admitted. Exact shader support remains a
future batching primitive, not evidence that the current per-member transport
is profitable.

The task read arena now also shares immutable captured bytes across packet
bindings. Each journal access index remains distinct, while a task-local pool
interns bytes only after a typed physical-range/content-digest match and a
full byte comparison, so neither a hash collision nor equal bytes at a
different address can authorize reuse. Ordinary single-packet execution does
not retain the pool. This closes the remaining 97.9-percent duplicate-capture
ownership path without weakening per-packet ordering; its live effect must be
measured separately from the rejected-program rollback.

Planning now performs the immutable program-shape half of compute admission
before task execution. A typed definitely-CPU disposition carries the exact
refusal reason; only packets whose program shape passes can request the later
target, access, TMEM, and generation admission. Missing state remains a loud
runtime concern. This removed the speculative stage/decode followed by a
second CPU stage/decode for 63,224 of 80,156 sustained-route members. On the
clean 1,600-pump route, p95 fell from 43.410 to 40.368 ms, mean from 31.069 to
29.199 ms, and the over-budget share from 50.7 to 39.4 percent. The result is
promising but not yet the required ten-run quiet-machine gate.

Raw-DPC planning no longer runs a speculative prepare/decode followed by the
authoritative decode. A command-only seed preflight performs no placeholder
TMEM allocation; one planning-only decode derives the exact journal and a
sealed adapter pushes it into the coordinator. The planning output type owns
neither a submitted ticket nor staged execution state, while ordinary decode
still rejects any exact-journal disagreement loudly. The previous 80,000-plan
census attributed 687.249 ms to probe prepare/decode alone; the one-pass live
effect remains to be measured after the linked shell is rebuilt.

The two largest coverage/fog CPU programs now enter a closed exact-program
specialization for their combiner and terminal RGBA16 write. Every skipped
generic terminal condition is part of the fallible proof. The specialized and
generic paths matched across 131,072 alpha/channel-boundary comparisons and
six full-frame fixtures repeated ten times. Twenty alternating release
microbenchmark samples reduced a 300x220 exact-program raster from a 1.897 ms
median to 1.458 ms, a 23.1-percent reduction. This is kernel evidence only;
the sustained WM2000 gate remains authoritative. With presentation caching
disabled, the rebuilt one-pass-planning plus CPU-specialization binary
improved the clean 1,600-pump result from 40.368 to 36.126 ms p95, 29.199 to
25.649 ms mean, and 39.4 to 13.6 percent over budget. This is one clean run,
not the required repeatability gate, and leaves a 2.793 ms p95 gap to the
33.333 ms line.

The next sustained-tail profile identified
`fc1596a3/f0fffe38/0018acef/00504240` as the new dominant exact CPU program in
the slowest fields. A second closed program proof specializes only its fog
lerp combiner and deliberately retains the shared noise-dither, blend,
coverage, and packing stages. Its shortcut matched the generic combiner over
all 65,536 texel-alpha/primitive-alpha pairs with deterministic channel
variation and matched three complete-frame fixtures ten times. Twenty
alternating release samples reduced the exact-program microbenchmark median
from 0.413 to 0.309 ms, a 25.2-percent kernel reduction. Its sustained live
effect was smaller but positive: the comparable instrumented 1,600-pump p95
fell from 36.837 to 35.942 ms. In the slowest five percent of fields, the
program's CPU bucket fell from 6.830 to 5.945 ms per frame. The remaining
2.609 ms p95 gap therefore cannot be closed by this specialization alone.

That same run made the next repeated-work target explicit. In the slowest five
percent of fields, combined texrect-and-TMEM packets consumed 3.853 ms per
frame, versus 0.242 ms for texrect-only, 0.254 ms for TMEM-only, and 0.063 ms
for fill-only packets. The task-tail analyzer now understands the typed
non-triangle partition and per-segment compute program fields, correlating
both CPU and compute buckets with the exact drawn frames. A same-frame
counterfactual ranks the broader CPU triangle population ahead of that one
packet shape: removing every CPU-triangle bucket would lower p95 by at most
8.625 ms, while removing every non-triangle bucket would lower it by at most
2.725 ms and removing only combined texrect/TMEM would lower it by at most
1.328 ms. These are correlation ceilings, not predicted speedups.

`FN64_RAW_DPC_TASK_CPU_COLOR_BATCH=1` enables the first ordered CPU triangle
batching tranche for same-binary A/B measurement. Compatible same-target,
depth-free triangle members move one full accumulator through the task instead
of cloning a seed and private shadow for every packet. Each packet still owns
an independently ordered, journal- and digest-bound sparse publication token;
typed generation reservations reject target substitution, skipped generations,
and stale or reordered publication. CPU/compute, depth, and incompatible-target
boundaries move the final accumulator into the private registry once. TMEM
staging also advances its already-validated word plan directly rather than
cloning the plan for a second scan. This tranche deliberately does not combine
the scalar triangle raster walks, so its live ceiling is the eliminated
allocation and framebuffer-copy work. It remains default-off until a long
counterbalanced live run proves byte identity and a net timing win. The first
two counterbalanced 1,600-pump pairs used one release binary and changed only
this flag. Candidate/control p95 was 34.173/34.961 ms and 34.539/35.447 ms;
mean was 24.692/25.387 ms and 25.078/25.589 ms. Recorded framebuffer hashes
matched at every 60-swap checkpoint through swap 900 in both pairs. This is a
repeatable positive A/B result, but not the ten-run deterministic gate, and
candidate p95 still exceeds the 33.333 ms field budget.

`FN64_TEXRECT_TIMING_CENSUS=1` provides the next exact specialization
ranking. It keys successful CPU texrects by the complete combiner, other-mode,
target, LUT, tile descriptor, address controls, tile bounds/parity, and axis
orientation, and reports calls, requested/clipped pixels, total/max elapsed
nanoseconds, and cost per clipped pixel. Periodic cumulative rows plus a
thread-exit tail snapshot close bounded-run evidence; an explicit flush seam
is available for backend teardown. The armed census includes its aggregation
and reporting overhead in outer task/frame clocks, so it is attribution
evidence only, never an authoritative p95 comparison. The disabled path reads
only a cached enable flag and performs no clock, key construction, locking, or
map traversal.

The first complete 1,600-pump census recorded 25,725 successful texrect calls
and 1.290 seconds in the executor. The rank-one exact program accounted for
12,680 calls and 537.8 ms: CI4 texels, RGBA16 TLUT and target, point sampling,
and the exact `fcffffff/fffdf6fb` plus `0000acef/005041c8` fragment state. A
closed admission type now specializes that exact sampler while every other
shape retains the generic path. `FN64_TEXRECT_RANK_ONE_SPECIALIZATION=0`
disables only this admission for same-binary live A/B runs. Mutating each of
the 24 admitted fields falls
back; boundary coordinates, 50,000 deterministic coordinate mutations, and
invalid source/TLUT bytes match the generic oracle. Ten alternating release
rounds reduced the sampler by 69.3 percent and a complete 64x64 texrect draw by
27.0 percent, with identical final target bytes. This is bounded kernel
evidence. Two counterbalanced 1,600-pump same-binary pairs then kept CPU color
batching enabled and changed only this specialization. Candidate/control p95
was 33.447/33.623 ms and 33.584/33.712
ms; mean was 24.233/24.398 ms and 24.319/24.394 ms. Every recorded framebuffer
hash matched through swap 900. The small positive effect repeats, but neither
run reaches the required margin or ten-run gate, so wider texrect shapes remain
subordinate to the higher-ceiling ordered CPU triangle work.

The shell also has a default-off presentation-cache experiment for the observed
two presentation requests per WM2000 swap. Unset or
`FN64_PRESENT_CACHE=0` is `disabled`; `FN64_PRESENT_CACHE=observe` captures and
compares the same exact dependencies while always redrawing; and the existing
`FN64_PRESENT_CACHE=1` value is `suppress`, where an exact hit skips the
pump-driven redraw. The observe/suppress pair therefore supplies the same
logical dependency samples and digest on both sides of a same-binary A/B while
changing only suppression. Startup, heartbeat, and final logs name the mode.
The dependency key owns the VI origin and geometry plus the exact word-rounded
RGBA5551 source bytes; comparison borrows live RDRAM without allocating, and
the snapshot advances only after a successful surface render. A separate
generation invalidates that authority when overlay/HUD composition closes or
toggles, overscan or zoom-fill policy changes, the window resizes or changes
scale/fullscreen, or a surface submission fails. OS redraw/expose, overlay,
tripwire, and frame-dump paths always redraw. Heartbeats and the final
`[fn64-present-cache]` row report the cumulative request/hit/miss denominator,
successful and failed submits, invalidations, and a logical dependency digest
with its sample and byte counts, so skipped submissions no longer disappear
from the experiment. One same-binary run changed p95 only from 40.368 to 40.237
ms, too small for a claim under the measured background-load noise, so it
remains opt-in pending a quiet counterbalanced A/B.

## Sources and nonclaims

The semantic oracle is fn64's existing CPU raster and its cited allowed
sources: public N64 documentation, pinned MIT RT64, and the repository's own
behavioral specs. The plan does not read or use a GPL runtime. It does not
claim that existing RGBA8 diagnostic GPU output is guest-correct, nor that a
GPU result is portable until the same byte-identity gates pass on another
supported adapter.
