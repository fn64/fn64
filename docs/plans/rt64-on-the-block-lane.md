# RT64 on the block lane: the ownership objection does not hold

Written 2026-08-07, after the first phase-resolved profile of a *rendering*
route. This is a **finding, not an implementation**. Nothing here was built;
everything here was read.

## Why this was re-opened

The blocker ledger records:

> RT64 does not apply to this lane, correcting an earlier assumption:
> `examples/wm2000-block-boot/Cargo.toml:40-41` depends only on
> `fn64-render`/`fn64-render-reference`. [...] Different contracts, not
> variants -- the reference backend is the correct choice here, not a
> fallback, and the recorded RT64 speedup does not transfer.
>
> -- `docs/plans/wm2000-playable-blocker-ledger.md:272`

That was written before anyone had profiled a route that renders. The profile
now exists, and it changes the stakes: **the software rasterizer is the
single largest line in the whole system.**

| component | % of executor | ms per field |
|---|---:|---:|
| **RSP graphics LLE - raw RDP rasterization** | **31.6%** | **11.00** |
| executor self (guest + runtime + guard) | 37.1% | 12.93 |
| VI present (`vi::scanout` chain) | 11.6% | 4.05 |
| RSP audio LLE (ucode interpretation) | 11.0% | 3.83 |
| graphics LLE other (setup/commit/copies) | 3.6% | 1.26 |
| graphics HLE preflight | 3.6% | 1.24 |
| RSP graphics LLE - RSP interpretation | 1.5% | 0.53 |

And a second fact the profile settled: **WM2000's graphics are not HLE'd.**
`gfx_lle tasks=4900` equals every graphics submit on the route. The display
list goes RSP-LLE -> raw RDP commands -> `dispatch_captured_raw_rdp` ->
`RenderBackend::process_rdp_commands` -> the scalar software rasterizer.

## The objection, and why it fails

The ownership argument originates at
`examples/wm2000-block-boot/src/shell.rs:15`:

> They also differ on RDRAM OWNERSHIP [...] The function lane hands `fn64-abi`
> a pointer to RDRAM the harness keeps [...] The block lane's bootstrap
> transaction VALIDATES an owned allocation and MOVES it into the runtime, so
> nothing outside `fn64-abi` holds the framebuffer bytes afterwards. **A
> windowed block-lane runner must therefore read the VI framebuffer back
> through the runtime** [...] which is a different present path.

Read precisely, that paragraph is about **the present path and the file
layout**. It is correct about both. It has since been generalized into a claim
about *backend applicability*, which it does not support. Three independent
levels refute the general claim.

### 1. Both lanes converge on the same registration call

- Function lane: `boot_thread0` -> `register_process_rdram(rdram, rdram_len)`
  (`crates/fn64-abi/src/host.rs:315`).
- Block lane: `install_owned_process_rdram` -> `host.owned_runtime_rdram =
  Some(storage)` -> `register_process_rdram(pointer, length)`
  (`crates/fn64-abi/src/host.rs:127-145`).

After that call `HostState.runtime_rdram` is the same raw pointer and the same
length in both lanes. The residual difference is **which struct's `Drop` frees
the allocation**, plus page alignment for the `mprotect` barrier. Neither is
observable to a render backend.

### 2. The `RenderBackend` trait expresses no ownership at all

`crates/fn64-render/src/lib.rs:1144`:

```rust
fn process_rdp_commands(
    &mut self, rdram: &mut [u8], start: u32, end: u32, output_addr: u32,
) -> Result<FrameStatus, RenderError>;
```

No `Vec`, no `Box`, no `'static`, no handle — a call-scoped `&mut [u8]`. A
backend that satisfies this against a harness-owned buffer satisfies it
identically against a runtime-owned one, because `fn64-abi` synthesizes the
slice in both cases (`renderer_rdram_slice`,
`crates/fn64-abi/src/task_dispatch/rsp_phase.rs:767`).

### 3. On WM2000's actual path the backend never sees process RDRAM anyway

`dispatch_captured_raw_rdp` hands the backend a **staging copy**, not the
registered allocation (`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1086`):

```rust
let mut image = vec![0u8; staged_end];
image[..physical_len].copy_from_slice(real);
...
backend.process_rdp_commands(&mut image, staging_start as u32, ...)
```

then copies back at `rsp_commit.rs:1131`. So today, in the block lane, with the
reference backend, the renderer already operates on an ABI-local `Vec` that has
no relationship to who owns the process allocation. **Swapping the backend
behind that same `&mut image` changes nothing about ownership.**

## RT64 supports the path WM2000 actually uses

This was the crux and the answer is favorable.
`crates/fn64-render-rt64/src/lib.rs:1285` implements `process_rdp_commands`
against a real native call (`fn64_rt64_process_rdp_commands`,
`crates/fn64-render-rt64/src/ffi/context.rs:386`), with an RDRAM rollback
transaction, and it sets `last_dp_full_sync` — which is what
`require_committed_full_sync_evidence`
(`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1140`) demands. It satisfies
the dispatcher's contract, not merely the signature.

`raw_dpc_batch_capability` returns `Unsupported`
(`crates/fn64-render-rt64/src/lib.rs:1347`) — **and this does not matter.** The
reference backend's own capability is `DiagnosticOnly`
(`crates/fn64-render-reference/src/backend/render_backend.rs:145`), and
`dispatch_captured_raw_rdp` calls `process_rdp_commands` directly. Batching is
a diagnostics seam, not the production route.

