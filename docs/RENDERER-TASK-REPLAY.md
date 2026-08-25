# Synthetic renderer-task replay

`fn64-render-wgpu` has a test-only structural replay for the ordered raw-DPC
task conveyor. It exists to shorten planning and lifecycle feedback loops; it
is not game evidence and cannot certify a ROM workload.

The recipe is constructed in Rust from synthetic command words and synthetic
texture texels. Nothing is serialized, and no raw game bytes, ROM bytes, or
recompiled output are accepted or written. Each replay creates a fresh
`WgpuBackend` and `RawDpcAbiSession`, then uses the production typed sequence:
plan the ordered task, bind every declared guest read, execute, validate and
commit exact guest effects, seal publication through a fresh `DeviceFabric`,
and publish each member in order.

The default adapterless chain contains a whole-target clear, one `LoadBlock`,
and eleven whole-target fills. Its receipt normalizes process-local authority
identities into per-generation write ranges and content digests, payload and
TMEM projection SHA-256 values, color-target generation and resident-byte
SHA-256 values, and the final logical guest-memory SHA-256. A frozen aggregate
SHA and two-fresh-run equality test guard deterministic reconstruction. Seed
mutation must change both the normalized receipt and final guest postimage.

This chain deliberately does not claim triangle execution coverage: production
triangle task execution requires a successfully created GPU device. The
existing `rdp_harness` triangle tests remain that path's focused oracle. The
structural replay instead gives headless CI an exact end-to-end task authority,
TMEM-load, CPU-fill, effect, and publication check without weakening the
production create contract.

Run the correctness chain with:

```text
cargo test -p fn64-render-wgpu rdp_harness::task_replay::tests \
  --no-default-features --lib
```

The ignored release-only test plans 80,000 synthetic members. It creates a
fresh backend/session authority for every 100-member task and reports wall and
planning time; it does not execute, publish, or silently turn timing into a
correctness threshold:

```text
cargo test --release -p fn64-render-wgpu \
  rdp_harness::task_replay::tests::eighty_thousand_plan_structural_task_replay \
  --no-default-features --lib -- --ignored --nocapture
```

Timing results are diagnostic only. Any proposed optimization still needs the
normal repository correctness gates and a live measurement on the production
workload before it can be called a performance win.
