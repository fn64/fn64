# WM2000 harness traps

Every trap here cost at least one wrong result or one lost run. Link to this
file from a brief instead of restating them; add to it when a new one bites.

## Run discipline

**Run exactly one ROM at a time.** Concurrent runs have twice produced false
results. Four runs once aborted at an identical swap and looked exactly like a
real plateau; the cause was an infrastructure collision, not the guest.

**Never `pkill -f wm2000-boot`.** Several sessions run that same binary. Read
`pgrep -fl`, identify your own PIDs, and kill only those. A bare pattern-kill
destroyed another session's 75%-complete run.

**Give every run its own `WM2000_TRACE_PATH`.** The default path is shared, and
sharing it is what produced the false plateau above.

**`WM2000_NO_TRACE=1` also disables frame dumps.** The harness computes
`dumps_disabled = trace_disabled || ...`, so turning off the trace turns off the
pictures you need as evidence. Grep the line out of the run script instead.

## Reading results

**Read the LAST `vi_swaps=` line, and confirm the run terminated.** Progress
checkpoints print every 50,000 steps. A mid-run checkpoint has been mistaken for
a final result more than once -- `825` in particular looks like a plateau and is
not. Termination means `step budget ... exhausted` or `BOOT SUMMARY`.

**An aborted run is not a measurement.** Discard it and re-run.

**"The screen did not change" is not "fn64 wrote the wrong value."** Conflating
those produced a premature "not an fn64 defect" verdict. A screen can hold while
the guest composes at full rate, and a value can be delivered correctly and then
refused by the guest.

## Stale artifacts

**Rebuild `recompile_rom` before every run.** `run-rs-lane.sh` now does this and
refuses to start when the binary is older than any codegen source, because a
stale binary emits a crate without your fix and then "reproduces" a blocker that
is already fixed. That cost two wrong conclusions in one day.

**Probe knobs belong in the harness, committed, behind env flags.** A
`WM2000_PORTS` knob was invented in a scratch copy, used to prove a real finding,
committed only as a `.patch` with absolute paths pointing into `/tmp`, and never
applied -- so the next run silently used one controller and the result was
misread. Patches rot; committed flags do not.

**Do not commit build artifacts.** A lane swept a 130 MB `target-test/`
directory into a commit and GitHub rejected the push. Keep `CARGO_TARGET_DIR`
outside the repository and check `git status` before committing.

## Guest memory

**WM2000 renders 480x237, not 320x240.** Reading the framebuffer at a hardcoded
320 stride shears every row by 160 pixels and turns coherent 3D geometry into
convincing horizontal "striping". That artifact was reported as a renderer defect
three separate times, in two different harnesses, before the reader was
identified as the cause. Read the geometry from the VI registers
(`fn64_abi::vi_width()` / `vi_output_height()`); both harnesses now do.

**RDRAM backing is width-dependent.** `store_h` writes half-words at
`backing_offset(vaddr) ^ 2`; `store_backed_word` writes words little-endian at
the plain offset. A probe using the wrong width or endianness reads a different
variable and yields plausible-looking garbage -- one lane got a stable,
pointer-shaped value that confirmed exactly the hypothesis under test. That cost
an hour and nearly produced the opposite answer.

## Evidence

**An aggregate histogram cannot answer a per-primitive question.** WM2000's
flat models were chased through a frame-wide texel histogram that looked
broad and healthy -- sixteen populated buckets, no bucket over 16% -- and
that measurement refuted both surviving hypotheses and would have closed the
investigation with no defect found. The truth was that 87% of triangles each
read a DIFFERENT single texel, which produces exactly that frame-wide shape.
One extra `HashSet` in the same loop, counting distinct texels PER TRIANGLE,
was unambiguous. Before trusting an aggregate, ask what per-primitive
pathology would produce the same aggregate. See
`docs/RT64-WM2000-COMBINER-CENSUS.md`.

**A fixture that inverts the constant under test cannot detect a wrong
constant.** Two `rdp_harness` perspective fixtures derived their expected
plane values by inverting fn64's own `PERSPECTIVE_TEXEL_SCALE`, so they
asserted the implementation against itself and passed happily under a scale
32x away from hardware -- both with detailed, confident doc comments about
what they proved. This is the "derive expectations BY HAND from the wire
layout" rule failing in the wild, and it is worth grepping a fixture for the
very constant it is meant to pin.

**Mutation-test every fix, including the arms you keep.** The recurring failure
is a fixture that samples a point where the correct and incorrect answers
coincide: testing tile 0 when the bug is "always returns 0", or a mask width
where the tested value reads identically under both. Choose fixture values that
actually distinguish the bug from the fix.

**A capability that exists is not a capability that is reachable.** Several real
defects today were a working implementation behind a caller that refused it, or
a comment asserting a field could not be read next to code elsewhere reading it
correctly. Check the caller before concluding the feature is missing.

**Equally: a refusal is not automatically a bug.** Several apparent defects were
correct guards protecting a real invariant, and admitting the input would have
combined against a fabricated value. Establish what the hardware does, with a
citation, before widening anything.

**A guard's stated reason can expire without the guard noticing.** Three
refusals in this crate have now been removed after measurement showed their
justification described the file at the commit that wrote it rather than the
file today: `TexrectWithoutTmemLoad`, `MixedTexrectAndRawTrianglePacket`, and
`MixedFillAndTrianglePacket`. Each was correct when written, each carried a
carefully argued comment, and each was outlived by a seam that landed later --
the N-command accumulation in `stage_color_commands`, which made Fill, Texrect
and RawTriangle peer citizens of one journal-ordered CPU buffer. So when a
guard's comment asserts two things are "disjoint with no defined ordering",
check whether that is still true of the current call graph before accepting it.
Read the executors the guard names, not only the guard.

**Prefer a per-access invariant to a per-packet shape gate.** All three of those
refusals approximated, at packet granularity, something the journal already
enforces exactly: every declared write claimed once, every staged write declared
(`MergedWriteUnclaimed`/`MergedWriteUndeclared`), every range inside its target
(`FillAccessOutsideTarget`). The shape gate could only be conservative, and each
time it was conservative in a way that dropped real guest-visible commands to
withhold one that was invisible anyway. When retargeting such a test, point it
at the per-access invariant rather than deleting it.
