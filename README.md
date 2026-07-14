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
| `fn64-shell` | The executable: window, input, audio out, ROM intake |
| `fn64-rt64` | FFI bridge to [RT64](https://github.com/fn64/rt64) (MIT, C++) — all C++ interop quarantined here |

Planned: `fn64-recomp`, a Rust-emitting recompiler, once the runtime earns it.

## Status

Pre-alpha, design phase — but not speculative. The proving ground is a live
port effort (WWF WrestleMania 2000 and WWF No Mercy, the AKI wrestling
engine), currently deep in boot bring-up on the reference runtime. Both
runtimes link the *identical* recompiled code, so every fn64 behavior gets
A/B'd against reality before the swap. The boot ladder is the test suite; it
does not grade on a curve.

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
- **Differential testing.** New runtime behavior emits the shared event trace
  (thread switches, queue ops, DMA, task submits) and gets diffed against the
  reference over identical recompiled code.
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
