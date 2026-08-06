# RDRAM write-attribution audit

Read-only audit of every code path that can mutate guest RDRAM, and whether
that mutation is attributed to a `WriterChannel` before the next dispatch
boundary reconciles the watched executable ranges.

Motivating failure class: an UNDECLARED write into a watched executable range
surfaces much later, at an unrelated dispatch boundary, as

```
unjournaled executable mutation changed physical RDRAM [addr, addr+1)
```

naming only an address and no writer. The in-tree suspicion is recorded at
`crates/fn64-abi/src/recompiled/live_program.rs:2047-2052`.

## Executive summary

The audit found **three structurally distinct attribution mechanisms**, not one.
Understanding which one covers a given writer is what decides whether it is a
real hole:

1. **Point declaration** — the writer calls `fn64_recomp_rs::notify_*_write`
   itself, immediately after its bytes commit
   (`crates/fn64-recomp-rs/src/runtime/host.rs:464-512`). Used by the CPU store
   path, PI DMA completion, and the two raw mirrors.
2. **Enclosing-transaction diff** — the writer declares nothing, but runs inside
   `invoke_catalog_block_host`
   (`crates/fn64-abi/src/recompiled/snapshots.rs:883-910`), which opens a host
   ABI transaction before the call and, on flush, snapshots the watched region,
   diffs it, and declares every changed byte as `HostAbi`
   (`crates/fn64-abi/src/recompiled/live_program.rs:1918-1923`). This
   **retroactively covers most raw `*_recomp` shim writes**, including several
   sites the prior partial audit flagged as holes.
3. **Explicit range publication** — the writer accumulates its own effect
   journal and a commit boundary converts it to notifications. Used by RSP DMA
   (`crates/fn64-abi/src/task_dispatch/rsp_phase.rs:66-80`) and the renderer
   (`crates/fn64-abi/src/recompiled/snapshots.rs:1003-1084`).

The remaining true holes are writers that reach RDRAM **outside** all three: the
executor's queue mirror (already fixed by the concurrent agent) and the handful
of paths below that run at device/scheduler boundaries rather than inside a
guest host-call.

**Verification note on mechanism 2:** the sibling API
`snapshot_for_host_shim` / `declare_host_shim_writes`
(`crates/fn64-abi/src/recompiled/live_program.rs:1764`, `:1778`) was written for
exactly this purpose but has **zero non-test callers** — grep for `host_shim`
finds only its own definition and doc text. The coverage that actually exists
comes from `begin_host_abi_transaction`, not from that pair. This is a latent
trap: a future author reading `declare_host_shim_writes` will assume C-shim
writes are covered by it, and they are not.

## Audit table

Risk key: **P0** = live in a normal boot route and can hit a watched executable
range; **P1** = live in a normal route but structurally confined; **P2** = only
reachable in RSP/RDP/audio routes a given run may never exercise; **P3** =
test/example only, or provably cannot reach a watched range.

