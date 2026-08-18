# RT64 port: gap-closure plan

Companion to `docs/RT64-PORT-HONEST-INVENTORY.md`. Measured against
`port/rt64-conveyor` @ `887ba6c2`.

## The real gap is not ported lines

Two facts, both measured:

1. **49 of 50 parity rows** carry `rust_evidence.availability: "unimplemented"`.
   48 of those give the reason *"The Rust renderer delegate does not exist
   yet."*; the 49th (`base::texture-address-filter-lod`) awaits a
   hardware/reference result. The single non-pending row,
   `feature::native-renderer-rdram-sync`, is `RUST_PASS` via fn64's
   **reference CPU rasterizer** — not via any Rust port module.
2. **No `rt64_*` port module is reachable from any conformance runner.**
   The one registered `delegate_kind="rust_port"` policy
   (`reference-native-rdram-sync-v1`, `check_rt64_port_parity.py:258`)
   points at `fn64-render-reference`, a pure-Rust **CPU rasterizer** that
   predates this port — not at the 62 ported modules. The delegate slot for
   the port itself has no runner behind it.

   This naming is a trap worth fixing: `rust_port` currently means "any
   non-RT64 Rust engine", so the ledger's one `RUST_PASS` reads as port
   progress when it is not. Rename to `rust_reference` vs `rust_rt64_port`.

So the port has 62 modules and 2,864 tests, and there is no delegate to run
any of them against RT64. Porting more files does not move the parity ledger,
because nothing consumes the ported code. **The binding constraint is the
delegate, not coverage.**

This is why "192/276 ported" and "1/62 wired" can both be true and why the
first number has been rising without the second moving.

## Ordering principle

Work that closes the delegate gap first; work that raises coverage second.
Concretely: **stop porting until one full parity row goes green through the
Rust delegate.** One green row proves the whole chain (module → delegate →
runner → verifier → ledger); until then every added module is unvalidated
inventory.

## Phase 1 — build the delegate (unblocks 48 rows)

The single highest-value change on this branch.

Good news for scoping: the plumbing already exists. `RunnerPolicy`,
`delegate_kind`, build receipts, and verifier-private authority all work
today — `reference-native-rdram-sync-v1` proves the whole chain end to end.
Phase 1 is registering a *second* policy of the same shape, not inventing
the mechanism.

- **1.1 Add a runner binary backed by port modules.** Mirror
  `fn64-render-conformance-reference-runner.rs`, but have it call into
  `fn64-render-wgpu`'s `rt64_*` modules instead of `fn64-render-reference`.
  New `RunnerPolicy` entry, new `delegate_kind` (`rust_rt64_port`) so the
  ledger can distinguish it from the CPU rasterizer.
- **1.2 Wire one module behind it.** `rt64_gbi_rdp_decode` is the only
  already-production-reachable module — use it. Target row: the
  `admitted_commands_state` observable (19 rows share it, the largest
  single bucket).
- **1.3 Make one row go green.** Produce a real `rust_evidence.availability:
  "qualified"` with runner/verifier/receipt artifacts, the way
  `native-renderer-rdram-sync` already does for the reference rasterizer.

Exit criterion: parity ledger reads **48 pending, not 49**, and the green row
cites a `rust_port` delegate.

## Phase 2 — wire the modules that already exist (raises 1/62)

Only after Phase 1. Ordered by parity leverage — how many ledger rows each
observable unblocks:

| observable | rows | candidate modules |
|---|---:|---|
| `admitted_commands_state` | 19 | `rt64_gbi_*` (7 modules), `rt64_rdp_state` |
| `resource_journal_guest_memory_effects` | 16 | `rt64_framebuffer_*`, `rt64_tmem_*` |
| `vi` | 5 | `rt64_vi_registers`, `rt64_vi_timing` |
| `shader_parameters` | 4 | `rt64_shared_params`, `rt64_*_shaders` |
| `full_sync_timeline` | 3 | `rt64_profiling_timer` |
| `framebuffer_high` | 2 | `rt64_framebuffer_tile` |
| `tmem_bytes` | 1 | `rt64_tmem_hasher`, `rt64_tmem_regions` |

Each module moves from `mod` to `pub mod` **only when a parity row cites it**.
That rule is what prevents the inventory from inflating again.

## Phase 3 — close coverage on files that matter

Not all 78 under-cited files are worth finishing. Rank by whether the file
feeds a pending parity row:

**Worth finishing** (feed `admitted_commands_state` / framebuffer rows):
- `src/render/rt64_texture_cache.cpp` — 1,791 lines, 11.0% cited
- `src/hle/rt64_game_frame.cpp` — 1,042 lines, 0% cited
- `src/gbi/rt64_gbi.cpp` — 561 lines, 1.2% cited
- `src/gbi/rt64_gbi_f3d.cpp` / `_f3dex2.cpp` — 456 lines combined, ~2% cited

**Probably not worth porting at all** — no parity row depends on them, and
fn64 already has equivalents:
- `src/common/rt64_user_configuration.{cpp,h}` — 357 lines, 0% cited
- `src/common/rt64_replacement_database.{cpp,h}` — 563 lines, 0% cited
- `src/preset/rt64_preset_{draw_call,material}.cpp` — 777 lines, 0% cited
- `src/hle/rt64_application.cpp` — 756 lines, 2.2% cited

Recommend moving these to `refused` with an evidenced reason rather than
leaving them credited as `ported`. That is a ~1,700-line **honest reduction**
in the numerator, and it is the correct direction: the inventory should shrink
when the claim was never real.

## Phase 4 — fix the metric so this cannot recur

- **4.1 Add `port_ratio` to the inventory schema.** `port_state: "ported"`
  with `port_ratio: 0.11` is honest; whole-file credit is not.
- **4.2 Gate on coverage, not digests.** `tools/rt64_port_coverage.py --check`
  is in this branch and fails if cited coverage regresses. Add it to the
  verification set alongside the existing checkers.
- **4.3 Report wiring as the headline.** "1/62 wired, 34.4% cited,
  61.8% digest-verified" — in that order. The first number is the one that
  predicts a working renderer.

## What not to do

- **Do not delete `evidence/rt64-port/.../source/`.** It is 83,771 lines of
  duplicated source and it looks like dead weight, but
  `check_rt64_port_parity.py:373` calls `registry.load()` on all 116
  `source_inputs` and retains their bytes; `closure_identity` is computed
  over those hashes. Deleting it turns the gate red. Repoint the receipt
  paths at the live tree instead — same hashes, same closure, no duplication.
- **Do not port more files before Phase 1.** They cannot be validated.
