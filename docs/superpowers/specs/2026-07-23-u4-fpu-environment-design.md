# Design spec: U4 FPU environment (0b)

Part of the "fn64 Rust runtime to N64-complete" program
(`~/.claude/plans/breezy-painting-melody.md`). This is the single largest
behavior item and the U4 critical path. **Review the recommended approach below
before implementation begins.**

## Problem (verified from code)

fn64's FPU today is a **round-to-nearest fast path**. Arithmetic is emitted as
raw host float ops with no IEEE machinery:

```rust
// crates/fn64-recomp-rs-codegen/src/emit.rs:2139
SqrtS { fd, fs } => line(out, format!("ctx.set_f_s({}, ctx.f_s({}).sqrt());", fd, fs)),
```

`FCSR` is modeled (`runtime.rs:218`: RM(1:0), Flags(6:2), Enables(11:7),
Cause(17:12)), and `round_for_mode` (`runtime.rs:1015`) + `raise_fpu`
(`runtime.rs:1028`, sets Cause/Flags and traps on an enabled exception) exist —
but they are wired ONLY to **conversions and compares**, never to arithmetic.

Consequences (all verified):
- **FCSR.RM ignored on ADD/SUB/MUL/DIV/SQRT** → wrong last bit when RM≠RN.
- **No IEEE flags/cause on arithmetic** — DIV-by-0 sets no Z, ADD/MUL no O/U/I,
  SQRT(−x) no V.
- **No enabled-FP-exception path** — there is no `FloatingPoint`/ExcCode-15
  variant in `CpuException` at all; an enabled exception `panic!`s
  (`runtime.rs:1033`).
- **Host NaN emitted**, not the MIPS canonical `0x7FBF_FFFF`/`0x7FF8…`; no
  SNaN→QNaN quieting outside compares.
- **FR=0 hardcoded**; a double from an odd register `panic!`s
  (`runtime.rs:1149`).
- **No FP conditional moves** (MOVT/MOVF/MOVZ.fmt/MOVN.fmt).
- **The interpreter lane implements NO FPU at all** — every COP1 arithmetic word
  → typed unsupported.

## The core decision: how arithmetic honors FCSR.RM + produces IEEE exceptions

Host `f32::sqrt()` / `+` / `*` always round-to-nearest and give no IEEE
exception flags. To model the VR4300 FPU faithfully we need per-op rounding
control AND per-op exception detection. Three approaches:

