# How to sequence this work, and why the current loop is slow

Written after a day in which the renderer advanced substantially and the
*method* repeatedly cost more than the fixes. This is a plan, not a
retrospective: it says what to build next and in what order.

## The diagnosis, from this day's own evidence

Every real defect was found at the **integration** level -- a ROM run aborting
-- and then reproduced at unit level by a lane spending 20-40 minutes on it.
That is the pyramid upside down, and the waste is measurable:

- Three scripted input-responsiveness attempts, ~90 minutes of ROM time,
  answered nothing. The owner settled it in about thirty seconds with a pad.
- One run was sized at 40M steps (12+ hours) to answer what 2M steps answers.
- A build-cache optimisation was scoped, then measured at **2%** of cycle time
  and abandoned. Build+recompile is ~46s; execution is 30+ minutes.

**The instrument that worked is the one that is fast.** The parity runner
(`docs/RT64-PARITY.md`) is adapterless, needs no ROM, runs in seconds, and it
found the scissor-Y defect and the partial-fill refusals, then *scored* the fix
6/10 -> 9/10. That is the first time a change here was scored rather than
asserted.

**So the gap is not "we lack a test pyramid". It is that the pyramid's fast
layer does not cover the code we are changing.** The parity corpus is 10
hand-authored cases, **fill-cycle rectangles only** -- no triangles, no
textures, no combiner, which is exactly where every open bug lives.

## Priority 1: extend the fast oracle to the code under change

Ordered by how much open work each unblocks.

1. **Triangles and texture sampling in the parity corpus.** Today's open
   defect -- every model flat/untextured -- is invisible to the current corpus.
   255,654 admitted triangles all call `sample_point` and none is checked
   against an oracle. This is the single highest-value gap.
2. **Combiner programs.** The lead hypothesis for flat models is that `Texel0`
   is sampled correctly and then discarded downstream. A corpus that varies
   combiner inputs would answer that in seconds instead of a ROM run.
3. **Real captured streams.** `FN64_GBI_PACKET_DUMP` emits replayable TSV and
   the runner reads it; the path is wired and tested but no capture is
   committed. Ten cases someone imagined is a regression instrument; a hundred
   packets the game actually draws is a fidelity measure. **One ROM run
   converts one into the other.**

## Priority 2: the oracles we own but do not execute

We have three and use them unevenly:

| oracle | status | authority |
|---|---|---|
| **RT64** | wired as a conformance runner, builds and renders live | command semantics, geometry, combiner, texture. **NOT** downstream of coverage -- no hidden-bits sidecar, memory-alpha hardcoded to 1.0f |
| **angrylion** | **read by humans as C source; never executed** | cycle-accurate silicon behaviour, including coverage/AA/dither where RT64 is silent |
| **fn64-render-reference** | in-tree, used ad hoc | repeatedly MORE correct than the port -- it already implemented fill-cycle rects, scissor clamping, and the shade semantics wgpu refused |

**angrylion is the largest unexploited asset in the project.** Every hardware
citation this week came from a human reading its C. Wiring it as a fourth
conformance runner would make coverage/AA/dither adjudicable instead of
permanently "UNKNOWN" in the guard audit, and would give a silicon-accurate
third opinion wherever RT64 and the reference disagree.

## Priority 3: gates, so regressions cannot ship silently

`RELEASE-GATE.md` gates determinism and byte-identity. CI (`.github/workflows/ci.yml`)
runs the workspace suite and doc lints on Linux. **Neither gates parity, and
neither gates the boot ladder.** Both instruments exist; both run only when
someone remembers.

- **Parity gate.** Fail the build if RT64-authoritative parity drops below its
  committed number. Cheap and adapterless -- but note the RT64 runner is
  macOS-only and needs the C++ build, so this is a local/self-hosted gate, not
  a GitHub-Linux one.
- **Boot-ladder gate.** `docs/tools/wm2000-boot-ladder.sh` already asserts the
  ROM reaches a measured floor (48,000 swaps, measured 53,485) with zero panics
  and zero backend errors. It is not wired to anything.
- **No perf gate exists at all**, and the repo says so: a perf regression ships
  silently today.

## Priority 4: batch size and merge discipline

The branch model is already right and already working: `port/rt64-conveyor`,
535+ commits, every lane cherry-picked only after both suites verify. Keep it.
Two changes worth making:

- **Merge smaller and sooner.** Several lanes today ran 40-90 minutes and
  landed 15-20 commits at once. A lane that lands its first provable increment
  in 10 minutes and then continues is strictly better -- one lane lost its work
  to a `git reset`, another to a machine sleep, and both would have kept
  everything under this rule.
- **The integration break neither lane could see.** The fill lane added a
  struct field; the parity runner, merged separately, did not set it. It only
  failed where the two met. A cheap guard: after cherry-picking any lane, build
  the feature-gated binaries too, not just the default workspace.

## The sequencing rule this all reduces to

**Before fixing a defect, ask whether the fast layer can see it. If it cannot,
extend the fast layer first -- that is usually cheaper than the ROM run you
were about to do, and it keeps working afterwards.**

Today's counter-example is the one to remember: the texture defect was chased
with a 40M-step ROM budget when a combiner-input tally in the parity corpus
would have answered it in seconds -- and the corpus still does not cover it.