| # | Site | Mechanism | Declares? | Can hit watched executable range? | Risk |
|---|---|---|---|---|---|
| 1 | `crates/fn64-runtime/src/executor/mod.rs:673` `mirror_queue_to_rdram`, raw write at `:744` | `std::ptr::copy_nonoverlapping` on the registered base pointer | **Now yes** — publishes via `queue_mirror_publisher` (`:734`), wired to `recompiled::write_guest_physical` at `crates/fn64-abi/src/host.rs:100`. Raw fallback at `:741-751` only when the publisher declines (no live journal). | Yes — an `OSMesgQueue` may be linked anywhere in the image; WM2000 has one at guest `0x8009b0b0` | **FIXED** (do not duplicate) |
| 2 | `crates/fn64-abi/src/mesgqueue.rs:148` `osRecvMesg_recomp` delivered-message store | `copy_nonoverlapping` at raw `rdram.add(o)` | **No point declaration.** Covered by mechanism 2: `osRecvMesg` is a registered ABI host shim (`crates/fn64-abi/src/recompiled/runners.rs:1611` `AbiHostShimV1::OsRecvMesg`) invoked through `invoke_catalog_block_host`, and its declared writer effects include `HostAbi` (`runners.rs:1643`). | Yes in principle — `OSMesg*` is a caller-supplied pointer. Covered only while the enclosing transaction exists. | **P1** |
| 3 | `crates/fn64-abi/src/pi/timing.rs:1096` `write_io_mesg_word` (4 call sites at `:1136-1139`, from `osPiStartDma_recomp`) | `copy_nonoverlapping` at raw `rdram.add(address)` | **No point declaration.** Sole caller `osPiStartDma_recomp` is a host shim under the same transaction. Note the *sibling* writes at `:1127`/`:1132` correctly use `RdramPtr::write_u8` — inconsistent, but neither declares. | Yes — `OSIoMesg*` is guest-supplied and can be linked into the image | **P1** |
| 4 | `crates/fn64-abi/src/pi/timing.rs:1064` `osEPiReadIo_recomp` / `osPiReadIo_recomp` 4-byte swizzled store | `copy_nonoverlapping` at raw `rdram.add(dram_addr)` | **No point declaration.** *(Not in the prior partial audit — newly found.)* Same transaction coverage as #3. | Yes — `u32*` destination is guest-supplied | **P1** |
| 5 | `crates/fn64-abi/src/task_dispatch/lifecycle.rs:646` `write_os_task_word` (3 call sites at `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1432-1434`, from `osSpTaskYielded_recomp`) | `copy_nonoverlapping` at raw `rdram.add(base + field)` | **No point declaration.** `osSpTaskYielded` is a registered shim (`runners.rs:1618`) with `HostAbi` in its effects (`runners.rs:1656-1659`). | Yes — `OSTask*` is guest-supplied | **P2** (graphics/audio task routes only) |
| 6 | `crates/fn64-runtime/src/rdram.rs:593` `RdramViewMut::write_u16` | `copy_from_slice` into the lane-mapped storage range | **No** — and neither do its siblings `write_u32` (`:582`), `write_u8` (`:603`), `write_logical_bytes` (`:610`). This type is a *pure lane-mapping* type; attribution is entirely the caller's job. `write_u16` is only distinguished by also lacking the `watch_raw_write` debug hook the other three have (`:583`, `:604`, `:611`) — an instrumentation gap, not an attribution gap. | Depends entirely on caller | **P1** (see caller list below) |
| 7 | `crates/fn64-runtime/src/rdram.rs:512` `RdramPtr::write_u32`, `:553` `write_u16`, `:529` `write_u8` | Raw `*mut u8` arithmetic + `write_unaligned` | **No** — same reasoning as #6. Only `write_u8` carries `watch_raw_write` (`:534`); `write_u32`/`write_u16` have no debug hook either. | Depends entirely on caller | **P1** |
| 8 | `crates/fn64-runtime/src/rdram.rs:757` `Rdram::write_bytes` | direct `self.bytes[..].copy_from_slice(data)`, flat, no lane swizzle | **No.** But: this is the owned-`Rdram` type used by harnesses and tests, **not** the process allocation the recompiled lane runs against (that is a raw pointer registered via `register_process_rdram`). No production caller found. | No production caller | **P3** |
| 9 | `crates/fn64-audio/src/rsp/recomp/runtime/mod.rs:1152` `RspMachine::dma_write` flat indexing `self.rdram[dram + i] = ...` | direct slice indexing | **Yes, transitively.** Every line calls `record_rdram_write` (`:1155`, impl `:1160`), merged into `rdram_writes`, drained by `take_rdram_writes` (`:1187`) and converted to `notify_rsp_execution_or_hle_writeback` by `commit_rsp_rdram_writes` (`crates/fn64-abi/src/task_dispatch/rsp_phase.rs:73`) at both real call sites (`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:183-189`, `:497-504`). | Yes, and it is declared | **P3** (verified covered) |
| 10 | `crates/fn64-audio/src/hle_lle.rs:401` / `crates/fn64-audio/src/hle_rspboot.rs:310` speculative `take_rdram_writes` | same journal, different consumer | **N/A** — these machines run against a **private `rdram_storage` copy**, not the live allocation; effects are returned as `CanonicalRdramPatches` and applied elsewhere. | No — writes never touch live RDRAM | **P3** |
| 11 | `crates/fn64-abi/src/task_dispatch/rsp_phase.rs:467` `apply_verified_audio_rdram_patches` | `RdramViewMut::write_logical_bytes` | `#[cfg(test)]`-gated. Its production-shaped caller `commit_verified_audio_effects` (`:593`, also `#[cfg(test)]`) applies patches to a **shadow copy**, preflights with `preflight_non_executable_host_writes` (`:642`), and publishes under `begin_catalog_nested_writer` (`:654`). | Preflight explicitly rejects executable overlap | **P3** |
| 12 | `crates/fn64-render-reference/src/backend/render_backend.rs:140` (`rdram.copy_from_slice(&image[..rdram.len()])` in `process_rdp_commands`) | whole-buffer overwrite | **No point declaration** — but every fn64-abi entry into the renderer goes through `track_rdp_renderer_mutation` (`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:897`, `:1117`; `task_dispatch/rsp_phase.rs:721`; `task_dispatch/setup.rs:436`), which diffs the watched set and declares `RdpRenderer` (`crates/fn64-abi/src/recompiled/snapshots.rs:1003-1084`). | Yes — a whole-buffer copy covers every watched byte | **P2** (covered) |
| 13 | `crates/fn64-render-reference/src/backend/render_backend.rs:200` (`process_raw_dpc_batch`) | whole-buffer overwrite | Same as #12. Additionally gated: `raw_dpc_batch_capability()` returns `DiagnosticOnly` (`:145-147`), and the fn64-abi publication path traps by name rather than publishing (`crates/fn64-abi/src/task_dispatch/rsp_phase.rs:617-626`). | Yes structurally, but unreachable in a certifying route | **P3** |
| 14 | `crates/fn64-render-reference/src/backend/imp.rs:345` (`rdram.copy_from_slice(&speculative_rdram)` after HLE geometry decode) | whole-buffer overwrite | Same as #12 — reached only under `track_rdp_renderer_mutation`. | Yes | **P2** (covered) |
| 15 | `crates/fn64-recomp-rs/src/runtime/host.rs:605` `Rdram::as_mut_slice()` | hands out `&mut [u8]` over the live allocation | **No** — documented in-tree as "an UNATTRIBUTED write path" (`:589-603`). Sole non-test caller `crates/fn64-abi/src/recompiled/runners.rs:1331` (`call_c`) uses it only to obtain `as_mut_ptr()` for the FR-stable C shim ABI. Those shims are reached from `invoke_catalog_block_host`, so mechanism 2 covers them. | Yes — a C shim writes wherever the guest points it | **P1** |
| 16 | `crates/fn64-abi/src/host.rs:144-145` scheduler running-thread mirror **fallback** | `RdramPtr::write_u32` | **No.** Only reached when `commit_scheduler_running_thread_mirror` returns `false` (no live canonical program) or `recomp-rs` is off. The primary path at `crates/fn64-abi/src/recompiled/execution.rs:697-708` declares correctly. | Yes, but only when there is no journal to violate | **P3** |
| 17 | `crates/fn64-abi/src/pi/mmio.rs:1037` `sync_live_ai_dpc_mmio_into_rdram` | `RdramPtr::write_u32` at `0xA4xx_xxxx` | **No.** Structurally confined to the sparse MMIO window (`RdramAddr::from_gpr(0xA450_000C).offset() == 0x2450_000C`, asserted at `crates/fn64-runtime/src/rdram.rs:865-869`), far above the 8 MiB physical device. | **No** — cannot alias RDRAM | **P3** |
| 18 | `crates/fn64-runtime/src/mmio.rs:505-512` `MmioSpace::sync_into_rdram` | `copy_nonoverlapping` at `RDRAM_MMIO_WINDOW_START + offset` | **No.** Same confinement as #17. | **No** | **P3** (already ruled out) |
| 19 | `crates/fn64-abi/src/pfs.rs` (`:60`, `:275`, `:292-298`, `:318`, `:338`, `:372`, `:390-395`), `crates/fn64-abi/src/gbpak.rs` (`:127`, `:185-191`, `:232`, `:304`, `:341`), `crates/fn64-abi/src/voice.rs` (`:85`, `:171-184`, `:356`), `crates/fn64-abi/src/si/mod.rs` (`:1062`, `:1152`, `:1247`, `:1264`, `:1382-1388`), `crates/fn64-abi/src/pi/mmio.rs:336-351` | `RdramPtr::write_u8/u16/u32` | **No point declaration** for any of them. *(None were in the prior partial audit — newly found, ~25 sites.)* All write guest-supplied out-parameters from inside `*_recomp` shims, so mechanism 2 covers them **iff** invoked through `invoke_catalog_block_host`. | Yes — every destination is a guest pointer | **P1** (controller/pak routes: `si` is live in a normal boot) |
| 20 | `crates/fn64-abi/src/pi/timing.rs:447-458` PI overlay mirror | `RdramPtr::write_u8` per byte | **Yes** — `notify_pi_dma_write(mirror.offset(), completion.len)` at `:459`. | Yes, and declared | **P3** (verified covered) |
| 21 | `crates/fn64-runtime/src/rom.rs:145-162` `impl DmaMemory for ProcessDmaMemory` | `RdramPtr::write_u8` per byte | **Yes** — invokes the `committed_write` callback at `:159`, wired to `notify_committed_dma_write` (`crates/fn64-abi/src/pi/mmio.rs:6-13`, installed at `pi/mmio.rs:901` and `pi/timing.rs:371`) which dispatches to the Pi/Si/Sp notify. | Yes, and declared | **P3** (verified covered) |
| 22 | `crates/fn64-runtime/src/rom.rs:69-77` `impl DmaMemory for Rdram` and `:81-89` `for RdramViewMut` | `dma_write_bytes`, **channel argument discarded** (`_channel`) | **No.** These two impls silently drop the `DmaWriterChannel` they are handed. *(Newly found.)* No production caller — the fabric is only ever handed `ProcessDmaMemory` (`crates/fn64-abi/src/pi/mmio.rs:903`, `crates/fn64-abi/src/pi/timing.rs:373`). | Only if a future caller passes one to the fabric | **P2** (latent trap, not a live bug) |
| 23 | `crates/fn64-abi/src/recompiled/receipts.rs:1009-1010` bootstrap import publication | `RdramViewMut::write_logical_bytes` | **No point declaration**, and none needed: this writes into the importer's own `self.storage` **before the journal is sealed** (`crates/fn64-abi/src/recompiled/live_program.rs:224-231` validates the watched digest at seal time). | Pre-seal only | **P3** |
| 24 | `crates/fn64-recomp-rs/src/runtime/host.rs` CPU store path: `store_backed_word` `:725-731`, `store_h` `:924-934`, `store_b` `:939-948`, `store_d` `:1040-1053` | direct `self.mem[..]` writes | **Yes** — every `self.mem[..]` write in the file is followed by `notify_cpu_instruction_store`/`store16`. `store_wl`/`store_wr`/`store_dl`/`store_dr` delegate to `store_w`/`store_d`. Audited exhaustively: the only `self.mem[` write sites are `:727`, `:930`, `:944`, `:1048`, `:1049`. | Yes, and declared | **P3** (verified covered) |
| 25 | `crates/fn64-abi/src/thread.rs:502` | `copy_nonoverlapping` | `#[cfg(test)]` fixture (`test_stack_writing_entry`) | n/a | **P3** |
| 26 | `crates/fn64-render-rt64/examples/synthetic_fixed_cycle_release.rs:519` | `copy_nonoverlapping` | Example binary | n/a | **P3** |

