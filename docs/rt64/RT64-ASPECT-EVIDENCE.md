# RT64 aspect-ratio evidence

Status: the public widescreen and ultrawide behavior rows are closed by the
bounded raw-DPC control plus a public non-ROM F3DEX2/Extended HLE matrix.

## Source and mechanism

This document uses only the pinned MIT RT64 source allowed by `AGENTS.md`.
Pinned RT64 derives the source aspect from the VI framebuffer, chooses the
configured or swapchain-derived target, computes
`aspectRatioScale = target / source`, and applies that scale to X resolution
([`rt64_workload_queue.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_workload_queue.cpp#L126-L211)).
When that scale differs from one it schedules projection processing
([`rt64_workload_queue.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_workload_queue.cpp#L305-L319)).
RT64 separately converts viewport and rectangle coordinates around their
declared origins
([`rt64_framebuffer_renderer.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/render/rt64_framebuffer_renderer.cpp#L55-L115)).

fn64 already represents this as runtime host policy rather than compiled game
policy. `RenderAspectRatio::{Original, Expand, Manual}` and bounded
`AspectTarget` values cross the complete `UserConfiguration` boundary.
Changing the main aspect mode or active manual target live calls pinned RT64's
update path, discards cached framebuffers, and retains the exact active policy
identity. Extended-GBI aspect mode/target remain separate typed fields because
runtime policy cannot manufacture game camera or UI intent.

## Live bounded result

`rt64_aspect_ratio_behavior` submits one synthetic, non-game raw-RDP image with
an asymmetric red rectangle, green rectangle, and full-frame blue background.
It creates one 112x48 Metal backend, then switches the same live backend through
original 4:3, manual 16:9, manual 2:1, and manual 21:9. Every switch must return
the exact `LiveApplied { framebuffers_discarded: true }` result, install a
distinct complete policy identity, advance presentation IDs, preserve target
guards, bind the completed capture to the exact commanded 64x48 RGBA16 target
and a nonzero managed source texture, and produce two consecutive identical
captures before its result is accepted. Exact per-color bounds/counts and
complete post-VI BGRA8 SHA-256 values reject both a no-op settings path and
ambiguous output-only reporting
([`rt64_aspect_ratio_behavior.rs`](../crates/fn64-render-rt64/examples/rt64_aspect_ratio_behavior.rs#L326)).

Ten consecutive fresh watchdog-bounded processes passed on 2026-07-19 against
clean pinned RT64 on macOS 26.5 arm64 / Apple M5 Pro. Their exact output hashes
were:

| Policy | SHA-256 |
|---|---|
| original 4:3 | `6600d15768d99ff607d67e605505e23b6009b06dac2e3c6f8c49a38ba32d1789` |
| manual 16:9 | `d420b54a9e3c2180c25eaad538e5f9db23b56b86ed6df2b27994a344d7bc39ff` |
| manual 2:1 | `b98b969542d7dc68cd1e4c29246fd3d52346c5e55ae75b5139945e3346492f4a` |
| manual 21:9 | `357f00115a03cb681c92f5e7a8cf48812d78f40efeb40c09434509d660b5a9c1` |

This establishes typed runtime switching and distinct deterministic raw-DPC
post-VI output at arbitrary and ultrawide targets. In isolation it does **not**
establish the advertised game-cooperative behaviors: raw RDP arrives after the
RSP has already projected geometry. The HLE matrix below supplies that missing
denominator.

## HLE closure matrix

`rt64_hle_aspect_behavior` uses the existing opt-in typed admission for one
hand-authored, non-ROM F3DEX2 dialect. It enters pinned RT64's normal
interpreter, workload, render, and presentation path without adding any hash
to production recognition. Normal `process_task` must return `NeedsLle` before
and after the matrix.

One workload combines a transformed asymmetric triangle, explicit viewport,
non-full scissor, and an Extended Adjust rectangle with independent Left/Right
origins and six-pixel offsets. The live backend renders twice per setting at
4:3, 16:9, 2:1, and 21:9. Every transition after the initial mode must take the
typed framebuffer-discard path; semantic shapes and complete post-VI bytes
must stabilize across both presentations and remain bound to one workload and
present ID.

The stable post-VI hashes are:

| Policy | SHA-256 |
|---|---|
| original 4:3 | `6c953ade5120bf0ba4aecc3c2df9da017a05356340bc37c387717a3f51ab24a8` |
| manual 16:9 | `6aaf948790b0a233bb796404abb557d498dbf2335ab92a80aad28cf836da99d6` |
| manual 2:1 | `3ca118f7bdbab3adc5ce63e54fd39b6c146f5f5ab59d4035be6f52dddb436da2` |
| manual 21:9 | `c5aa2d5a1b57db7700e9ceaffad4b818a5ace1b0e61698d79032f45412501e9b` |

The fixture compares the transformed triangle and aligned rectangle width
ratios against native and rejects any non-native case that reduces them to one
horizontal scale. Ten fresh clean Metal processes produced byte-identical logs;
the log-file digest itself is historical manual evidence, not tested by the
fixture, and therefore is not a release gate. This closes widescreen and
ultrawide without claiming that the test-only dialect admission is a production
microcode hash. Separate exact gates now close public Extended command behavior
and renderer-API interpolation; production-recognized microcode/full-ROM and
physical scanout certification remain open.
