# WM2000 timing and performance closure

Status: active execution plan, 2026-08-29. This document is the authoritative
resume point for the current WM2000 timing/performance closure. Historical
measurements and implementation narratives remain in
`WM2000-30HZ-OPTIMIZATION-LOOP.md`, `WM2000-COMPUTE-RASTER.md`, and
`perf-method.md`; when they disagree with this document's current frontier,
re-measure rather than carrying a quoted number forward.

## Outcome

WM2000 must advance at hardware pace, present the intended video field, and
play the corresponding audio cue without starvation. The corrected WGPU
renderer must then meet the full-intro performance bar with exact output
identities. The runtime remains title-neutral and extensible: hardware-visible
state and ordering are authoritative, but host implementation mechanisms may
use AOT translation, GPU compute, SIMD, workers, deferred coherence, and PGO.

This plan is not a claim that A/V synchronization, red/flame rendering,
diagonal striping, uninterrupted audio, or the final performance bar is fixed.

## Frozen facts and current frontier

- Landing worktree: `/private/tmp/fn64-perf-landing`.
- Landing branch: `perf/wm2000-hidden-coverage-row-bins`.
- Committed frontier at plan creation: `e9532e144ef71e9d0247cdb928ede55ca08e03f6`.
- A fresh `git fetch origin main` on 2026-08-29 measured `origin/main`,
  `FETCH_HEAD`, and the merge base at `339a55d0`. The clean integration HEAD
  was `0ce4a624`, 66 commits ahead and zero behind.
  W00 still requires the remaining compiler, binary, feature, corpus, and
  environment receipts before a final build.
- The landing worktree contains a large unrelated unstaged formatter/integration
  spill. No broad reset, restore, clean, or bulk stage is allowed. Use clean
  detached worktrees for builds and certification, and import semantic patches
  narrowly.
- `b6008aba` independently retained the behavior-neutral owned task-shadow
  preflight refactor. It is no longer an unresolved dirty-file decision.
- The exact task-batch replay adapter is branch-only. A direct timing comparison
  with a lane lacking that adapter is invalid even when final RDRAM agrees,
  because commit grouping differs.
- The host-only ACFF production row-bin route is rejected. It admitted only
  22/149 middle-capture members, changed exact identities, and slowed execution
  from roughly 12.3 to 19.6 ms. Do not retry it without first solving durable
  cross-packet TMEM authority and admission.
- Reverse-order intra-member copyback coalescing is rejected for the measured
  red-transition workload: approximately 0.017 ms copyback reduction. New
  evidence or a materially different population is required before reprioritizing.
- Current clean-integration relative-rate evidence showed audio at
  0.9999944256 host seconds per emulated second and video at 0.9998954337,
  approximately -99 ppm video versus audio. The generic video-minus-audio phase
  near -69 ms compares API boundaries, not corresponding content cues, and is
  not a sync verdict.
- The former two-DMA CoreAudio start gate was replaced by host-stream
  preactivation plus a first-active-DMA delivery gate. This removes host
  `play()` from the guest-timed interval but does not by itself close playback
  continuity: bounded default/256/128-frame scouts observed 4/1/2 underrun
  callbacks respectively. The first shortage followed a roughly 52--53 ms
  producer gap near DMA 28; later shortages occurred at ordinary roughly
  33 ms guest production cadence. Callback-size selection and one callback of
  invented host padding are rejected as a correction.
- From DMA 1 through DMA 553, 9.976896 seconds of resampled host PCM covered
  9.977435 seconds of emulated AI starts. Repeating the comparison at roughly
  0.9-second intervals held the difference between 0.536 and 0.556 ms rather
  than growing. This is fixed resampler lookahead/phase latency, not a rate
  mismatch and not permission to steer the emulated clock from host buffering.