### The real structural exposure

Items 2, 3, 4, 5, 15, and 19 are all covered by the **same single mechanism**:
the host ABI transaction opened by `invoke_catalog_block_host`. That coverage is
**conditional on the invocation lane**. Two host-call sites use the *unwrapped*
variant `invoke_observed_block_host` instead:

- `crates/fn64-abi/src/recompiled/runners.rs:176` (executable-write resolve-call)
- `crates/fn64-abi/src/recompiled/runners.rs:209` (plain `HostCall`)

Both live in `run_block_program` (`runners.rs:3`), the **non-catalog** block
lane, reached from `crates/fn64-abi/src/recompiled/execution.rs:1608` (the IPL3
bootstrap coroutine) and `crates/fn64-abi/src/recompiled/runners.rs:1164`
(spawned `OSThread` entry when a non-catalog `program` is installed). The
catalog lane (`run_catalog_block_program`, `:854`, and
`run_catalog_block_program_dynamic`, `:708`) correctly uses
`invoke_catalog_block_host` at `:778`, `:835`, `:985`, `:1042`.

So: **every raw-writing `*_recomp` shim in rows 2-5, 15, and 19 is undeclared
when it is invoked from the non-catalog block lane.** That lane does not
generally have a live `mutation_state` (which is why it does not open a
transaction), so today this is mostly benign — but it is the exact shape of the
bug that took eight hypotheses, and it silently becomes live the moment a
canonical program is installed under a `run_block_program` route.

