# Dispatching a lane: what to put in the brief

Distilled from a session where roughly twenty lanes ran. The practices below
each caught a real defect or prevented a false result; the omissions each cost
a wrong conclusion. Link this from a brief rather than restating it.

## Always include

**Link [the harness traps](RT64-WM2000-HARNESS-TRAPS.md).** Do not restate them
inline. They drift when restated, and the list is now long enough that a brief
that inlines it buries the actual task.

**"Run exactly one ROM at a time."** State it explicitly every time. Two lanes
each running the ROM concurrently is not twice the throughput -- concurrent runs
have twice produced false results, and the machine is oversubscribed at eight
processes.

**The worktree rule.** A fresh worktree from the integration branch; never the
owner's dirty checkout; never a sibling repository unless the card is explicitly
about one.

**"Commit early and often."** Lanes that die mid-flight lose everything
otherwise, and several have. A lane with zero commits after an hour is a lane
one crash away from nothing.

**What is already known.** Name the specific files, addresses, and prior
findings. The best results this session came from lanes handed a precise
starting point -- and the worst waste came from lanes rediscovering what a
committed doc already said.

## The evidence rules that actually caught things

**Fail-before / pass-after, stated for every test.** Not "tests pass".

**Mutation-test the arms you KEEP, not only the one you change.** This is the
single highest-yield rule here. It caught a real gap in nearly every lane that
applied it, including tests that could not fail: a fixture sampling a point
where the correct and incorrect answers coincide (tile 0 when the bug is
"always returns 0"; a mask width where the tested value reads the same either
way; a sweep deriving its expectation from the constant under test).

**Derive expectations BY HAND from the wire layout, never from the code under
test.** A test written from the implementation passes the implementation.

**Prove it on the ROM, not only in the suite.** Several green-suite changes did
nothing to the game, and one lane's ROM run found a defect no unit test could.
Equally: a suite regression is real even when the ROM looks fine.

**Mark every claim CONFIRMED (measured) or HYPOTHESIS.** The lanes that did this
were the ones whose conclusions survived scrutiny.

## The anti-workaround clause

State it explicitly, because the pressure to produce a passing run is real:

> Do not weaken a guard, substitute a placeholder, feed a fabricated value, or
> skip a command to get past this. If a guard refuses, establish what the
> hardware actually does -- with a citation -- before widening anything.

Several apparent defects this session were correct refusals protecting a real
invariant, and admitting the input would have combined against an invented
value. Equally, several real defects were a working capability behind a caller
that refused it. Both shapes exist; measurement tells them apart.

## Invite refutation

The most valuable lane results were the ones that **refuted the brief's own
hypothesis**: a "bank-ambiguous dispatch" card that proved the addresses were
never ambiguous, a "fix the FillRectangle guard" card that found a layering
error instead, a "read what the screen polls" card that reversed its framing.

So write the hypothesis as a hypothesis, and say plainly that refuting it with
evidence is a valid and valuable outcome. A lane that stops at a well-scoped
negative has done real work; a lane that forces a fix to satisfy a brief has
produced something worse than nothing.

## Scope the collision surface

When more than one lane is live, name the files each owns and tell them to stay
out of the others'. Lanes that shared a file produced merge conflicts and, once,
duplicated effort on the same defect from both ends.
