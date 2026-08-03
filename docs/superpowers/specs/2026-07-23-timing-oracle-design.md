# Design spec: ares/mupen differential timing harness (0a)

Part of the "fn64 Rust runtime to N64-complete" program
(`~/.claude/plans/breezy-painting-melody.md`). This is the shared acceptance
gate for EVERY timing-refinement item (U2 PI latency, U5 AI drain / EEPROM /
Flash busy, U6 RSP). **Review before implementation.**

## Problem

fn64's device timing is deliberately-honest **deterministic policy**, not
hardware-derived:
- PI DMA: `FixedPiTiming` returns a constant latency independent of length
  (`device.rs:313`).
- AI drain: principled but computed from the DAC rate, not a measured AI model.
- EEPROM/Flash: libultra-compatibility deadlines, not per-chip timing.

The plan's *timing bar* is **reference-emulator parity** (not cycle-exact
silicon — no game observes RCP cycle counts, and no logic analyzer is available).
To move ANY of the "swap the policy for a measured model" items from "policy" to
"reference-parity done," we need a **differential oracle**: run the same ROM +
input on a reference emulator and on fn64, compare cycle-stamped device events.
Without this, every timing refinement is unverifiable and can't be called done.
Hence 0a is first.

## What already exists (build ON, don't rebuild)

- **`tools/mupen-trace/mupen_trace.c`** — a v1 producer driving a DEBUGGER=1
  mupen64plus-core in single-step. **But it emits `executed_pc` records** (a PC
  trace in fn64-discover's trace-schema JSONL), NOT cycle-stamped device events.
  It's a PC tracer, not a timing tracer.
- **Jer's native arm64 mupen dynarec** (PR #1184, `~/Code/mupen64plus-core`) —
  removes the Rosetta blocker the docs cited; a fast native reference core exists.
- **fn64-recomp-rs oracle test scaffolding** — the differential-comparison
  pattern (`cop0.rs`, `interp_differential.rs`, `fpu_oracle.rs`) to mirror.
- **`crates/fn64-discover/src/trace/mod.rs`** — the existing trace schema to extend.

## The build

### Reference choice: mupen (Jer's native core), not ares — with rationale
- **mupen** is already wired (`mupen_trace.c`), Jer owns a native-dynarec build
  (fast), and its DEBUGGER API exposes single-step + register/memory + a callback
  seam we can extend to emit device-register/DMA events. RECOMMENDED as the
  primary oracle.
- **ares** is the more cycle-accurate reference and worth a SECOND opinion for
  contested timings, but it has no headless/trace mode (verified in
  DISCOVER-PLAN.md — GDB-stub only). Keep ares as a manual tie-breaker, not the
  automated gate.

### Component 1 — extend the producer to emit device-timing events
Extend `mupen_trace.c` (or a sibling `mupen_devtrace.c`) to record, per step, a
cycle-stamped stream of **device events**, not just PC:
- MMIO writes/reads to PI/AI/VI/SI register ranges (addr, value, guest cycle).
- DMA start/complete for PI/SI/AI (source/dest/len, start cycle, complete cycle).
- MI interrupt raise/ack (source, cycle).
Emit as a new `TRACE_SCHEMA` device-event JSONL variant in
`fn64-discover/src/trace.rs`. The mupen DEBUGGER `DebugSetCallbacks` /
memory-access hooks give the seam; the guest cycle is `r4300` core count.

### Component 2 — fn64-side event capture
fn64's `DeviceFabric` already IS a cycle-stamped event queue (the U5 assessment's
"load-bearing achievement"). Add a capture tap that emits the SAME device-event
schema from fn64's fabric during a fixed-cycle headless run (reuse the boot
harness's fixed-cycle checkpoint machinery).

### Component 3 — the differential comparator (Rust)
A `tools/timing-diff` (or a `fn64-discover` bin) that:
1. runs a corpus ROM + fixed input on both producers to N cycles,
2. aligns the two device-event streams by (event-type, address, ordinal),
3. reports the first divergence: which device event, expected vs actual
   completion cycle, and the tolerance band.
- **Tolerance**: reference-parity, not bit-exact — define a per-device tolerance
  (e.g. PI completion within X guest cycles of mupen). The tolerance IS the
  acceptance spec; document it per device. Ordering divergences
  (`bytes→PI idle→MI pending→notify`) are ZERO-tolerance (already fn64's
  invariant); cycle-*count* divergences get the band.

## How the timing items consume this
Each Phase-2 item = "replace the deterministic policy behind the existing trait
seam (`PiTimingModel`, AI drain seam, EEPROM/Flash deadline) with a model whose
device-event stream falls within tolerance of mupen's, on the corpus." The
harness is the pass/fail gate. `wrong==0` still holds: a model is admitted only
when its trace matches the reference within the documented band.

## Acceptance gate (for 0a itself)
- The harness runs a corpus ROM (start with OoT — it boots deep) on both
  producers and produces aligned device-event streams.
- It correctly REPORTS a known divergence (inject a deliberately-wrong PI latency
  in fn64 → the harness flags it at the right first-divergent cycle) — proving
  the oracle has signal, not just agreement.
- Deterministic: same ROM+input → same divergence report, 10 runs.

## Scope boundary / non-goals
- NOT cycle-exact silicon timing (explicitly out of scope per the plan).
- NOT a full mupen integration — only the device-event trace seam.
- Does not touch fn64-audio/RSP (Codex) or RT64 (colleague).

## Effort / model
Large; Opus for the comparator + tolerance design; the C producer extension is
mechanical (Sonnet-suitable) once the event schema is fixed. Sequence:
event-schema (fn64-discover/trace.rs) → fn64 fabric tap → mupen producer
extension → comparator → self-test with an injected divergence.

## Open decisions for the user
1. **Primary reference = Jer's native mupen core** (recommended), with ares as a
   manual tie-breaker — OK? (vs investing in an ares headless mode, which the
   docs already assessed as not worth it.)
2. **Per-device tolerance bands** — these are a judgment call (how close is
   "parity"?). Propose starting loose (correctness of ORDERING is the hard gate;
   cycle counts within a generous band) and tightening per-device as data comes
   in. Approve that philosophy, or do you want tighter bands up front?