## Fix patterns for the top 3 risks

The reference implementation is the scheduler running-thread mirror,
`crates/fn64-abi/src/recompiled/execution.rs:697-710`:

```rust
let transaction_id = state.borrow_mut().begin_child_transaction();
let transaction = CatalogNestedWriterTransactionV1 {
    live: Some(live),
    transaction_id: Some(transaction_id),
    thread: None,
    operation: "scheduler running-thread mirror",
    committed: false,
};
unsafe { storage.write_u32(origin.global, origin.handle) };
fn64_recomp_rs::notify_host_abi_write(physical_start, 4);
transaction
    .commit_with(|physical| unsafe { storage.read_u8(RdramAddr::from_offset(physical)) });
```

Three invariants make it correct, and all three matter:

1. The child transaction is opened **before** the bytes become visible, so the
   journal brackets the mutation rather than discovering it.
2. The `notify_*` covers **exactly** the bytes written, as one contiguous span.
   Splitting one logical update into several declarations forces each to be
   committed before the next ordering boundary — see the 12-byte single-span
   rationale at `crates/fn64-runtime/src/executor/mod.rs:709-718`.
3. `commit_with` runs before returning. The `Drop` impl
   (`execution.rs:712-729`) **poisons** the mutation state if the transaction
   unwinds uncommitted, so a panicking writer cannot leave a silent hole.