### Option A — Soft-float library (`rustc_apfloat`) — RECOMMENDED
Add `rustc_apfloat` (MIT/Apache-2.0, Rust-native, no C, used by rustc itself —
clean provenance, fits fn64's `wrong==0`/MIT discipline). It is IEEE-754-exact:
every op takes a rounding mode and returns a `StatusAnd<T>` carrying the exact
exception flags (invalid/divByZero/overflow/underflow/inexact). Emit arithmetic
as calls into a small `fn64_recomp_rs::fpu` shim that:
1. reads FCSR.RM, maps to the apfloat rounding mode,
2. performs the op via apfloat,
3. feeds the returned status flags through the existing `raise_fpu` (Cause/Flags
   + enabled-exception trap),
4. materializes MIPS canonical-NaN / SNaN-quieting on the result.
- **Pros:** IEEE-exact incl. all edge cases (denormal, NaN payload,
  under/overflow) that host libm *cannot* validate; deterministic across hosts
  (critical for `wrong==0` and cross-platform); the exception flags come for
  free and correct. Interpreter and block lane share the one shim → FPU parity
  for free.
- **Cons:** slower than host float (soft-float is ~10–50× a hardware op); a new
  dependency; per-op call overhead in generated code.
- **Perf mitigation:** the hot audio path already runs through the RSP (its own
  VU, not COP1); game CPU FPU is projection/physics math at a few kHz of ops —
  soft-float cost is invisible there. If a specific title proves FPU-hot, a
  fast-path-when-RM==RN-and-no-enabled-exceptions guard can call host float and
  only fall to soft-float when the mode/enables demand it (keep this as a later
  optimization, not initial scope).

### Option B — Host rounding-mode management (`fesetround`)
Save/restore the host FPU rounding mode around each op via libc `fesetround`,
and detect exceptions via `feclearexcept`/`fetestexcept`.
- **Pros:** near-native speed; no new pure-Rust dep.
- **Cons:** `f32::sqrt()` in Rust does NOT observe the C rounding mode reliably
  (LLVM constant-folds and assumes RN); requires `unsafe` libc + inline-asm
  fences to prevent reordering; **host-dependent** (x86 vs ARM FPU differ on
  denormal/NaN handling) which BREAKS `wrong==0` determinism and cross-platform
  reproducibility; can't produce the MIPS canonical NaN without extra work
  anyway. Rejected primarily because host-dependence is disqualifying for a
  soundness-first runtime.

### Option C — Hand-rolled soft-float
Write our own IEEE core.
- **Cons:** re-implementing apfloat, enormous surface for subtle bugs, exactly
  what a library exists to prevent. Rejected.

**Recommendation: Option A (`rustc_apfloat`).** It is the only one that is both
IEEE-exact and host-independent, which the `wrong==0` discipline requires, and
it gives correct exception flags for free. Perf is a non-issue for CPU-side FPU
and has a clean escape hatch if ever needed.

## Scope of the build (Option A)

1. **`fn64-recomp-rs/src/fpu.rs` (new)**: the shared soft-float shim —
   `add/sub/mul/div/sqrt/abs/neg` for S and D, each taking FCSR.RM, returning
   `(bits, ieee_flags)`; canonical-NaN materialization; SNaN→QNaN quieting;
   denormal-flush per VR4300 (flush-to-zero with the Unimplemented-Operation
   subtlety — the VR4300 traps denormal inputs/results as Unimplemented rather
   than flushing silently; model the documented behavior).
2. **`emit.rs`**: replace the raw-host arithmetic arms (2139, 2158, add/mul/div)
   with calls into the shim; add FP conditional moves (MOVT/MOVF/MOVZ/MOVN.fmt).
3. **`runtime.rs`**: extend `raise_fpu`/FCSR plumbing to consume the shim's flag
   set on arithmetic; add the `FloatingPoint`/ExcCode-15 exception to
   `CpuException` (`execution.rs:175`) with precise EPC/BD; FR=1 register-file
   support (read `Status.FR`; even/odd pairing).
4. **`interp.rs`**: route COP1 arithmetic through the SAME shim → interpreter
   FPU parity (closes the "interpreter has no FPU" gap).

## The oracle (bar: hardware-accurate)

Host libm CANNOT validate this (that's the whole point — it can't produce the
right flags/NaN/denormal). Acceptance oracle, in priority order:
1. **`rustc_apfloat` IS the reference** for IEEE correctness (it's the same core
   rustc trusts) — the shim's own results are correct by construction for
   ordinary IEEE behavior.
2. **MIPS/VR4300-specific behavior** (canonical NaN bit pattern, denormal→
   Unimplemented-Operation trap, FCSR cause/flag exact bits, enabled-exception
   vectoring) needs the **hardware/ares vector set** — extend the existing
   `crates/fn64-recomp-rs/tests/fpu_oracle.rs` with hand-transcribed
   architectural vectors from the VR4300 manual + ares' FPU (`fn64-recomp-rs`
   tests already establish this pattern for CVT/compare).
3. **The live corpus (SM64)** is the integration oracle — SM64 is FPU/collision-
   math-heavy; it running with correct physics is the end-to-end proof. C2
   (SM64 bring-up) co-develops with this.

## Acceptance gate
- Extended `fpu_oracle.rs` passes vs the VR4300/ares vector set (RM modes on
  arithmetic, DIV-by-0→Z, overflow→O, canonical NaN, SNaN quieting, enabled-
  exception→ExcCode-15 with correct EPC/BD, denormal handling, FR=1).
- Block lane and interpreter lane produce identical FPU results
  (extend `interp_differential.rs`).
- SM64 runs with visibly-correct math/physics (no FPU trap, no RM/NaN geometry
  corruption).
- Deterministic: 10 clean consecutive runs.

## Effort / model
Large; Opus. Sequenced: shim + arithmetic + flags first (unblocks the common
case), then enabled-exception vectoring + canonical-NaN/denormal (the edge
correctness), then FR=1 + cond-moves + interpreter parity. Each sub-step is its
own PR with its own oracle vectors.

## Open decision for the user
**Approve adding `rustc_apfloat` as a dependency?** It is MIT/Apache-2.0,
pure-Rust, and used by rustc itself — but it is the first soft-float dep in the
tree. The alternative (host `fesetround`) is rejected above for breaking
host-independence, which `wrong==0` requires. If you'd prefer zero new deps, the
fallback is Option C (hand-rolled), which I do not recommend.
