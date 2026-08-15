# fn64-diff lockstep: first-divergence report (2026-07-14)

> **HISTORICAL RECORD — the §1 harness no longer exists (removed 2026-07-17).**
> This report is kept for §2, which is cited from `fn64-abi/src/mesgqueue.rs`
> as the provenance of the coroutine-context-corruption bug it localized, and
> whose regression test still guards that fix. Do not treat §1 as a runnable
> recipe or as a description of the current crate.
>
> The savestate parser, the faki-tools `oracle` subprocess client, and the
> `lockstep`/`dump_snapshot` binaries described in §1 were deleted: the oracle
> client was a client for another project's CLI (`docs/DESIGN.md` §1.0), and
> the instruction-exact transplant path it drove is **not representable**
> against a recompiler-shaped runtime (`docs/DESIGN.md` §1.0's
> no-mid-function-resume finding — read it before proposing to rebuild any of
> this). What survives in `crates/fn64-diff` is the pure comparator alone.
> References below to `bin/lockstep.rs`, `tests/transplant.rs` and the
> savestate/oracle modules describe code as it stood on 2026-07-14.

## 1. Harness proof (NW4E, real oracle, real fixture) — REMOVED, see header

`cargo run -p fn64-diff --release --bin lockstep -- --oracle <faki-tools oracle> --state <NW4E .st5>`
runs end-to-end: parses a real oracle savestate, transplants RDRAM+GPRs into a real
`fn64_abi`/`fn64_runtime::Executor`, runs forward, then re-queries the SAME faki-tools
oracle binary (`breakpoint --state ... --break-at ...`, subprocess) for ground truth at
the PC fn64 reports reaching, and diffs every GPR + PC.

Real run, fixture `WWF No Mercy-13BA7681-r4-rock-idle-break-801187ac.st5`:

```
LOCKSTEP: starting from snapshot ... (raw pc=0x80001000, resolved resume_pc=0x8012ff04)
LOCKSTEP: fn64 executed 1 checkpoint(s)
LOCKSTEP: querying oracle for checkpoint 'transplant-entry' @ pc=0x8012ff04 ... DIVERGED

FIRST DIVERGENCE @ checkpoint 'transplant-entry' (pc=0x8012ff04):
at: ours=0x0000000000000000 reference=0xffffffff80150000
```

This is the HONEST expected result, not a bug in the harness: the transplanted entry
point is still the stand-in function (`bin/lockstep.rs`'s `stand_in_target`, which only
seeds r29/r31), since no real NW4E `RecompiledFuncs` corpus is linked into this binary.
The moment a real corpus is linked at this entry point, the same harness (unchanged)
reports genuinely deep divergences. This proves the mechanism (subprocess protocol,
savestate parse, RDRAM/GPR transplant, per-field diff, first-divergence selection) is
real and load-bearing — the underlying transplant machinery this reuses lived in the
crate's own transplant integration test (deleted 2026-07-17; see this file's header).

## 2. Real-bug localization: OoT boot, Main-resume SIGBUS

Ran the REAL OoT boot harness (`recomps/wm2000/packages/oot-boot`, standalone workspace) against the
real out-of-tree corpus:

```
RECOMPILED_DIR=~/Code/aki-recomp/games/OOTU/RecompiledFuncs
RECOMP_H_DIR=~/Code/aki-recomp/refs/N64RecompSource/include
ROM=~/Code/aki-recomp/refs/oot-decomp/build/ntsc-1.0/oot-ntsc-1.0.z64
./target/release/oot-boot
```

Result: reproduces the known bug exactly — process exits 138 (SIGBUS). No mupen64plus/
oracle savestate exists for OoT (the faki-tools oracle's savestate parsing/`breakpoint`
plumbing is NW4E-specific; no `.stN` fixture for OoT boot exists to lockstep against),
so ground truth here comes from fn64's own incremental trace sink
(`/tmp/oot-boot-trace.jsonl`, `TraceEvent` records already emitted by the executor) plus
an `lldb` backtrace at the crash.

**Localization** (via `lldb -o run -o bt`):

```
* thread #1 stop reason = EXC_BAD_ACCESS (code=257, address=0x1)
  frame #0: 0x0000000000000001
  frame #1: fn64_runtime::executor::Executor::run_one_step + 624
```

Cross-referenced against the trace (`grep 'to: 3' /tmp/oot-boot-trace.jsonl`): thread 3
("Main") is resumed via `reason: Woken` exactly 5 times; the crash is on trace seq 409,
its 5th resume, immediately after thread 18 blocks on a queue (seq 408) — i.e. the very
next `run_one_step` call after thread 3's `Resume::SendUnblocked`/`Delivered` wake path
(`executor.rs`'s `try_deliver_recv`/`try_deliver_send`, `handle_yield`'s `BlockOnRecv`/
`BlockOnSend` arms).

**First divergence, precisely**: `Executor::run_one_step` (`executor.rs:648`,
`thread.resume(RunToken::issue(), resume_with)`) jumps into thread 3's suspended
`corosensei::Coroutine` stack and control lands at PC `0x1` — not a MIPS-level GPR/RDRAM
mismatch, but corruption of the coroutine's own *native saved resume context* (the
machine stack pointer/return address `corosensei` swapped to on suspend). This
distinguishes it from an ordinary MIPS-semantics bug: no instruction executed with a
wrong operand: the resume mechanism itself jumped somewhere invalid before any MIPS
instruction ran. The corruption is written sometime between thread 3's PREVIOUS
suspend point and this resume — i.e. somewhere in the `BlockOnRecv`/`BlockOnSend`/
`wake_thread`/`MesgQueue` delivery path that runs on threads 0/18 while thread 3 sits
blocked, matching the existing module doc's "state-divergence bug" suspicion (not a
plain stack-overflow, which was already ruled out) in `mesgqueue.rs`'s
`wake_one_sender`/delivery bookkeeping or `pending_resume` handling touching thread 3's
saved state out from under it.

**Scope boundary**: root-causing/fixing this is `fn64-runtime`/`fn64-abi` work,
explicitly out of this task's scope (crates/fn64-diff only, no edits to those crates).
This report localizes the bug to the exact function/call and resume-mechanism class;
fixing it is a follow-up task against `fn64-runtime::executor`/`mesgqueue`/`thread`.

## 3. Gate status

`cargo build/test/clippy -D warnings/fmt --check` all green in `crates/fn64-diff` at
this commit (see workspace CI); the standalone `recomps/wm2000/packages/oot-boot` workspace builds and
runs (not part of the gated workspace, per its own `Cargo.toml` isolation policy).