- A fresh device-default schema-v9 run retained 1,002 DMA queue records with
  no telemetry loss. Its exact binary digest remains in the private run
  receipt. Its only underrun
  followed a 51.094 ms DMA 28-to-29 producer gap; the nominal window submit
  nearest the callback took only 0.247 ms, so the callback's
  `window_present` phase label does not establish a long presentation call.
  Over the 18.071-second overlap the whole-trace video-versus-audio rate was
  approximately +1.5 ppm, but the stability gate correctly refused a pace
  verdict because the run was short, window estimates disagreed, and audio
  continuity generation changed.
- A bounded 80-pump phase census reproduced the startup gap as pump 29:
  65.838 ms wall, 20,317 executor steps, 64.993 ms executor time, 57.245 ms
  executor-resume gross, and only 0.308 ms graphics plus 0.338 ms audio LLE.
  It therefore localizes the startup starvation to pathological recompiled
  guest-resume volume, not RDP execution, the audio interpreter, VI scanout,
  or the nominal window submission. The actual linked-game route did not
  populate `FN64_RESUME_SPLIT` subcounters because those clocks exist only in
  the catalog/block runners, while WM2000 uses the linked whole-function lane.
- A fresh release run with the linked-lane executor census reproduced pump 29
  at 69.272 ms and 20,317 steps. Across 80 pumps, thread 6
  (`func_800222D8`, the title's overlay-loader thread) accounted for 20,994 of
  23,756 resumes: 10,380 blocking receives and 10,497 instruction
  checkpoints. The runtime separately reveals that live ROM installation
  still selects the explicit one-cycle `FixedPiTiming` compatibility policy.
  This makes the next correction target hardware-derived, length- and
  domain-sensitive PI completion timing: it can explain both compressed guest
  loading time and the host cost of thousands of completion handoffs. This is
  a localization result, not yet a pace, continuity, or performance fix.
- The ordinary live ROM route now seeds Domain 1 from the normalized cartridge
  header and uses the programmed-domain/transfer-geometry `RcpPiTiming` model.
  In ten fresh 80-pump release processes, the presentation dependency digest,
  total 23,496 resumes, and thread 6's 20,894 resumes were identical. The old
  roughly 69 ms/20,317-step pump became a distributed startup population whose
  maximum wall time ranged 26.433--30.396 ms; the original pump 29 measured
  81 steps in the first scout. This validates the intended timestamp and tail
  correction, but disproves the premise that one-cycle PI completions caused
  most coroutine crossings: thread 6's total population barely changed.
  Continuity is still open; underrun-event counts were
  `1/0/2/0/0/1/2/0/0/3`. The next CPU discriminator is whether linked
  whole-function instruction checkpoints can be coalesced at boundaries that
  retain exact device/event visibility, not another guest-clock adjustment.
- That discriminator passed in one fresh 80-pump diagnostic run from shell
  `e9f61bc7...`. Thread 6 had 11,167/11,167 instruction checkpoints at the C
  lane's synthetic 250-cycle OS-call precharge; 10,827/11,167 (97.0%) next
  yielded on a blocking receive, and 11,162/11,167 (99.96%) reached the owner
  again without another coroutine resume interposed. The leading candidate is
  therefore a typed checkpoint-plus-deferred-yield action which retains exact
  deadline advancement and scheduler arbitration while avoiding the bridge
  coroutine crossing. Directly applying the receive at the checkpoint is not
  equivalent: devices, equal-cycle timers, interrupts, and higher/equal
  priority threads may become observable before the underlying OS call.
- The resulting typed checkpoint-plus-deferred-yield candidate is rejected.
  It halved physical coroutine resumes (23,496 to 11,763 in comparable
  80-pump populations) but retained the same scheduler-turn geometry and did
  not produce a clear wall-time gain. With identical phase gates, the three
  control startup maxima were 31.764/28.865/27.393 ms and the three candidate
  maxima were 28.358/27.145/29.293 ms; the ranges overlap. All six retained
  the same canonicalized presentation sequence digest `9177d84b...`. This
  disproves physical coroutine crossing as the dominant remaining cost. Do
  not retry the shape without removing work from a scheduler turn rather than
  merely moving that work across the coroutine boundary.
- Exact raw-DPC visual-checkpoint vocabulary now binds a task-batch member to
  its live transaction, exact command-completion reads, device-order target,
  physical hidden coverage, and complete post-copyback RDRAM. It refuses the
  current reconstructed/pristine replay route by name. This establishes the
  input contract for a flame/red differential but is not backend wiring or a
  fidelity result.
- A five-second macOS textual sample of the current bounded intro supplied a
  second performance perspective after the startup population. Its active
  renderer tops included VI dither restoration, bilinear resampling, TMEM
  reads/filtering, texrect blending, and scalar raw-triangle rasterization.
  The sample was not aligned to an exact flame checkpoint, so it prioritizes
  mechanism experiments but cannot attribute a visual defect or certify a
  flame-specific speedup.
- The current-source release binary after removing the rejected callback
  experiment is digest-bound in the private run receipts. Ten consecutive
  80-pump processes retained identical VI presentation and
  guest-task identity sequences (`ca04d88e...` and `68ce4fac...`) with complete
  telemetry. Continuity was not fixed: underrun-event counts were
  `0/2/4/2/3/2/2/4/2/2`, and maximum queue gaps ranged from 49.124 to
  56.808 ms. This is a deterministic-output bar for the retained startup and
  measurement mechanisms, not an audio-correction bar.
- The latest retained PMU capture attributed large inclusive populations to raw
  triangle rasterization and the audio RSP interpreter. It may predate the exact
  final sampler/cache implementation, so it selects experiments but cannot
  certify the final binary.
- A fresh full-intro PGO workflow and final 6,000-pump visible run are still
  required from the finalized source. Historical visible results are context,
  not current certification.

## Authorities and source boundaries

Use the fn64 device trace for internal event authority, RT64 and permissive
hardware references for renderer mechanisms and exact workload differentials,
and Mupen or N64ModernRuntime only through public documented/debugger interfaces
as black-box behavioral oracles. GPL runtime implementation code is excluded.
Measured ROM facts from reverse-engineering projects are allowed under
`AGENTS.md`; any newly used project must be recorded in `DISCOVER-PLAN.md`.

The legacy C lane is not a correctness authority while its callable-body audit
finds empty bodies. It may still inform mechanism and performance. Matching an
observed horizon in `lane-parity.sh --observe` does not promote it.

## Acceptance contract

All claims bind the exact source commit, dirty-diff digest if any, compiler,
binary digest, private corpus digest, renderer, feature set, environment,
warmup, measured population, and machine state.

### Timing and A/V

1. Guest VI, AI, timer, and scheduler events share typed monotonic emulated
   time. Host wall time never becomes guest device authority.
2. Two or more externally identified, separated video/audio cue pairs measure
   both fixed phase and relative pace. A nearest-cycle or API-return pairing is
   diagnostic only.
3. The cue trace records guest cycle, host time, video occurrence, AI DMA and
   sample offset, output-ring continuity, underrun/drop/retime state, and an
   opaque cue ID. Invalid continuity refuses a sync verdict.
4. A deterministic timing correction needs 10 consecutive clean runs. A
   stream/thread/queue correction needs 20 or more and names the exact closed
   interleaving in a fix-site comment.
5. The reference and fn64 lanes must use the same externally observed cue
   definitions. Report guest-cycle delta and predicted host-playback delta
   separately.

### Rendering and performance

1. Exact committed target identity and final RDRAM postimage remain stable.
   Optional private SHA-256 or direct-byte evidence must back exact-frame
   claims; FNV alone is a lookup key.
2. Visual gates separately cover the known diagonal stripe signature, red/fog
   severity, flame filtering/tiling, and the corresponding visible frame.
   Disabling one artifact detector cannot stand in for the others.
3. The final native WGPU/live-audio profile-use run uses 300 warmup and 6,000
   measured pumps and reports drawn-frame mean, p50, p95, p99, maximum,
   over-30 ms count, over-33.333 ms count, audio continuity, and task-batch
   identity closure.
4. The target remains drawn-frame p95 at most 25 ms, p99 at most 28 ms, no
   frame over 33.333 ms, at least 97% gap-two cadence, and 10 consecutive clean
   certification runs. Until every condition holds, report `not verified`.

### Progressive experiment policy

- Scout order is control, candidate, control.
- Stop an obvious reject after three fresh processes only when the candidate is
  at least the predeclared 1.0 ms guardrail slower than both controls.
- Otherwise complete a balanced 2+2 scout.
- Promotion uses six timed controls, six timed candidates, and four additional
  candidate identity closures: 10 independent candidate closures total.
- Concurrency changes always retain the separate 20-or-more-run bar.
- Keep one variable per A/B. Re-profile retained candidates to prove cost fell
  rather than moved.

## Hypothesis register

Each hypothesis remains open until its named discriminator runs. Mechanism
differences are localization evidence, not proof of incorrect behavior.

| ID | Hypothesis | Fast discriminator | Acceptance or rejection evidence |
|---|---|---|---|
| H01 | Live audio RSP interpretation on the emulation thread consumes material wall time and serializes guest progress. | Exact-final-binary PMU plus per-task mechanism census; digest-bound translated callback A/B. | Retain only with exact live-IMEM/overlay/entry identity, loud unknown-image fallback, unchanged audio bytes/events, and measured end-to-end gain. |
| H02 | CPU-first RDP rasterization and synchronous publication serialize work hardware performs as a coprocessor. | Join CPU/GPU timestamps to task, coherence, and VI-presentation records. | A device-resident task/burst A/B must preserve exact publication identities and reduce visible/replay critical path. |
| H03 | Coherence barriers, readbacks, and checkpoint materialization occur before any guest or VI consumer requires them. | Log every readback/coherence reason and first subsequent consumer. | Defer only boundaries proven consumer-free; reject on changed generation, queue, task, or framebuffer order. |
| H04 | Recompiled functions yield/checkpoint at a granularity that serializes work more often than N64-visible scheduling requires. | Mechanism census with function/block entry, yield reason, device cycle, and runnable-thread state. | Change only at boundaries justified by libultra or hardware authority; first-divergence trace must stay clean. |
| H05 | Scheduler or libultra event timing differs before the visible pace symptom. | Producer-neutral fn64 versus black-box trace comparator reporting the first semantic divergence. | Earliest divergence repeats for one fixed input/corpus and disappears after the correction without later drift. |
| H06 | A startup pump performs pathological recompiled guest-resume volume and exhausts marginal audio runway after delivery activates. | **PI timing correction validated; resume-cost mechanism localized.** Programmed-domain/geometry timing reduced the roughly 69 ms/20,317-step spike to 26.433--30.396 ms across ten runs while preserving exact counts and presentation identity. A fresh 80-pump transition census found all 11,167 thread-6 checkpoints were the synthetic 250-cycle OS-call precharge, 97.0% next yielded on blocking receive, and 99.96% had no interposed coroutine resume. | Implement a typed checkpoint-plus-deferred-yield action, not an eager queue operation. Retain guest-authorized first-DMA delivery, exact device/equal-cycle timer order, interrupts, priority arbitration, and the second catalog publication seam; preserve exact output/event identity and run the 20+ concurrency bar. A latency buffer remains a separate typed fallback, never a guest clock source. |
| H07 | Host event-loop scheduling or presentation stalls cause late audio and choppy heavy scenes despite acceptable pump work. | Join pump, event-loop wake, renderer completion, presentation, audio callback, and underrun timestamps. | Attribute every large wall gap; retain a change only when continuity and visible distribution improve together. |
| H08 | Current presentation timestamps observe renderer API return rather than physical/display presentation. | Trace successful presented field and, where available, backend completion/display timing. | Keep phase labels explicit; never promote API-boundary phase to physical A/V sync without an applicable timestamp. |
| H09 | Missing, fallback, or misrouted recompiled bodies change workload or scheduling relative to the intended program. | Callable-body census, dispatch mechanism trace, and existing codegen oracles. | Zero unexplained fallback for the certified route; legacy C remains non-authoritative until its audit precondition holds. |
| H10 | Runtime validation, hashing, journaling, or evidence retention remains on production hot paths. | Exact-final-binary inclusive PMU plus armed/disabled same-binary counters. | Remove or defer only redundant host work; retain the independent authority needed to detect guest-visible divergence. |
| H11 | Lazy shader/pipeline/resource creation creates heavy-scene spikes. | Per-resource first-use records joined to pump and GPU timestamps. | Prewarm/cache only content-independent keys; unchanged outputs and lower tail latency across repeated cold and warm runs. |
| H12 | Missing or stale PGO leaves significant native-code layout and branch headroom. | Supported instrument/train/merge/use workflow with full receipts. | Retain when a fresh profile-use binary improves the fixed population without identity or continuity regressions. |
| H13 | The fast lane and fidelity lane execute materially different rendering state, explaining both visual and performance differences. | Same captured task batches through exact CPU, compute, and RT64/reference observations with state-key census. | Optimize the correct programmed state; a visually wrong fast lane is not a performance control for corrected rendering. |

## Work graph

The IDs below are stable. `depends` is the minimum prerequisite set; a work
package may run beside another package only when their declared write sets do
not overlap.

| ID | Work package | Depends | Deliverable and exit evidence |
|---|---|---|---|
| W00 | Freeze latest-main-plus-perf inputs | — | **Source identity measured; other receipts pending.** Fresh fetch measured clean integration HEAD `0ce4a624`, remote main and merge base `339a55d0`, and left/right count `0/66`; emit compiler, binary, feature, corpus, and environment receipts before final timing. Do not time unmatched commit grouping. |
| W01 | Mechanism-parity census | W00 | **Mechanism implemented; live population pending.** Content-free schema v8 retains each admission-keyed guest task with CPU dispatch lane, RSP interpreted/translated/unavailable lane, RDP CPU/compute/unavailable/not-applicable state, emulated start/end, host span, thread/queue identity, terminal outcome, and coherence reason. Focused lifecycle paths passed 10/10 on v7; final-source v8 population remains W06 input. |
| W02 | First-divergence comparator | W00 | Implemented by `gate_timing_diff` over the producer-neutral device-trace wire: reports the first strict semantic divergence and refuses incompatible identity/schema/clock/scope, same-producer, empty, ambiguous-resolution, and truncated/aborted evidence. Synthetic pass, divergence, ambiguity, and truncation tests own the gate. |
| W03 | Exact A/V cue records | W00 | **Instrumentation implemented; live evidence pending.** Presentation schema v8 retains the v7 exact-cue contract binding `FN64_AV_SYNC_CUE_ID` to exact video occurrence and audio DMA/sample records, captures callback continuity generation, and emits a pair only while continuity remains valid. Final-source v8 serializer/join tests and the summarizer passed 10/10 fresh invocations; callback publication, nested phase unwind, and producer-stop-before-terminal-drain tests passed 20/20. Private two-cue smoke evidence remains required. |
| W04 | Supported PGO workflow | W00 | **Complete.** The reviewed `perf/pgo-workflow` mechanism supplies manifest-owned instrument/train/merge/use and ordinary builds, isolated targets, compatibility receipts, hostile content-free tests, and CI coverage; no raw ad-hoc flags or private route enters fn64. |
| W05 | First-active-DMA stream start | W03 | **Mechanism implemented; continuity correction incomplete.** A move-only active-DMA/payload authorization replaces the two-payload threshold, host preactivation moves `play` before the wall epoch, and schema v9 separates `play` return from guest-authorized delivery. Programmed PI timing passed a ten-process deterministic identity/count bar and reduced the startup maximum to 26.433--30.396 ms, but callback traces still recorded `1/0/2/0/0/1/2/0/0/3` underrun events. The remaining thread-6 checkpoint/receive population, not PI completion latency, is the next measured CPU target. |
| W06 | Exact-final CPU/GPU profile | W01, W03, W04 | **Schema mechanism complete; live profile pending.** PMU inclusive/exclusive profile, GPU timestamps, presentation join, and per-mechanism cost table must come from the same final-source candidate and population. Schema v8 retains worker-thread CPU duration and adds per-callback underrun reason/depth/active-host-phase plus one VI operation span and separate window-submit spans. Final-source deterministic joins passed 10/10 and callback/teardown interleavings passed 20/20; scheduler or sampling evidence remains required to divide other non-CPU wall among blocking, driver waits, and preemption. |
| W07 | Digest-bound translated RSP experiment | W01, W02, W06 | Private artifact binds complete live IMEM generation, entry/resume, and overlay lineage; unknown images trap or loudly use the already-authorized accuracy fallback. Exact audio/event differential plus progressive A/B. |
| W08 | Device-resident RDP/coherence experiment | W01, W02, W06 | First-consumer proof, task/burst-resident target and TMEM, bounded readback, exact generation/checkpoint publication, GPU timing, and progressive A/B. Do not revive host-only ACFF admission. |
| W09 | Event-loop and presentation closure | W02, W03, W05, W06 | Every heavy wall gap classified among guest, render, GPU wait, present, OS scheduling, and audio callback; exact cue pace/phase and continuity measured over the visible route. |
| W10 | Visual differential closure | W02, W03 | Corresponding-frame evidence for red/fog, flames/filtering, and diagonal stripes against an allowed exact authority or black-box reference. Rendering fixes keep exact task/postimage gates and 10 clean runs. |
| W11 | Incremental optimization loop | W06 | One-hotspot candidates run through progressive replay policy; retained changes include focused tests, mutation/identity evidence, paired timings, and a fresh profile. |
| W12 | Final PGO and visible certification | W05, W07, W08, W09, W10, W11 | Fresh full-intro training and profile-use build from finalized clean source; 6,000-pump visible metrics, exact A/V cue verdict, visual gates, identities, 10 deterministic runs, and 20+ for every concurrency fix. |
| W13 | Handoff and durable docs | W12 | Update behavior docs in the same commits, run `scripts/lint-docs.py`, retain compact receipts outside git where required, and record remaining nonclaims or disproved hypotheses. |

### W10 visual differential and enhancement boundary

Visual comparison uses one manifest per source: original hardware capture,
black-box emulator, RT64, or fn64. The manifest retains the source class,
region, renderer and policy identities, active VI geometry, and capture-chain
transform. Private ROM and image material stays outside git. A source with no
exact guest-frame anchor is qualitative evidence only; video timestamps and a
similar-looking pose do not establish corresponding-frame identity.

The exact lane starts from the same admitted task batch and joins guest VI
cycle, presentation stage/generation, task/postimage identity, and the native
active-image crop before comparing output. It reports, separately:

1. exact pixel/hash agreement where an exact authority exists;
2. red/fog channel distributions and spatial error, flame edge/alpha area and
   repeated-tile periodicity, and the diagonal-stripe detector;
3. temporal persistence and change rate across adjacent corresponding fields;
4. CPU execute, GPU work, copyback, presentation, and total cost for that same
   task population.

An online video can reject a gross qualitative hypothesis, but compression,
scaling, deinterlacing, unknown emulator use, and unknown region/capture timing
prevent it from closing pixel or pace parity. Hardware capture or a verified
black-box oracle aligned to exact guest state remains the closure authority.

The current evidence narrows, but does not close, the flame defect. The exact
production capture proved that the affected textured triangles programmed
Bilinear filtering while the old CPU paths sampled one point texel. The
retained prepared sampler removed the repeated rectangular flame tiles in
exact fn64 frames and preserved an unaffected control frame byte for byte.
That establishes the old blockiness mechanism; it does not establish that the
remaining red intensity, mesh, or filtered output matches hardware. Likewise,
the exact fragment specializations are byte-equal to fn64's generic path, not
to silicon, so their differential excludes specialization drift without
promoting the shared arithmetic to an external oracle.

The next W10 discriminator is deliberately two-stage:

1. Replay the exact pre-red through red-onset task batch one member and
   checkpoint at a time through WGPU and the independent reference backend,
   retaining separate visible-color and hidden-coverage identities. The first
   differing member localizes RDP state, interpolation, sampling, blending, or
   checkpoint publication without relying on a similar-looking video pose.
2. Feed the identical live VI register snapshot, source bytes, and coverage
   state to both post-VI paths and compare their output identity. Matching
   pre-VI state with different post-VI output localizes VI filtering; an
   earlier difference keeps the investigation in RDP. Record the complete VI
   filter tuple because a STATUS change can alter that conclusion.

Build this discriminator by extending the existing `raw_dpc_replay` member
loop and per-member task receipt, not by adding a second replay authority. The
backend-neutral receipt needs target geometry plus separate SHA-256 identities
for visible bytes, canonical per-pixel stored coverage (`0` unknown, `1..8`
known), and the complete postimage. Concrete diagnostic accessors may expose
WGPU's resident target/coverage projection and the reference backend's hidden
sidecar without promoting either representation into `RenderBackend`.

Four authority gaps are explicit prerequisites rather than implementation
details: the current loader hardcodes XBUS input; its per-member reads come
from one pristine guest image rather than proven temporal payloads; current
dumps do not bind all VI registers and field state to the selected member; and
WGPU VI cannot consume its warm hidden-coverage registry. The measured WM2000
AA2/resample-only field can proceed through the VI comparison without that
last capability. AA0/AA1 must refuse exact post-VI closure until coverage is
plumbed into scanout. The reference `RawDpcBatch` remains a diagnostic
localizer, not hardware certification.

The qualitative reference ladder is original-hardware footage when its
capture chain is known, a black-box Mupen capture, the pinned RT64 comparative
lane, then fn64. A public online recording may supplement the first lane but
never substitutes for an exact guest-frame anchor. The same manifest format
and scene landmarks apply to every lane so region, aspect, crop, scaling, and
deinterlacing differences remain visible rather than being normalized away.

The faithful profile always executes the programmed RDP/VI semantics. A host
implementation optimization may ship there only when it preserves the exact
output and authority gates. A visual change belongs to an explicit typed
enhancement control instead. Aggregate `upgrade` or `remaster` profiles may
select such controls, but the resolved individual fields and complete policy
digest remain the evidence identity; no title-, scene-, address-, or texture-
specific flame exception enters the runtime. Presentation-only scaling,
deinterlacing, and color transforms stay distinct from semantic changes such
as higher-precision blending, non-RDP texture reconstruction, or altered
noise/dither so each can be A/B tested and disabled independently.

The implementation boundary is one canonical faithful target plus optional
host-only derivatives. Enhancements may consume typed semantic state or the
immutable faithful image, but may not mutate guest RDRAM, hidden coverage,
task/DP ordering, framebuffer-read inputs, or canonical hashes. Existing typed
`RenderRuntimePolicy` and its canonical digest are the aggregate identity.
Faithful resolves its fields explicitly to original resolution/aspect/refresh,
one-times scale, no MSAA, faithful enhancement modes, console emulation, and
shell zoom-fill disabled; overscan remains explicit. A remaster preset is only
a resolver for those individual typed fields and any new separately typed
flame reconstruction, color grading, or higher-precision blending controls.
Existing resolution, scaler/filter, aspect, internal-format, texture-LOD, and
presentation controls should be wired before inventing another family. Any
new control must name one mechanism such as presentation color transform or
non-RDP texture reconstruction; an aggregate remaster profile is only a named
selection of those independently digestible fields.

### Execution waves

1. W00 is the sole source-freeze gate.
2. W01, W02, W03, and W04 may proceed in parallel after W00 when their write
   sets are disjoint.
3. W05 follows the cue schema. W06 follows instrumentation and PGO readiness.
4. W07 and W08 are independent architecture experiments after W06. W09 can
   proceed once audio startup and joined profiling are available. W10 can run
   alongside those packages with separate capture ownership.
5. W11 consumes the exact-final profile and repeats until its acceptance bar
   is met or every sized candidate is rejected.
6. W12 is the convergence gate. W13 publishes only what W12 actually proved.

W01's runtime mechanism is retained in host-presentation schema v8. StartGo
creates a record only after retaining `(task_offset, admission_generation)`;
yield terminates that generation, while a resumed admission gets a new key and
an optional predecessor generation. HLE continuation and LLE raw-DPC ownership
are move-only, and raw completion joins the actual backend member mechanism.
This status does not claim a private-ROM run, population parity, cost
attribution, GPU timestamps, or a performance improvement; those remain W06
measurement work.

The schema-v8 summarizer labels its generic phase as an API-boundary residual:
it compares predicted cpal playback with window-present return and therefore
does not claim content A/V phase, physical display scanout, or acoustic output.
An exact sync claim still requires two or more separated externally identified
cue pairs. Its whole-trace ppm fit is likewise diagnostic, not a pace verdict.
For a single-trace verdict it divides the observed common emulated interval
into four equal, content-neutral partitions. A first partition whose OLS
rate-ratio interval disagrees with every later partition is reported separately
as startup or debt recovery; the remaining partitions must agree with each
other. The common interval must span at least 60 emulated seconds, and each lane
needs at least three observations in every partition. The summarizer also
refuses a verdict across telemetry loss or audio-continuity generation changes.
Missing monotonic audio-DMA anchor IDs are reported as expected sampling
coverage gaps because the shell retains the latest anchor at presentation
cadence; they are not diagnostic transport loss. Ready VI fields without a
successful exact-identity window submission are likewise reported as
presentation coverage rather than automatically invalidating a rate fit. The
OLS interval is a within-trace disagreement detector, not a calibrated
independent-sample population confidence claim; final pace still requires
repeated exact cue evidence under this plan's validation bars.

The first live v8 smoke trace demonstrates why that distinction is load-bearing.
Its all-overlap diagnostic was -686.3 ppm, but the first-partition catch-up,
disagreeing later partitions, and 9.856-second common interval make the pace
verdict `refused`. The 39 sampled-away audio-DMA anchors and three
ready-but-unsubmitted VI fields remain visible coverage facts, not refusal
reasons. Removing fixed amounts of startup changed the same trace's fit from
-686.3 ppm to values near zero, so no fixed warmup cutoff is promoted into
policy. Its stable -146.739 ms median remains only an API-boundary offset; no
exact cue was requested.

## Measurement loop

For every candidate, write the following before running it:

1. exact hypothesis ID and predicted affected metric;
2. source/binary/corpus identities and one control population;
3. correctness oracle and mutation that would make it fail;
4. fixed reject threshold and maximum process budget;
5. possible mechanism, visual, continuity, and instrumentation confounders.

Then run the smallest discriminator first. An obvious regression stops after
three processes. Ambiguous scouts reach balanced 2+2. Only promising work pays
the 6+6 timing and 10-candidate identity bar. Re-profile a retained candidate
before choosing the next hotspot. Do not aggregate away individual receipts.

## Orchestration target

The completed tooling should expose one content-free command that:

1. verifies a clean, frozen source and no competing Cargo/rustc process;
2. builds or selects the exact binary and records its digest;
3. runs trace/cue/profile/replay or visible populations under an explicit mode;
4. applies the progressive decision policy;
5. checks identities, continuity, trace completeness, and required run counts;
6. writes a path-free summary plus private mode-0700 raw artifacts outside git;
7. exits nonzero on missing evidence rather than reporting a partial success.

This is an orchestration layer over existing tools, not a second authority or
a title-specific runtime policy.

## Repository hygiene and resume rule

No ROM bytes, captures, screenshots, generated game content, private cue data,
or PGO profiles enter git. Before each commit, inspect the exact staged paths
and diff; every commit carries the required `Co-Authored-By` and
`Claude-Session` trailers and records measured evidence plus nonclaims.

At resume, read this document, run `git status`, verify W00's identities, and
start the lowest-numbered unblocked package. Update this document when a work
package changes status, a hypothesis is rejected, or an acceptance criterion
changes. Investigation transcripts belong in private receipts or historical
evidence docs; this file retains the current decision and the evidence needed
to reproduce it.