## The real blockers, in order

**A. RT64 is simply not wired in.** `examples/wm2000-block-boot/Cargo.toml`
lists only `fn64-render` and `fn64-render-reference`; there is no `rt64`
feature and no backend selector. Compare `examples/wm2000-boot/Cargo.toml:20`
(`rt64 = ["fn64-render-rt64/rt64"]`) and the `FN64_RENDER` selector in
`examples/oot-boot/src/main.rs:591`. **Missing plumbing, not an
incompatibility.**

**B. RT64 needs a GPU and a display server, even "headless."** There is a
hidden-window mode, but it is hidden-*window*, not window-*less*
(`crates/fn64-render-rt64/ffi/fn64_rt64_shim.cpp:1400`, `SDL_WINDOW_HIDDEN |
SDL_WINDOW_METAL`). Hard prerequisites: macOS **main thread**
(`shim.cpp:1380`, an explicit guard that turns a worker-thread embedder into a
recoverable error rather than an Objective-C crash), `SDL_VideoInit`, a Metal
system-default device, and a real `NSWindow`/`CAMetalLayer`. Satisfiable
locally; **not** satisfiable in detached CI, and the `rt64` Cargo feature is
non-default precisely so CI/no-GPU hosts still build.

Whether `wm2000-block-boot`'s benchmark loop runs on the main thread must be
checked before assuming this works. The precedent exists: `examples/wm2000-boot`
already does it.

> **Checked 2026-08-07: it does.** `examples/wm2000-block-boot/src/main.rs`
> contains exactly one `std::thread::spawn`, at `:1036`, and it is an opt-in
> `FN64_BLOCK_WATCHDOG` diagnostic that only prints `entries=`/`last_pc=` every
> five seconds. The execution loop itself is not spawned — it runs on the main
> thread, so RT64's macOS main-thread guard (`shim.cpp:1380`) is satisfied for
> the headless lane without restructuring.
>
> Taken with **C** being "mostly moot" for headless, this narrows the headless
> benchmark path to essentially **A alone** — the Cargo wiring, with a working
> precedent at `examples/wm2000-boot/Cargo.toml:20`. That is the cheapest place
> to get a real number for the 31.6% rasterizer line, and it is a different
> question from shipping RT64 in the windowed shell, where **C** is real work.

**C. Presentation semantics genuinely differ — this is where the original
intuition had real content.** RT64's `present` requires
`PresentMemory::Physical` (`crates/fn64-render-rt64/src/lib.rs:1375`,
"RT64 presentation requires current physical RDRAM authority") and renders into
its own GPU surface rather than writing back to `rdram[output_addr..]`. The
block lane reads the framebuffer back through
`fn64_abi::with_registered_physical_rdram_read`
(`examples/wm2000-block-boot/src/shell.rs:903`) exactly as the shell.rs comment
says. **For the headless benchmark lane this is mostly moot** — there is no
windowed present. For a windowed `wm2000-shell` it is real work.

**D. Behavioral risk, not a blocker.** RT64 requires 8-byte-aligned
`start`/`end` and an in-bounds `output_addr`
(`crates/fn64-render-rt64/src/ingress.rs:53`). `dispatch_captured_raw_rdp`
stages commands *past* `physical_len`, so RT64 receives an 8 MiB+N buffer whose
tail is synthetic. The reference backend does the equivalent
(`render_backend.rs:121`), but RT64's native side should be verified against it.

## What the recorded 12x actually measured

`rt64-throughput-win` records reference 57.31 s vs RT64 4.82 s, ~11.9x, ~56 fps.
That was measured on **`examples/wm2000-boot`, the function lane** — which
carries the `rt64` feature — not on `wm2000-block-boot`. The ledger is right
that the measurement was taken elsewhere. It does not follow that the speedup
cannot transfer: both lanes drive the same `dispatch_captured_raw_rdp` ->
`process_rdp_commands` seam.

## The correct restatement

> RT64 has **not been wired into** the block lane, and the recorded speedup was
> measured on the function lane, so it is **unverified here** — not "RT64
> cannot apply."

`docs/plans/wm2000-playable-blocker-ledger.md:272` should be corrected, and
`examples/wm2000-block-boot/src/shell.rs:15` should scope its ownership
argument to the present path, which is the only place it is load-bearing.

## What it would take to get a number

Add `fn64-render-rt64` and an `rt64` feature to
`examples/wm2000-block-boot/Cargo.toml`, mirror the `FN64_RENDER` selector from
`examples/oot-boot/src/main.rs:591` into the backend registration at
`examples/wm2000-block-boot/src/main.rs:830`, confirm main-thread execution,
and build with `FN64_RT64_DIR` set. **Nothing in the ownership model has to
move.** Roughly thirty lines of plumbing to a measurement, against a component
that is 31.6% of executor time — and that share is a *floor*, because it was
measured on the pre-`5ed7f2c` menu route, which carries 2.42x less graphics
work per step than the gameplay route.

## What this does not claim

That RT64 will be faster here. That is what the measurement is for. This
document only establishes that **the stated reason for not measuring is
wrong**, and that the cost it would attack is the largest one there is.