Note that `crates/fn64-abi/src/recompiled/host_memory.rs:64-114`
(`write_guest_physical`) already packages exactly this pattern behind a
byte-slice API, including the no-live-program fallback. **Prefer calling it over
hand-rolling the transaction.** It is what the queue-mirror fix uses.

### Risk 1 — `write_io_mesg_word` (row 3) and `osEPiReadIo_recomp` (row 4)

`crates/fn64-abi/src/pi/timing.rs:1096-1104` and `:1058-1065`.

These are P1 because `osPiStartDma`/`osEPiReadIo` are ordinary boot-route calls
and both destinations are guest pointers into the image. They are also the
easiest to fix, because the same function already holds an `RdramPtr` for its
sibling writes (`:1125-1135`) — the raw `copy_nonoverlapping` is gratuitous.

Replace the body of `write_io_mesg_word` with a declared write. The four fields
at `mb+0x4 .. mb+0x14` are contiguous, so batch them into **one** 16-byte
declaration rather than four:

```rust
// in osPiStartDma_recomp, replacing the four write_io_mesg_word calls
let mut fields = [0u8; 16];
fields[0..4].copy_from_slice(&queue.to_be_bytes());
fields[4..8].copy_from_slice(&dram_addr.to_be_bytes());
fields[8..12].copy_from_slice(&dev_addr.to_be_bytes());
fields[12..16].copy_from_slice(&nbytes.to_be_bytes());
let start = mb.checked_add(0x4).expect("OSIoMesg field overflow").offset();
if !crate::recompiled::write_guest_physical(start, &fields) {
    // no live journal: fall back to the existing raw stores
    unsafe {
        write_io_mesg_word(rdram, mb, 0x4, queue);
        /* ... */
    }
}
```

