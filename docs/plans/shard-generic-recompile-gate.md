# Plan: shard `gate_rom_recompile`'s emission so novel ROMs certify

## Context

`gate_rom_recompile` (commit 3bcf3bc) generalizes fn64's whole-ROM CPU
recompilation certification to any ROM discovery accepts. It works up to the
compile step, then stalls: on Clay Fighter 63⅓ it emits **111 MB / 1,886,522
lines of Rust into a single translation unit**, and `rustc -O` had not
finished after 32 minutes (killed, not crashed).

`gate_wm2000_recompile` never hits this because WM2000's emission is split
across 35 shard crates. The dense-AOT pack already computes the shard
geometry that makes this possible — `DENSE_AOT_SHARD_BYTES = 64 KiB`
(`crates/fn64-discover/src/dense_aot_pack.rs:14`) — and the emitter already
has a per-shard entry point,
`emit_dense_bank_shard_runner_function_with_host_calls`
(`crates/fn64-recomp-rs-codegen/src/emit.rs:468`).

So this is not new capability: it is routing the generic gate through the
sharding that already exists, so the certification terminates in practical
wall-clock for any ROM size.

Success = `gate_rom_recompile` on Clay Fighter completes, prints
`rustc_compiles=true harness_runs=true`, reports `unsupported=0`, and exits 0.

## Global Constraints

- **The gate must still fail loud.** Every existing failure path stays: a
  nonzero `unsupported` count, a rustc error, a probe assertion, or a
  materialization mismatch must still exit nonzero. Sharding is a
  compile-strategy change and must not weaken any proof.
- **No shared-library invariant changes.** Do not modify
  `generation_topology.rs`, `dense_aot_pack.rs`, `block_pack.rs`, or
  anything under `crates/fn64-recomp-rs*`. Another agent (Codex) is active
  in this tree; this work is confined to
  `crates/fn64-discover/src/bin/gate_rom_recompile.rs` plus its docs.
- **Byte identity is the oracle.** The emitted code must remain the
  materialized block words; sharding may split *where* code is compiled,
  never *what* words are emitted.
- **Determinism.** Two runs on the same ROM must produce identical
  `runner_sha256`. Shard boundaries must derive from geometry, never from
  iteration order or timestamps.
- **Content-free receipts.** `fn64.rom-recompile-report.v1` must not gain
  ROM bytes or local paths.
- Verify with `cargo nextest run --workspace` (never `-p` — CI parity).

## Task 1: Split emission into bounded compile units

In `crates/fn64-discover/src/bin/gate_rom_recompile.rs`, change
`compile_and_run_harness` so the generated source is split into multiple
`.rs` files compiled separately, rather than one translation unit.

Requirements:

- Introduce a bound on generated source per compile unit. Use a word budget
  derived from the existing 64 KiB shard convention: a unit holds at most
  `DENSE_AOT_SHARD_BYTES / 4` = 16,384 emitted words' worth of runner
  functions. Name the constant and comment why the bound exists (rustc
  compile time is superlinear in single-unit size; 111 MB in one unit did
  not finish in 32 minutes).
- Emit each unit as its own file in the temp directory, compiled with the
  same `rustc --edition=2021 -O --extern fn64_recomp_rs=… -L deps` flags
  already used. Prefer compiling units as separate `--crate-type=rlib`
  artifacts linked by a small driver binary, OR use `mod` includes if that
  proves simpler — the deciding requirement is that no single `rustc`
  invocation sees more than the bound.
- The driver keeps today's behavior: registers every bank, probes each
  bank's first and middle block PC, asserts the typed `UnalignedPc` fault
  on `first.start_va | 1`, prints the same lines.
- `runner_sha256` must stay a digest of the *runner text* (unchanged
  semantics), computed over units in deterministic order.

Verification: build the gate; run it on a small ROM
(`/Users/jer/Code/roms/n64/Penny Racers (USA).z64`, ~94 KB of proven code)
and confirm it still reaches `rustc_compiles=true harness_runs=true`.

## Task 2: Certify Clay Fighter end to end

Run the sharded gate on the novel proof target and record the result.

Requirements:

