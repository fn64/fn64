# fn64

**A Rust runtime for N64 static recompilation. MIT-licensed, because sharing
shouldn't require a lawyer.**

`fn64` runs the C emitted by an N64 static recompiler ([our fork of
N64Recomp](https://github.com/fn64/n64recomp), MIT) as a native desktop app.
It's the layer under the game: scheduler, message queues, timers, DMA and
overlay lifecycle, save/input/audio, RSP task dispatch. That layer exists
elsewhere — GPL-3.0, C++, and carrying race conditions we've personally
excavated. So we're building the one we actually want.

## Why

- **License, obviously.** Everything here is MIT OR Apache-2.0, end to end.
  Ship it, fork it, embed it, never ask permission. (No game content or
  ROM-derived bytes live in this repo — users recompile their own ROMs,
  locally, always.)
- **We already paid the tuition.** We spent weeks inside the incumbent runtime
  with lldb and hardware watchpoints: a scheduler handoff that lets two "only"
  threads run at once, queue structs corrupted through bypass writes, a lost
  function that fell off the end of its own C body. We wrote down every
  invariant that code was supposed to keep. fn64 is those invariants with a
  compiler enforcing them — Rust's ownership model turns our bug archaeology
  into type errors.
- **General on purpose.** The ABI belongs to the recompiler, not to any one
  game. fn64's core has zero game-specific assumptions; the libultra surface
  is implemented exactly as far as shipping ports need, and everything else is
  a trap that *names itself* when hit. No silent shrugs.

## Crates

One workspace, separate crates, each publishable alone:

| Crate | Role |
|---|---|
| `fn64-runtime` | Core: scheduler, OS message queues, timers, PI/SI/VI/AI plumbing, rdram model, overlays |
| `fn64-abi` | The `extern "C"` surface recompiled code links against |
| `fn64-boot-harness` | Shared generated-section bridge, registration callback, and RDRAM allocation for boot hosts |
| `fn64-shell` | The executable: window, input, audio out, ROM intake |
| `fn64-render` | Backend-agnostic render seam + the pure-Rust `ReferenceBackend` (headless CI oracle) |
| `fn64-render-rt64` | FFI bridge to [RT64](https://github.com/fn64/rt64) (MIT, C++) — all C++ interop quarantined here |
| `fn64-recomp` / `fn64-recomp-rs` | The Rust-emitting recompiler and its whole-ROM driver — the `rs` lane |
| `fn64-audio` | RSP audio ucode execution |
| `fn64-diff` | The first-divergence comparator (pure; no I/O) |
| `fn64-discover` | ROM discovery: symbol/section metadata without a decomp |

`fn64-recomp` was once "planned, once the runtime earns it" — it exists and
boots OoT. See `docs/DESIGN.md` §1.1 for the two lanes it introduces.

## Status

Pre-alpha, but not speculative and no longer design-only: fn64 boots OoT with
the RT64 renderer, real audio ucode, and input, in a windowed shell. The
graded target is **Ocarina of Time** — the zeldaret decomp's 10,833 named
functions make it a rare answer key to grade discovery and recompilation
against. (The original proving ground was a WM2000/No Mercy port effort on the
AKI engine; that lineage is why `aki-recomp` appears in older docs. It is a
legacy checkout fn64 is cutting loose — see ROADMAP Phase H.)

fn64 and the reference runtime link the *identical* recompiled code, so every
fn64 behavior gets A/B'd against reality before the swap. The boot ladder is
the test suite; it does not grade on a curve. What is NOT done is tracked
honestly in `docs/ROADMAP.md` — audio is still broken (R5) and the outdoor
gameplay eye-gate is unmet (R3b).

### Discovery corpus

Function-boundary discovery is graded against decomp answer keys under a strict
`wrong == 0` posture: a discovered boundary that splits a real answer function
fails the grade — only machine-checked evidence promotes a boundary, nothing is
guessed. The numbers below grade the ROM's **resident boot bank** (the code
present at the entrypoint before any overlay DMA — a few hundred functions per
game, not the whole ROM; OoT's 10,833 total live mostly in later banks and
overlays). Recall on merged `main`:

| Game | Boot-bank matched / total | Recall | Wrong |
|------|---------------------------|-------:|------:|
| Ocarina of Time (primary answer key) | 116 / 137 | 84.7% | 0 |
| Majora's Mask | 399 / 486 | 82.1% | 0 |
| Super Mario 64 | 2816 / 3030 | 92.9% | 0 |
| Kirby 64 | 402 / 531 | 75.7% | 0 |
| WM2000 (AKI, NWXE) | 603 / 847 | 71.2% | 0 |
| No Mercy (AKI, NW4E) | 754 / 985 | 76.5% | 0 |

`wrong == 0` holds across every game. The open remainder is an honest,
characterized gap — genuinely unreferenced library/dead code (Kirby 64),
struct-callback dispatch that needs runtime information static analysis cannot
soundly recover (AKI), and variable-length script callbacks — not
mis-attributed boundaries.

## How we work

This is an **agent-forward** codebase: most code is written by AI agents in
orchestrated waves, humans direct and hold the gates. That's not a disclaimer,
it's the design — and it's why the norms below are written down instead of
tribal:

- **`AGENTS.md` is the contract.** Read order, scope rules, validation bars,
  clean-room protocol. Agents and humans follow the same one.
- **Evidence or it didn't happen.** Commits cite what they verified — a spec
  section, a differential trace, a run count. "Not verified" is an honest
  state we respect. A false "done" is the one sin.
- **Validation bars with teeth.** Deterministic fix: 10 consecutive clean
  runs. Concurrency fix: 20+. One green run proves nothing and we treat it
  that way.
- **Differential testing.** Behavior changes get diffed against something that
  actually runs: `scripts/lane-parity.sh` A/Bs our two recompiler lanes over
  identical ROM by per-swap framebuffer SHA, `c_smoke` link-tests the ABI with
  a real C caller, and the recompiler has a per-instruction oracle suite. Each
  has a documented blind spot, named where it's used — a differential you can't
  run isn't evidence.
- **Types carry the invariants.** One-runnable-game-thread, queue ownership,
  rdram addressing — modeled so misuse fails to compile where possible, and
  fails *loudly* where not.
- **Docs are load-bearing.** Design docs and evidence trails are how the next
  agent inherits context. They get maintained like code because here, they are.

## Clean room

fn64 is written from our own behavioral specs (earned the hard way, in a
debugger), the public libultra documentation, and the recompiler's MIT source.
GPL implementation code is not read, not referenced, not laundered. The full
protocol — allowed sources, disallowed sources, citation rules — lives in
`AGENTS.md` and every design doc states its provenance.

## License

MIT OR Apache-2.0, at your option.