`write_guest_physical` takes **guest-order** bytes (it writes per-byte through
`RdramPtr::write_u8`, which applies the `^3` lane XOR), so `to_be_bytes` is
correct here — the same reasoning as
`crates/fn64-runtime/src/executor/mod.rs:720-724`. This is the single most
important detail to get right; a `to_ne_bytes` here would byte-swap each field.

For `osEPiReadIo_recomp` at `:1058-1065`, the existing code already computes the
swizzle `[buf[3], buf[2], buf[1], buf[0]]` explicitly for a flat store. Feeding
the **unswizzled** `buf` to `write_guest_physical` produces identical storage
bytes and declares the write, deleting the hand-rolled swizzle at the same time.

### Risk 2 — `osRecvMesg_recomp` delivered-message store (row 2)

`crates/fn64-abi/src/mesgqueue.rs:141-151`.

P1 for the same reason, plus `osRecvMesg` declares `all_live_channels()` as its
writer effects (`crates/fn64-abi/src/recompiled/runners.rs:1643`) — the widest
claim any shim makes — which means the journal already expects arbitrary
mutation from it and will not help localize a fault.

```rust
if let Some(msg) = delivered {
    if !msg_out_is_null {
        let bytes = (msg as u32).to_be_bytes();
        if !crate::recompiled::write_guest_physical(msg_out_addr.offset(), &bytes) {
            let o = msg_out_addr.offset() as usize;
            unsafe {
                std::ptr::copy_nonoverlapping((msg as i32).to_ne_bytes().as_ptr(), rdram.add(o), 4);
            }
        }
    }
}
```

Keep the `msg_out_is_null` guard on the **raw register** (`ctx.r5 == 0`), not on
the translated `RdramAddr` — the correction documented at
`crates/fn64-abi/src/mesgqueue.rs:112-127` is load-bearing and easy to
re-break while editing this block.

### Risk 3 — the `run_block_program` unwrapped host-call lane

`crates/fn64-abi/src/recompiled/runners.rs:176` and `:209`.

This is the structural one. Fixing rows 2-4 individually still leaves rows 5,
15, and 19 (~30 sites) depending on an enclosing transaction that this lane does
not open.

The narrow fix is to make the two sites use the wrapped variant when a canonical
program is live:

```rust
match with_host(|host| host.canonical_recompiled_program.clone()) {
    Some(live) => invoke_catalog_block_host(&live, vram, resume, host, ctx, mem),
    None => invoke_observed_block_host(vram, resume, host, ctx, mem),
}
```

`begin_host_abi_transaction` (`crates/fn64-abi/src/recompiled/live_program.rs:1876`)
already returns `None` when there is no `mutation_state`, and
`finish_host_abi_transaction` (`:1936`) no-ops on `None`, so the wrapped form is
safe even without a journal. The `Some`/`None` split above is only needed
because `invoke_catalog_block_host` takes `&CanonicalLiveBlockProgramV1` by
reference.

**Cost caveat:** the wrapped form snapshots the watched region (1 MiB on WM2000)
at every host call. That cost is already documented as significant —
`live_program.rs:1818-1824` puts `reconcile_before_dispatch` at 1055 of 2627
profile samples, and `:2040-2044` notes the snapshot+SHA pair was ~42% of the
shell profile. Extending it to a second lane is not free and should be measured
before merging.

## Structural fix: one instrumented seam

**The single change that would make this bug class impossible:** make it
*impossible to obtain write access to guest RDRAM without naming a
`WriterChannel`*, by removing every escape hatch that hands out a bare `*mut u8`
or `&mut [u8]` over the live allocation.

