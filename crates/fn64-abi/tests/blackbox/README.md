# Black-box shim observations

Cleanup-plan task 5.6. `AGENTS.md`'s clean-room protocol allows exactly one way
to learn how the GPL reference runtime behaves: a differential experiment
against it as a black box. This directory is the fn64 half of that experiment.

## What is here

- `<scenario>.json` — an input script: a sequence of shim calls with concrete
  register arguments and the registers/rdram words to observe after each.
- `<scenario>.observed.json` — the tuples the **reference runtime** produced for
  that script, with a provenance header naming the runtime commit, the driver
  commit, the date, the platform, and the exact command.

The observation files are facts about a black-box run. They carry no runtime
code and describe no runtime internals, which is what makes them citable here.
Nothing in this directory is derived from reading GPL implementation sources.

`crates/fn64-abi/src/blackbox_replay.rs` replays every script through fn64's own
shims and classifies each tuple as `match`, `deliberate-divergence` (with the
public libultra manual citation that justifies fn64's behavior), or
`unexplained`. Only `unexplained` fails. It runs in the normal
`cargo nextest run -p fn64-abi`; no GPL code is needed at test time.

A call the driver could not drive as a black box is recorded `not-observed`
with the measured reason, never invented. `osSetTimer` and `osPiStartDma` are
both in that state: the reference terminates on a signal from a bare context
because they need subsystems the driver does not stand up.

## Regenerating an observation

The driver lives outside this repository, in the GPL aki-recomp checkout at
`tools/shim-probe/` (commit `69b504a`, runtime commit `9f06f81e`). It is GPL by
necessity and is never copied into fn64. Build and run instructions are in that
directory's own README. Recording is:

```sh
./tools/shim-probe/build/shim-probe <fn64>/crates/fn64-abi/tests/blackbox/<scenario>.json 2>/dev/null
```

The runtime writes diagnostics to stderr; the observation JSON is on stdout
alone. Paste the result into `<scenario>.observed.json` under a refreshed
provenance header — a recorded observation without its runtime commit, driver
commit, date and command is not a citable fact, and the replay test asserts the
header is present.

## Adding a scenario

Add the script and its recording as a pair, add the pair to `SCENARIOS` in
`blackbox_replay.rs`, and add any shim the script calls to that file's
`call_fn64_shim` dispatch — it panics on an unrecognized shim rather than
skipping the call. If fn64 intends to differ, add a `DELIBERATE_DIVERGENCES`
entry naming the manual section and the exact fn64 value; a divergence that
later drifts to some third value is reported unexplained rather than silently
tolerated.

Scenario addresses are KSEG0 (`0x80xxxxxx`). Seeding `r2` with the sentinel
`0x12345678` is how a shim that never writes `$v0` is told apart from one that
writes `0`; several recorded reference tuples show that sentinel surviving the
call. A scenario may declare host preconditions in a `setup` object (currently
`tv_type`), so both sides are driven from the same file rather than from setup
hidden in the harness.