- `FN64_DISCOVER_ROM="/Users/jer/Code/roms/n64/Clay Fighter 63 1-3 (USA).z64"
  FN64_RECOMPILE_REPORT=<path> gate_rom_recompile` completes and exits 0.
- Capture wall-clock. If it still exceeds ~10 minutes, reduce the per-unit
  bound and re-measure rather than accepting a slow gate.
- Confirm the receipt JSON validates against its own schema shape:
  `unsupported_destinations == 0`, `rustc_compiles == true`,
  `harness_runs == true`, and `emitted_code_bytes` equals
  `pack_words * 4`.
- Run the gate twice; `runner_sha256` must be identical across runs.
- Record the invocation and result table in
  `crates/fn64-discover/reference/corpus-invocations.md`, under a new
  `## gate_rom_recompile` section, matching the style of the existing
  `gate_rom_rebuild` section.

## Task 3: Standardize the compile-unit bound as one shared authority

Task 1 solves the gate. This task makes the *pattern* shared, so a fourth
consumer cannot reinvent it a fourth way.

Current state, verified: the shard **emitter** is shared
(`emit_dense_bank_shard_runner_function*`, used by the codegen crate and two
test suites) and the shard **geometry** is shared
(`DENSE_AOT_SHARD_BYTES`), but **compile-unit splitting is not**: WM2000
expresses it as 35 hand-listed crates in
`examples/wm2000-block-shards/build.rs`, and `gate_rom_recompile` had no
splitting at all. Nothing names the invariant "one rustc invocation must
not exceed N emitted words."

Requirements:

- Add one small public helper to `crates/fn64-discover/src/block_pack.rs`
  (the module that already owns `MaterializedPackedBank` and the runner
  emission both consumers use) that partitions materialized banks into
  compile units under the word bound. Suggested shape — adjust if the
  existing types make another cleaner:
  `pub fn plan_compile_units_v1(banks: &[MaterializedPackedBank], max_words_per_unit: usize) -> Vec<CompileUnitV1>`
  where `CompileUnitV1` names the bank(s)/blocks it covers and its word
  count.
- The bound's *rationale* lives in that doc comment, once: rustc compile
  time is superlinear in single-unit size, measured at 111 MB / 1.89 M
  lines not completing in 32 minutes. Export the default bound as a named
  constant beside it rather than a literal at each call site.
- Partitioning must be deterministic and total: every block lands in
  exactly one unit, units are in stable order, and no unit exceeds the
  bound unless a single indivisible block does (which must be reported,
  not silently oversized).
- Refactor Task 1's implementation in `gate_rom_recompile.rs` to call the
  helper instead of its own inline splitting.
- Unit tests in `block_pack.rs`'s existing test module: total coverage
  (every block appears exactly once), determinism (same input → same
  partition), bound respected, and the oversized-single-block case
  reported rather than hidden.
- Do NOT retrofit `gate_wm2000_recompile` or the WM shard crates in this
  task. Their 35-crate layout carries digest receipts and a scenario gate;
  changing it is a separate, riskier change with its own verification.
  Note the follow-up in the doc comment so the divergence is recorded
  rather than forgotten.

Verification: `cargo nextest run --workspace`; the gate still certifies
Penny Racers and Clay Fighter with byte-identical `runner_sha256` to
Task 2's recorded values.

## Task 4: Document the milestone

Add the result to `docs/DISCOVER-PLAN.md` alongside the existing byte-exact
rebuild milestone.

Requirements:

- State plainly what was certified: whole-ROM CPU recompilation of a ROM
  with no known decompilation or recompilation project, cold, with zero
  unsupported destinations, compiled by a real rustc and probed at
  arbitrary guest PCs.
- State the honest limits with equal prominence: this is a CPU-recompilation
  milestone and **not a booting game** (RSP audio and RDP graphics are
  separate runtime subsystems; the boot harness additionally needs
  host-binding recognizers this gate never consults). Do not imply
  playability.
- Note that the generic gate uses the validated multi-bank composition
  rather than the catalog-bound generation fixed point, and why
  (`NoOverlayGenerations` on single-bank ROMs).