Concretely:

1. **Give `RdramPtr` and `RdramViewMut` a channel.** Today they are pure
   lane-mapping types (`crates/fn64-runtime/src/rdram.rs:445-640`) and every
   write method is silent. Make construction require a channel token —
   `RdramPtr::from_storage_ptr(ptr, WriterChannel::HostAbi)` — and have each
   `write_*` method emit the declaration itself. This alone converts rows 3, 4,
   7, 16, 19, and 22 from "depends on an enclosing transaction" to
   "declared at the point of write", and it is a **type error** to add a new
   undeclared writer.
2. **Delete `Rdram::as_mut_slice`** (`crates/fn64-recomp-rs/src/runtime/host.rs:605`).
   Its own doc comment already calls it "an UNATTRIBUTED write path" and names
   its single legitimate caller. Replace that caller
   (`crates/fn64-abi/src/recompiled/runners.rs:1331`) with a
   `as_mut_ptr_for_c_shim(channel)` accessor that returns the pointer *and*
   opens a scoped transaction whose `Drop` commits the diff — i.e. wire up the
   already-written-but-never-called
   `snapshot_for_host_shim`/`declare_host_shim_writes` pair.
3. **Make the `DmaMemory` channel non-discardable.** Rows 22's two impls take
   `_channel` and drop it. Either delete those impls (no production caller
   exists) or make the trait method return a receipt the caller must consume.
4. **Delete the two unwrapped `invoke_observed_block_host` call sites** so there
   is exactly one host-call seam.

**What it would cost.**

- *Churn:* ~60 call sites across `fn64-abi` (`pfs`, `gbpak`, `voice`, `si`,
  `pi`, `task_dispatch`, `vi`, `mesgqueue`) plus `fn64-runtime` and
  `fn64-recomp-rs`. Nearly all are mechanical — thread a channel constant
  through construction. The `#[cfg(test)]` and example call sites
  (`fn64-certification` has ~20, `fn64-render-reference` tests ~30) need a
  `WriterChannel::BootstrapOrImport` or a test-only unattributed constructor,
  or the churn triples.
- *Runtime:* the point-declaration form is **cheaper** than what it replaces,
  not more expensive. `notify_attributed_guest_write`
  (`crates/fn64-recomp-rs/src/runtime/host.rs:464-479`) is a page-mark plus two
  thread-local observer calls, and it early-returns on `len == 0`. Today's
  coverage comes from diffing a 1 MiB watched region per host call; replacing
  that with per-write declarations should *remove* the dominant profile cost
  identified at `live_program.rs:1818-1824`. This is the strongest argument for
  doing it: the structural fix and the performance fix are the same change.
- *Risk:* the lane-mapping subtleties are the danger. Guest-order vs
  native-order (`crates/fn64-runtime/src/rdram.rs:10-38`), the `^2`/`^3` XOR,
  and the physical-vs-logical offset distinction are each easy to get wrong
  while mechanically threading a parameter, and a mistake produces *plausible*
  bytes rather than a crash. The differential test at
  `crates/fn64-runtime/src/rdram.rs:998-1025`
  (`word_wise_copy_matches_the_per_byte_reader_at_every_alignment`) is the right
  model: any such refactor should be landed behind an equivalent
  every-offset/every-length differential assertion.

### Cheaper interim mitigation

Short of the full refactor, the highest value-per-line change is to extend
`watch_raw_write` (`crates/fn64-runtime/src/rdram.rs:464-489`) to the three
methods that lack it — `RdramViewMut::write_u16` (`:593`),
`RdramPtr::write_u32` (`:512`), `RdramPtr::write_u16` (`:553`). The comment at
`:531-534` records that instrumenting only `RdramViewMut` is precisely what made
the `0x0009b0b3` write produce no backtrace and cost several of the eight failed
hypotheses. Three one-line additions close that diagnostic gap for the next
occurrence, independently of whether the attribution is ever fixed.
