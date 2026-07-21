# fn64 design

Status: pre-alpha, design phase. This document is the load-bearing spec
`AGENTS.md` requires agents read before touching code. Every claim below
cites its source per the clean-room protocol: our own boot-ladder evidence
(`aki-recomp/docs/BOOT-LADDER-PLAYBOOK.md`, `aki-recomp/games/NWXE/profile.toml`
rung comments), the mechanically-extracted ABI surface
(`aki-recomp/runtime/ABI-SURFACE.md` / `abi_surface.json`), and the public
libultra manual. No GPL runtime implementation code was read to write this.

## 1. Crate layout

```
fn64-runtime   core: scheduler, OSMesgQueue, timers, PI/SI/VI/AI plumbing, rdram model, overlays
fn64-abi       the extern "C" surface recompiled code links against
fn64-boot-harness shared generated-section bridge/registration and ABI-sized rdram allocation
fn64-shell     the executable: window, input, audio out, ROM/RecompiledFuncs intake
fn64-render    backend-agnostic render seam + the pure-Rust ReferenceBackend (A/B oracle)
fn64-render-rt64 FFI bridge to RT64 (C++) -- all C++ interop quarantined here
fn64-recomp / fn64-recomp-rs  the Rust-emitting recompiler and its whole-ROM driver (§1.1's `rs` lane)
fn64-audio     RSP audio ucode execution
fn64-diff      the first-divergence comparator, pure/no-I/O (§4's comparator lane)
fn64-discover  ROM discovery: symbol/section metadata without a decomp (Phase D)
```

(`fn64-rt64` is this doc's older name for `fn64-render-rt64`; the crate is
`fn64-render-rt64` and §1's later prose still uses the short form.)

Dependency direction is strictly one-way:

```
fn64-shell ──depends on──> fn64-abi ──depends on──> fn64-runtime
    │                                                    ^
    └──────────────────depends on───────────────────────┘
    └──depends on──> fn64-boot-harness ──depends on──> fn64-abi + fn64-runtime
    └──depends on──> fn64-rt64 ──depends on──> fn64-runtime (types only)
```

`fn64-runtime` depends on nothing else in this workspace. It is pure, safe
Rust: the scheduler, message-queue semantics, timer wheel, rdram buffer
ownership, and the diagnostic/watch hooks. It has no knowledge that it is
being called from generated C, and no knowledge of RT64. This is what makes
it independently testable (unit tests drive scheduler/queue invariants with
no ABI or graphics involved) and is also the reuse seam: any future
recompiler backend (not just N64Recomp's C) links against the same core.

`fn64-abi` depends on `fn64-runtime` only. It is the thin, mechanically-
checkable translation layer: every `#[no_mangle] extern "C"` symbol
generated `RecompiledFuncs/*.c` calls (`recomp.h` dispatch helpers and the
`_recomp` shim inventory) lives here,
each one a direct call into an `fn64-runtime` API. This crate is deliberately
"dumb" -- if a function's `fn64-runtime` counterpart already exists, its
`fn64-abi` wrapper is a signature-and-marshalling adapter, not a place new
policy gets invented. Reviewing `fn64-abi` in isolation should answer "does
this match ABI-SURFACE.md" without needing runtime-internals knowledge.

`fn64-boot-harness` depends on `fn64-abi` and `fn64-runtime`. It owns the
game-agnostic generated-C boot boundary shared by `fn64-shell` and the
headless boot examples: the clean-room `recomp_overlays.inl` bridge, its
`fn64_register_func` callback and section accumulator, registration-order
adapter, generated `recomp_entrypoint` declaration, and the one RDRAM
allocation sized for physical RDRAM plus the raw MMIO/KSEG1 window. Which
sections begin resident and all input/save/render/audio policy remain local
to each harness because those choices differ by game and host.
The allocation API requires a typed `TvType` (`Pal`/`Ntsc`/`Mpal`) and writes
the IPL-owned `osTvType` boot global before thread 0 runs. A zero-filled buffer
is not valid console boot state: generated initialization reads this global to
choose region-dependent VI/audio parameters before any ABI shim can repair it.

`fn64-shell` depends on `fn64-abi`, `fn64-runtime`, and `fn64-rt64`. It owns
the parts every recompiled game needs but that aren't part of the libultra
ABI surface itself: windowing, input device polling, audio output backend,
loading a user's own locally-recompiled ROM output (per `README.md`'s "no
game content in this repo" rule -- the shell is where a user's own build
artifacts get linked/loaded, never anything checked into fn64).

The shell's audio backend keeps two clocks and two queue views explicit. AI
DMA buffers arrive at the true DAC rate returned by `osAiSetFrequency`; cpal
may run at a different device rate and resamples at that boundary. `AI_LEN`
reports only the current emulated DMA. The host output prebuffer is separate,
starts after two AI DMAs are queued, and exists only to absorb callback jitter;
letting its depth leak into `AI_LEN` would make guest buffer sizing depend on
host latency rather than N64 hardware state.

`fn64-rt64` depends on `fn64-runtime` (for the shared types the gfx task
handoff needs to name -- e.g. an rdram-relative address newtype, task
buffers) but is the ONLY crate in the workspace permitted to contain C++ or
call into RT64's C++ API. Rationale, three reasons:

1. **License and language boundary are the same boundary.** RT64 is MIT but
   C++; keeping all `cxx`/`bindgen`/raw-FFI surface in one crate means a
   `cargo geiger`-style or manual audit of "where is this workspace not
   memory-safe Rust" has exactly one crate to look at, not a foreign-function
   call site sprinkled through the runtime.
2. **The gfx task handoff is explicitly an open question, not a settled
   contract** (`ABI-SURFACE.md` section (e): "the gfx task handoff signature
   that RT64 consumes is NOT visible from generated RecompiledFuncs C in this
   snapshot for either game -- no direct osSpTaskLoad/osSpTaskStartGo
   `_recomp` call site found... this is a real gap, not a resolved ABI
   point"). Quarantining the unresolved seam in its own crate means the
   uncertainty doesn't leak into `fn64-runtime`'s otherwise well-specified
   scheduler/queue model; when the real call shape is observed (a profile.toml
   rename reaching that call site), only `fn64-rt64` and the `fn64-abi` glue
   need to change.
3. **Independent buildability.** A contributor working on scheduler
   correctness should never need a C++ toolchain or RT64 checked out. Only
   building `fn64-shell` (which needs real graphics output) pulls in
   `fn64-rt64`; `cargo test -p fn64-runtime -p fn64-abi` stays pure-Rust and
   fast in CI.

#### License boundary: what the RT64 wrap is allowed to link (LIVE CONSTRAINT)

fn64's linked binary stays MIT/Apache-clean **only because the RT64 build is
scoped to its render/HLE path.** This is a standing build constraint, not a
one-time finding — re-check it whenever the RT64 pin moves or the wrapper's
CMake scope changes.

**The rule: build RT64 as a static lib for its render/HLE target only, and do
NOT enable the mupen64plus plugin target.** `mupen64plus-core` is **GPLv2**
(source: `third_party/rt64/src/contrib/mupen64plus-core/LICENSES`, "licensed
under the GNU General Public License version 2"). It is not linked today:
RT64 puts only `.../mupen64plus-core/src/api` on the *include path*
(`third_party/rt64/CMakeLists.txt:421`) to consume the mupen plugin ABI
headers (`m64p_*` descriptor types), and no RT64 source outside `contrib/`
`#include`s an `m64p` header in the evaluated tree. That include exists for
RT64's own future emulator-plugin build — a feature its `README.md:6` says is
"not available in this repository yet." Enabling that target is what would
pull GPL into fn64's binary.

Everything else RT64 links is permissive, audited 2026-07-16 against the
`no-mercy-recompiled/third_party/rt64` @ `f0728a2` checkout: RT64 itself MIT
(`third_party/rt64/LICENSE`); `plume` GPU abstraction + `re-spirv` MIT
(`src/contrib/plume/LICENSE`, `src/contrib/re-spirv/LICENSE`);
`imgui`/`implot`/`im3d`/`hlslpp`/`VulkanMemoryAllocator`/`stb`/`ddspp` MIT or
public-domain; `xxHash`/`zstd` BSD; `nativefiledialog-extended` Zlib;
`spirv-cross` Apache-2.0; `dxc` LLVM/Apache-with-exception and build-time only
(a shader compiler binary, not linked into the runtime —
`third_party/rt64/CMakeLists.txt:39-61`).

The same one-crate quarantine that bounds the unsafe audit bounds this license
audit: there is exactly one crate to check.

**Provenance note (clean-room):** RT64 is HLE "directly reverse engineered by
observing console behavior" (its `README.md`) — it studied *hardware output*,
not copyrighted game code. That is different in kind from the matching
decompilations this project rejects, and consuming it as an MIT dependency does
not touch fn64's own from-ROM-bytes provenance for game code. `raster.rs:17-24`
already cites RT64's MIT `shared/rt64_color_combiner.h` as its algorithm
source; reading MIT RT64 is an allowed source under AGENTS.md, GPL runtime
internals are not.

No longer planned-only: `fn64-recomp`/`fn64-recomp-rs`, the Rust-emitting
recompiler `README.md` deferred until the runtime earned it, are built and
boot OoT. They add the second lane below.

### 1.0 The outer boundary: fn64 owns its toolchain

The rules above govern crate-to-crate concerns. They say nothing about the
boundary between fn64 and everything outside it, and that omission has a
scar: a legacy sibling checkout (`aki-recomp`) became load-bearing without
violating a single written rule — it is not a crate, so dependency direction
never caught it; it is not C++, so the quarantine never caught it. It was
found 2026-07-17 and is being cut (ROADMAP Phase H). The rule that would have
prevented it:

**Everything needed to build and run fn64 lives in fn64, except a user's own
game content.** Exactly one class of input is legitimately out-of-tree — ROMs
and anything ROM-derived, which the no-game-content rule bars from git
forever. Everything else — recompiler configs, upstream MIT headers, tooling,
metadata — is either owned here, vendored here, or generated here.

Corollaries, each earned the hard way:

- **A path to another project is not a dependency mechanism.** If fn64 needs
  an artifact, vendor it, submodule it, or generate it. Reaching into a
  sibling working directory couples fn64 to one machine's layout and, worse,
  to another project's lifetime.
- **Out-of-tree inputs are named and declared, never defaulted to someone's
  home directory.** A default path that only resolves on the author's machine
  is a silent shrug: it works for exactly one person and fails or — worse —
  silently reads something stale for everyone else. (`native-emit.sh` did
  exactly this: it hashed a stale driver into a cache key when the repo-local
  and `CARGO_TARGET_DIR` copies diverged.)
- **Test/gate fixtures obey this too.** A gate whose inputs are compile-time
  `const` paths into a personal directory (`fn64-discover`'s `gate_*.rs`,
  ROADMAP H3) produces numbers exactly one person can reproduce. Unreproducible
  evidence is not evidence — see AGENTS.md's validation bars.

#### Instruction-exact savestate transplant is NOT REPRESENTABLE here (negative result, 2026-07-14)

This is an architecture fact about the runtime's shape, kept here because the
code that discovered it has been deleted (see below) and a future session must
not re-derive it the expensive way — or, worse, re-add a mupen64plus savestate
parser believing it closes a gap. It does not. There is no gap; there is a
representability wall.

fn64 (like N64Recomp itself) compiles each MIPS function to one native
function. `SectionRegistry::resolve` (`fn64-runtime/src/overlay.rs`) matches
only a vram address that is an EXACT function-entry offset, **by design**:
`LOOKUP_FUNC`'s only real call shape is a whole-function indirect call.
A savestate's saved PC lands wherever an instruction happened to be
executing — essentially never exactly at a function's first instruction.

Therefore true instruction-exact transplant ("resume at PC") is **not merely
unimplemented — it is not representable by a recompiler-shaped runtime at
all**, without either:

- (a) sub-function-granularity call targets, which N64Recomp's own codegen
  does not produce; or
- (b) a bytecode/threaded-interpreter fallback for the remainder of the
  interrupted function.

The deleted code was honest about this rather than faking it: its
`resolve_entry_point` reported the ENCLOSING function (nearest registered
function whose vram range contains the resume PC) plus the offset into it,
rather than silently pretending an exact resume had happened. Starting the
enclosing function from its own top is a materially different — and for that
invocation, near-certainly incorrect — execution.

Consequence for the comparator lane (§4): the unit of comparison against a
reference runtime can only be a **checkpoint PC reached by whole-function
execution**, never a single MIPS instruction. `fn64-diff` is scoped to exactly
that comparison and nothing more.

This is a statement about the current function-granularity lane, not a permanent
limit on fn64. `UNIVERSAL-RUNTIME-PLAN.md` defines the bank-qualified
arbitrary-PC block lane that removes the representability wall. Until its U1
gate passes, savestate resume remains unrepresentable and no tool may claim
otherwise. The working-tree sparse emitter now compiles a real digest-verified
N64 bank without decoding holes. `BlockProgram` atomically registers the owned
`CodeBank` with the generated callable and rechecks a sparse entry before
invocation; emitted code supplies the bank-bound registration helper. The live
executor now has an explicit `boot_thread0_block_program` lane that owns the
registered program for thread 0 and spawned OSThreads. Generated instruction
checkpoints suspend to the executor, which charges their instruction count to
virtual time and services device deadlines before another block can run. The
real discovered pack is not installed by a boot target yet. Arbitrary-PC
codegen emits direct boundaries for a supplied static host-JAL inventory and a
distinct `ResolveCall` for dynamic JAL/JALR targets. The live resolver types
those as either an installed host function or a bank-qualified guest target;
generated `jr`/`jalr` recognizes only an explicit OSThread return sentinel.
`ExecutableRegion` now owns one active immutable generation and atomically
retires the previous `CodeBank` plus runner on same-range replacement. The ABI
registers equal-length physical/virtual executable spans and observes typed CPU
stores, generated-C direct RDRAM stores, and device DMA writes through one
post-commit range seam. At the next host boundary it snapshots architectural
byte order, builds and publishes the replacement pair, retires the old pair,
and re-resolves interrupt, checkpoint, host-resume, and spawned-thread entries
through the active mapping before executing. A generated runner can still
execute later instructions from its current immutable body after a store and
before that runner returns; store-interior checkpoints remain required for the
strict per-instruction invalidation rule. No real pack supplies a dynamic
builder or boot registration yet, so this does not make real-ROM
transplant/resume available by itself.

Provenance of the removal: `crates/fn64-diff` once carried a subprocess client
for the *faki-tools* `oracle` CLI plus a mupen64plus `.m64p` savestate parser,
to drive this transplant path. Both were removed 2026-07-17 — the oracle client
because a client for another project's command line is precisely what §1.0
above forbids, and the savestate parser because the path it fed cannot work,
for the reason stated here. The historical run they produced is preserved in
`crates/fn64-diff/docs/2026-07-14-first-divergence-report.md`.

### 1.1 The two lanes: how the game arrives, and what draws it

Two independent switches select a build configuration. They are orthogonal,
and a symptom is only diagnosable once you know which lane produced it — the
same visual artifact means different things in each.

**Recomp lane — `FN64_RECOMP=c|rs`.** Which *form the game arrives in*:

- `c` (default): N64Recomp's emitted `RecompiledFuncs/*.c`, compiled and
  linked as before.
- `rs`: `fn64-recomp-rs` emits the whole ROM as a typed-Rust crate
  (`recompile_rom`), linked directly.

Both lanes are the same recompiled semantics in two forms, so they A/B each
other. This is a *different axis* from §4's A/B, which swaps which **runtime**
implements the `_recomp` surface under an identical game; this swaps the
**game's form** under an identical runtime. §4's `nm`-based completeness gate
applies to the `c` lane's archive; the `rs` lane resolves the same surface
through Rust linkage instead.

The emitted crate is out-of-tree, game-derived material, and per `AGENTS.md`
it must never enter git or the main workspace graph. That constraint — not
taste — is why the rs lane builds through **standalone manifests carrying
their own `[workspace]`** (`examples/oot-boot/rs/Cargo.toml`,
`crates/fn64-shell/rs/Cargo.toml`) that reuse the sibling `src/main.rs` and
`build.rs` rather than duplicating them. A gitignored `recompiled` symlink is
refreshed from `RECOMP_RS_DIR` before Cargo resolves the graph. A standalone
manifest is the seam that keeps game-derived code out of the workspace; if
you find yourself adding the emitted crate as a normal path dependency, that
is the rule you are about to break.

**Render backend — `FN64_RENDER=reference|rt64`** (feature `rt64`):
`ReferenceBackend` is the pure-Rust, headless CI/seam-test backend and A/B
oracle; RT64 (§1's `fn64-render-rt64`) is the faithful lane. **Keep both — the
oracle is not obsolete once RT64 works.** ReferenceBackend is what keeps
`cargo test` GPU-free (RT64 needs Vulkan/D3D12/Metal, so headless CI would
otherwise need a GPU or software Vulkan), and it is the differential oracle the
wgpu port is gated against (ROADMAP P2).

An `OSTask` boundary is an RSP scheduling boundary, not an RDP reset. The
reference backend therefore owns one persistent RDP decode state shared by its
admitted F3DEX2 and raw-DPC entry paths: other mode, combiner/constants,
scissor/fill, texture-image/tile/TLUT registers, and physical TMEM survive.
Only the F3DEX2-owned `G_TEXTURE` enable/tile/scale selection is rebuilt per
task. Keeping this ownership in the backend prevents a new decoder invocation
from manufacturing reset registers or a white texture. `G_SETCIMG` is part of
that persistent RDP state: production F3DEX2/raw color writes require it and
never infer it from the independent VI scanout/`output_addr` state. Persistence
retains the register, not a private copy of memory: the selected image is
re-imported from RDRAM at each task boundary to observe intervening CPU/device
writes.

Operational detail (emit caching, the shared target dir, the `./oot` loop)
lives in `FAST-LOOP.md`; this section is only the shape and the why.

## 2. Threading model

### The invariant this model exists to enforce

**Exactly one game thread runs at a time.** This is not an optimization
choice, it is dictated by the ABI: `recomp_context` (per
`ABI-SURFACE.md` section (b), from `recomp.h`, MIT) is a plain mutable
struct of MIPS register state with no synchronization of its own, and
every `RECOMP_FUNC` receives `uint8_t* rdram` -- one shared, unsynchronized
byte buffer -- as raw pointer, not behind any lock. Real N64 hardware ran
one CPU; the recompiled C was generated assuming exactly that. A host
implementation that lets two "logical" OSThreads' recompiled C actually
execute concurrently on two host threads is not parallelizing a
parallel-safe program -- it is inventing a race the original program never
had and the ABI gives no tools to guard against.

### The evidence this is a real, not theoretical, failure mode

`aki-recomp/games/NWXE/profile.toml`'s rung 18 / 18b writeup (boot-ladder,
2026-07-14) is the definitive case study, cited here as our own evidence
(not GPL code -- we read our own debugger output, not vendor source):

- The crash: `EXC_BAD_ACCESS` inside `thread_queue_pop`, dereferencing a
  popped queue head that a caller-side `!thread_queue_empty()` guard had
  just certified non-empty, "with nothing else executing in THIS thread" --
  i.e. the queue's own head vanished between check and pop.
- Diagnosis ruled out the obvious suspects one at a time, with a hardware
  watchpoint as the actual tie-breaker (not a guess): four separately-named
  SI-manager candidate functions were individually cleared by full disasm
  read; a scheduler-wide `recursive_mutex` closing the check-then-pop TOCTOU
  was landed and confirmed live in the compiled binary (disassembly showed
  real `lock()`/`unlock()` bracketing) -- and the crash reproduced **20/20**
  at the identical site anyway, proving the mutex closed a real but
  different bug, not this one.
  - Prior rung's WCW_WATCH_ADDR-based diagnosis was *inconclusive/misleading*
    on this exact question (the same rdram address is reused earlier in boot
    by an unrelated function, and separately, `dladdr`'s `fn=` attribution
    on a large float-heavy function was shown to be an artifact of clang
    tail-merging near-identical slow-path stubs -- "do not trust
    WCW_WATCH_ADDR's fn= naming at face value... without cross-checking
    against a real hw watchpoint").
  - The eventual ground truth came from a **late-armed** real hardware
    watchpoint (armed only after the specific queue's creation, conditioned
    on the exact mq address) -- an env-var watch armed from process start
    could not isolate the actual writer among address reuse and other noise.
  - Final root cause identified: the field transitions via a **genuinely
    concurrent OTHER game thread's own recompiled MIPS code** executing
    `osSendMesg`'s blocking-insert path on the shared queue struct, touching
    raw rdram bytes with **no lock the scheduler API can see at all** --
    "it cannot stop two 'game' host threads from both executing arbitrary
    recompiled code that touches shared rdram bytes with no lock at all,
    which is the deeper version of the disease this rung's dispatch
    described."
- The explicit refusal on record: a "silently treat a low/implausible
  pointer as empty" guard was **drafted and reverted** -- "that would
  convert a hard, honest crash into silently losing a blocked thread
  forever."

The mechanism this whole rung exposes is upstream architectural, not a
one-off bug: giving every `OSThread` its own real host `std::thread` and
relying on a signal-then-return handoff (a semaphore signal without waiting
for the signaled thread to actually park) that has **no lock anywhere**
around `running_queue` or any `OSMesgQueue`'s blocked lists — so a second
"game" thread's recompiled MIPS code can be mid-instruction on shared rdram
at the same moment the first thread believes it has exclusive access. This
class of bug is exactly what the threading model below must make
structurally impossible, not merely less likely.

### OSMesgQueue's other invariant, independently confirmed (rung 12)

A second, independent piece of evidence about what the *data structures*
themselves assume, cited because it directly informs the `MesgQueue` design
in §3: rung 12 (`profile.toml`) found that leaving `osCreateMesgQueue`
un-named (its body still raw recompiled MIPS) meant every queue's
`blocked_on_recv`/`blocked_on_send` fields got initialized to a ROM
sentinel struct's address (`D_80048860`, a hardware dummy tail node with
`next=0, priority=-1`) instead of a real null. Runtime code that tested
"is anything blocked" via `*queue == NULLPTR` was always false against that
sentinel, so every send/recv treated it as a real blocked thread, and its
own `next` field (word `0`, reread as an in-rdram address) created a
self-loop that permanently corrupted the run queue's walk. **Lesson coded
into the design**: `osCreateMesgQueue`'s reset is not "zero some bytes," it
is "establish the empty-queue invariant these fields are load-bearing for,"
and that reset must be a single, non-bypassable constructor path — not
something any caller can reach around by writing raw fields (see the newtype
design in §2's `MesgQueue` below, and the `blocked-list ownership` point).

### Options evaluated

**(a) OS-thread-per-`OSThread` with a single-runnable baton.** One host
`std::thread`/`std::thread`-equivalent per `OSThread`, gated by a shared
token/mutex+condvar such that only the token holder may execute recompiled
code; `pause_self`/scheduler handoff releases the token and blocks on a
condvar until re-granted. This is architecturally what the reference runtime
already does (per the rung-18 evidence: "4 separate real host OS threads all
named 'game' alive simultaneously... this runtime gives every OSThread a
genuine std::thread, not a coroutine") — and rung 18/18b is the direct
demonstration of why it's fragile: the "single-runnable" property is an
*invariant maintained by convention across every call site that touches
scheduler state*, not a property the type system enforces. Every one of
`thread_queue_pop`/`insert`/`remove`/`schedule_running_thread` becomes a
place a missing lock (or a lock that's present but held over the wrong
window, per the fix that landed and still didn't close rung 18) reopens the
race, and — the harder problem — real game rdram touched by recompiled code
running on a second live host thread is *never* inside any of those guarded
functions, so no scheduler-level lock, however carefully placed, can close
it. Real preemption at the OS-thread level exists here even though the
model is trying to emulate a single core; correctness rests entirely on
every yield point being disciplined, forever.

**(b) Single executor + stackful coroutines (e.g. `corosensei`).** One real
host thread executes all game logic; each `OSThread` is a stackful coroutine
(its own machine stack, switched to and from cooperatively). "Only one game
thread runs at a time" stops being a discipline every future contributor
must maintain across N call sites and becomes **physically true** — there is
exactly one native call stack live in guest code at any instant, because
there is exactly one native thread executing it. A yield
(`pause_self`/blocking `osRecvMesg`/timer wait/scheduler switch) is a
`coroutine.yield()` back to the executor's scheduling loop, which picks the
next runnable coroutine per libultra priority rules and resumes it — all on
the same host thread, so "resume coroutine B" and "coroutine A's last write
to rdram" have a trivial happens-before relationship (sequential program
order on one thread), not a cross-thread visibility question requiring a
lock or atomic at all. `recomp_context`'s per-thread MIPS register state
naturally becomes coroutine-local (each coroutine owns its own
`recomp_context`, no shared mutable state to race on); the shared `rdram`
buffer is still shared, but now the only way two writes to it can interleave
is a yield point *the coroutine itself chose* (an explicit
`pause_self`/blocking-syscall boundary that the recompiled C emits), never
an arbitrary instruction boundary an OS scheduler picked. This makes the
rung-18 failure mode — "a second thread's recompiled code touches shared
rdram with no lock the scheduler can see" — **unrepresentable**: there is no
second thread.

The Rust recompiler lane uses the same model. Generated functions own
a safe `fn(&mut fn64_recomp_rs::RecompContext, &mut Rdram)` ABI, while
`fn64-abi::recompiled` is the single adapter at the already-unsafe C host-shim
boundary. It marshals GPR/HI/LO/COP0 status into the legacy host context,
calls the existing queue/DMA/VI/thread shim, then copies architectural state
back. `osCreateThread` constructs a recompiled context inside the same
`GameThread` coroutine; it does not create another executor, RDRAM image, or
host thread. The generated module also exports section `(ROM, static VRAM,
size)` geometry. The existing DMA load registry records relocated heap bases,
and host-first lookup maps a relocated callback back to its static typed
function entry. Thus rs and C lanes share scheduling, peripherals, and
memory ownership without pretending their register structs are layout-
compatible.

**(c) async (Rust `Future`s / an async runtime).** Model each `OSThread` as
an `async fn`, yielding at `.await` points, driven by a single-threaded
executor (e.g. a `LocalSet` / current-thread runtime). Shares (b)'s core
correctness property (one poller, one logical thread of control at a time)
but the ergonomic fit is poor for this specific workload: recompiled `C`
calls into `fn64-abi` are ordinary synchronous function calls with a
fixed `(rdram, ctx)` signature (per every extern surface entry in
`ABI-SURFACE.md`) — there is no natural `.await` point inside a
`RECOMP_FUNC` because the recompiled code was never rewritten to be async,
and retrofitting yield points would mean either (i) polling from inside a
non-async C call via a hand-rolled waker dance at every `pause_self`/blocking
call site (recreating stackful-coroutine mechanics on top of a strictly
worse primitive for this — Rust's stackless coroutines require the yield
point to be a syntactic `.await`, which recompiled C's call graph doesn't
have), or (ii) running each `OSThread`'s entire body as a blocking task on
a dedicated thread anyway, which collapses back into option (a)'s hazards.
Async's real strength — cheap concurrency for I/O-bound, deeply nested
call graphs with natural suspend points — doesn't match "run a fixed MIPS
call graph that suspends only at a handful of libultra API boundaries."

### Recommendation: (b), single executor + stackful coroutines

This is the load-bearing choice. Reasoning, mapped to the specific seams the
task calls out:

- **`pause_self` / yield sites.** Each libultra call that can block or
  voluntarily yield (`pause_self` itself — 3 call sites in NWXE, 2 in NW4E
  per `ABI-SURFACE.md`'s dispatch-helper table; `osRecvMesg_recomp` when the
  queue is empty; a blocking `osSendMesg_recomp` when the queue is full,
  the exact path rung 18b root-caused) becomes a single `yield_now()`-style
  call into the executor from inside the current coroutine. The executor's
  resume logic picks the next runnable `OSThread` by the same priority rule
  libultra specifies (see `osCreateThread`/`osSetThreadPri`'s semantics —
  highest-priority runnable thread runs) and resumes its coroutine, which
  is the *only* place execution physically transfers between "threads." No
  call site anywhere else in the runtime can accidentally run two
  `OSThread`s' recompiled code concurrently, because there is exactly one
  coroutine ever resumed.
- **VI/timer event delivery.** VI retrace and timer expiry are host-side
  events (real wall-clock/vsync driven), not guest compute — they must be
  able to interrupt/wake a blocked coroutine (e.g. a thread parked on
  `osRecvMesg` from `OS_EVENT_VI`) without themselves being a second
  "runnable game thread." Model them as executor-level scheduling inputs:
  the host VI/timer driver (in `fn64-runtime`, no coroutine of its own)
  posts to the target `OSMesgQueue`/marks the target coroutine runnable and
  returns; the *executor's* next resume decision (still made from the single
  active coroutine's yield point, or from the top-level scheduling loop
  between coroutine turns) is what actually runs the woken thread's code.
  This mirrors real hardware exactly: a VI interrupt on real N64 doesn't
  execute game code itself, it posts a message and returns to whatever the
  CPU was doing; libultra's own scheduler decides what runs next.
- **SI/PI completion messages.** Same shape: DMA completion is host-driven
  (a real disk/cart read finishing, or in fn64's case a host-file-backed
  ROM read finishing), and the correct model is "post the completion
  message to the registered `OSMesgQueue`, let the next coroutine-resume
  decision (not a new host thread) pick up the woken thread." This is
  exactly the shape `ultramodern::send_si_message`/`dequeue_external_messages`
  is evidenced to have in the rung-18b writeup — an external (non-coroutine)
  message source feeding the same queue machinery a blocking `osSendMesg`
  from guest code feeds — the design difference is only that in fn64 there
  is no second real thread that could race the queue mutation, because the
  actual mutation of "make thread X runnable" is executor-owned state
  touched only between coroutine resumes.

  The live device implementation now makes that ordering structural. Its
  RCP/MI authority exists from `HostState` construction and is not optional or
  coupled to cartridge ROM load order; a separate `rom_installed` invariant
  keeps PI DMA's missing-content path loud.
  `DeviceFabric` owns PI's sole in-flight request and guest-cycle deadline;
  managed EPI, raw PI APIs, and typed-Rust PI register writes all enter it.
  At the deadline it writes the process's one RDRAM allocation, clears PI
  busy, raises MI PI pending, and only then returns an executor notification.
  `advance_virtual_time` injects that notification before it returns. The
  translated checkpoint path also suspends first, advances executor time, and
  commits the fabric in `fn64-abi::run_one_step` before any later resume. The
  default one-cycle `FixedPiTiming` is an explicit,
  host-configurable compatibility policy because the allowed public manuals
  define PI domain parameters but not an exact completion formula; it is not a
  hardware-cycle-accuracy claim. The same fabric now owns AI's two-slot FIFO.
  It derives deterministic drain deadlines from stereo-frame count, the
  93.75 MHz CPU clock, and libultra's quantized DAC rate, then raises MI AI and
  returns OS_EVENT_AI only after the current buffer drains. This is guest-time
  ordering, not a claim of hardware-verified AI bus timing. Its DAC divisor
  uses the same IPL-selected NTSC/PAL/MPAL video clock as VI. SP, SI, VI, PI,
  AI, and DP pending bits and masks are
  one level-sensitive gate. Typed raw writes apply the acknowledgement commands
  documented by the public `rcp.h` register definitions: SP status bits 3/4,
  any VI_CURRENT/AI_STATUS/SI_STATUS write, and MI mode bit 11 for DP. In the
  same fabric, SI owns persistent 64-byte PIF RAM and schedules distinct
  DRAM-to-PIF command and PIF-to-DRAM response transfers. Completion order is
  `PIF/RDRAM bytes -> SI idle -> MI SI -> OS_EVENT_SI`; the current one-cycle
  deadline is an explicit policy because the public register definitions do
  not supply a transfer-cycle formula. Raw controller query/read commands are
  implemented; other raw PIF device commands remain loud gaps. The fabric also
  owns the RSP's persistent 4 KiB DMEM/IMEM, PC, status, atomic semaphore, and
  double-buffered SP DMA. DMA forces public 64-bit alignment, decodes
  length/count/skip rows, commits at an eight-setup-cycle plus one-cycle-per-
  64-bit-beat deterministic deadline, and increments an IMEM generation only
  after commit. The public `osSpTaskLoad` sequence copies all 64 OSTask bytes
  to DMEM `0xfc0`, aligned rspboot bytes to IMEM zero, resets PC, and clears a
  preceding task's SIG0/SIG1 yield handshake. `osSpTaskYield` writes the public
  `SP_SET_YIELD`/SIG0 command and returns immediately. After SP completion,
  `osSpTaskYielded` observes SIG1; a real acknowledgement sets
  `OS_TASK_YIELDED` and replaces `ucode_data`/`ucode_data_size` with the task's
  yield-buffer fields, while normal completion returns zero and leaves the
  task untouched. These semantics come from the public *RSP Programmer's
  Guide*, "Task Yielding", the `rcp.h` SP signal definitions, and the
  `osSpTaskYielded` manual page. The query never dispatches a backend or ucode,
  preventing a completed task from running twice. A renderer returning
  `FrameStatus::Yielded` drives the same SIG1 state and schedules SP completion
  without a premature DP completion; missing or failed renderer operations all
  pass through one loud gate rather than synthetic-completing. Reloading the
  rewritten task then calls the backend with `OS_TASK_YIELDED` and the saved
  data range, providing cooperative HLE resume even though host-atomic backend
  calls cannot yet be preempted mid-call. Known graphics/audio tasks now run
  the admitted rspboot through its real scalar interpreter until control first
  reaches an IMEM range installed by read DMA. RDRAM DMA writes, DMEM, the
  final IMEM generation, SP status, and ucode entry PC commit before the HLE
  backend represents the loaded-ucode phase; BREAK, DPC submission, or a
  bounded failure before that handoff traps loudly. Scalar/VU register state
  does not cross this optimization boundary. Unknown
  and custom tasks execute from that persistent image through the clean-room
  scalar/vector interpreter: IMEM DMA replaces a generation and resumes at the
  saved PC; BREAK commits DMEM, RDRAM DMA writes, status, and DRAM/XBUS DPC
  submissions before the guest resumes. Graphics HLE preflight is
  transactional and content-addressed: selecting an HLE decode mode admits no
  content. Both HLE backends return `NeedsLle` when the task-entry IMEM digest
  is unregistered; the reference renderer additionally decodes admitted tasks
  against cloned RDRAM/RSP state and rejects an unadmitted `G_LOAD_UCODE`
  generation. The clone is discarded and the complete ucode phase runs from
  untouched post-rspboot state through that interpreter. DRAM and staged XBUS
  DPC ranges then reach either the Rust raw renderer or RT64's bounded LLE RDP
  entry with the submission boundary's explicit VI output address; no raw path
  infers it from a preceding HLE call. This avoids an
  impossible fabricated mid-HLE scalar/VU transplant while preserving BREAK
  and DRAM/XBUS DPC effects. Exact graphics/audio HLE task
  effects after rspboot remain atomic backend calls, so SIG0 cannot yet
  preempt and resume them in the middle of a task. Completion no longer wakes
  the scheduler from inside `osSpTaskStartGo`: the fabric schedules SP at the
  measured rspboot instruction count plus one HLE policy cycle and graphics DP
  one cycle later, raising each MI source and returning each OS event in order.
  Those deadlines are a compatibility policy until hardware-derived RSP
  timing and real RDP FullSync provide the timing.
  VI is scheduled in this fabric rather than asserted after an executor ticker
  fires. Its 14-word raw register image is shared with typed MMIO;
  `VI_CURRENT` is derived from the programmed `VI_V_SYNC`: progressive output
  exposes the public even half-line sequence,
  while `VI_STATUS.SERRATE` alternates even/odd fields and the sampled low bit.
  Equality with `VI_INTR` raises the common MI source, and any `VI_CURRENT`
  write acknowledges it without replacing the sampled line.
  `osViSetMode` decodes the public `OSViMode` structure into that same image,
  retaining both five-word field register sets. Each interrupt selects the
  set matching live field parity, and its origin is added to the queued
  framebuffer base rather than misread as an absolute address.
  `osViSetSpecialFeatures` consumes the public `u32` ON/OFF command pairs—not
  a pointer—and composes gamma, gamma-dither, divot, and bit-16 dither-filter
  changes with the queued control image before the same interrupt latch.
  `osViSetXScale` and `osViSetYScale` validate their public ranges and multiply
  the mode's low 12-bit 2.10 coefficient while retaining its subpixel offset;
  a later mode call resets earlier overrides, while later scale calls compose
  into the queued mode.
  `osViGetCurrentLine`, `osViGetCurrentField`, `osViGetStatus`, and
  `osViGetCurrentMode` query that live state; a queued mode does not become
  current until the interrupt latch.
  At the interrupt, pending mode/scales/blanking/framebuffer state becomes
  current before the general OS_EVENT_VI target or `osViSetEvent` target can
  wake. The general event fires every field; the VI-manager target honors its
  public nonzero `retraceCount` divisor independently. Framebuffer and
  black/unblack transitions each trigger the renderer only after that latch.
  Typed `ViPresentation` state crosses the renderer boundary; the Rust
  reference backend keeps its RDP image separate from VI scanout, presents
  black without erasing that image, implements the public `osViFade` 10-bit
  interpolation of its first two rows, implements `osViRepeatLine`, and
  restores the unmodified source when each effect is disabled. These two
  public functions are beyond the canonical NMR inventory but are exported
  for general N64 software in both C and Rust-recompiler host-call lanes.
  Enabling black with an effective Y scale other than the manual-required 1.0
  traps loudly, as do blacking while fade/repeat is active and enabling fade
  and repeat together. The RT64 adapter sends the same typed state through its
  quarantined C boundary and programs the VI mechanisms directly: disabled
  pixel type for black, zero Y scale for repeat-line, and zero Y scale plus
  the 10-bit Y subpixel offset for fade. This was feature-compiled and its
  Rust/C/C++ ABI exercised by the RT64 unit build; pixel-level GPU capture is
  still an integration gate. A typed IPL television standard is
  the common VI/AI clock authority. Before a mode exists, VI uses the public
  nominal 60 Hz NTSC/MPAL or 50 Hz PAL rate; once H_SYNC and V_SYNC are
  nonzero, their public line/half-line units derive the next guest-cycle field
  interval from that standard's video clock. Hosts query the live interval at
  every injection point, so a latched mode changes the next deadline. This
  formula is clean-room derived from public register definitions and has not
  yet been checked against a hardware timing trace. Analog VI output filtering
  remains open.
  In the block
  lane, raw MI mask commands and RCP completion drive CPU IP2; the next
  instruction boundary applies the
  Status IE/IM/EXL/ERL gate, commits Cause/EPC/EXL, and resolves the BEV-selected
  handler through the active code mapping. The same boundary synchronizes the
  executor-owned half-rate Count/Compare clock: equality latches CPU IP7 and a
  handler's MTC0 Compare acknowledges it before ERET can resume. Generated-C
  translation units compile as C++ solely for `fn64_mmio_proxy.h`: its lvalue
  proxy preserves direct native-word RDRAM reads/writes but routes KSEG1 RCP
  `MEM_W` loads and assignments through the same raw handlers as the typed
  block lane. Subword RCP access traps loudly. This closes split register
  authority, but not the function lane's inability to suspend inside one
  generated function; tight timed-device polling still requires block-lane
  checkpoints.
- **Why rung-18-class races become unrepresentable, precisely.** Rung 18's
  actual root cause was not "the mutex was in the wrong place" — a mutex
  *was* added at exactly the TOCTOU the original hypothesis named, verified
  present in the compiled binary, and the crash reproduced unchanged 20/20.
  The real cause was a second genuinely-concurrent host thread executing
  recompiled MIPS code that touches shared rdram through no queue API at
  all — a category of write no scheduler-level lock can intercept, because
  it doesn't go through the scheduler. A stackful-coroutine, single-executor
  model removes the precondition entirely: there is no second host thread
  ever executing recompiled code, so there is no "genuinely concurrent write
  to shared rdram bytes with no lock the scheduler can see" to have in the
  first place. The invariant "exactly one game thread runs at a time" is not
  maintained by discipline at N call sites (as in (a)) — it is a physical
  fact about how many native call stacks exist, enforced by the executor
  loop itself, at exactly one place in the codebase.

### `OSMesgQueue` semantics, designed from the libultra manual + rung evidence

Modeled as (all in `fn64-runtime`, no `unsafe`, no direct field access from
`fn64-abi`):

```rust
/// Owns the invariant osCreateMesgQueue is documented (libultra manual,
/// "Message Manager") and rung 12 proved load-bearing: a freshly-created
/// queue's blocked lists are EMPTY, full stop -- never a stale/sentinel
/// value, never partially constructed. The only way to get a MesgQueue is
/// through this constructor; there is no path that produces one with a
/// non-empty blocked list, matching the ROM's own real osCreateMesgQueue
/// semantics (zero both fields) and closing off the rung-12 failure mode
/// (a caller writing raw struct bytes and leaving a sentinel/garbage
/// pointer where the runtime's "is anything blocked" check expects None)
/// by construction: there is no raw-write path in this API at all.
pub struct MesgQueue {
    buffer: Box<[Mesg]>,      // count-capacity ring buffer (osCreateMesgQueue's `msg`/`count` args)
    valid_count: usize,       // validCount: how many slots currently hold a real message
    first: usize,             // ring index of the oldest valid message
    blocked_on_recv: BlockedList,  // OSThreads parked in osRecvMesg on an empty queue
    blocked_on_send: BlockedList,  // OSThreads parked in osSendMesg on a full queue
}
```

- **Blocked-list ownership.** `BlockedList` is not a raw pointer/sentinel
  (the exact shape rung 12 found corrupting the run queue) — it is an
  `Option<CoroutineId>` chain owned exclusively by the executor's scheduler
  module, never touched by `fn64-abi` shim code directly. A shim
  (`osRecvMesg_recomp`, `osSendMesg_recomp`) calls a `fn64-runtime` method
  (`MesgQueue::try_recv`/`try_send` returning `Blocked` or `Delivered`); only
  the executor's yield/resume machinery ever mutates which coroutine is on
  a `BlockedList`. This means the field can never observe the rung-12 state
  (a queue whose blocked list "contains" a foreign, non-thread ROM address)
  because nothing outside this module's constructor and the executor's
  single mutation path can write it at all — there is no `unsafe`, no raw
  pointer cast, and no second writer to race.
- **What `osCreateMesgQueue` resets (rung 12).** `MesgQueue::new(buffer,
  count)` is the only constructor; it always produces `valid_count: 0,
  first: 0, blocked_on_recv: None, blocked_on_send: None`. There is
  structurally no way to observe a freshly-created queue with a non-empty
  blocked list, which is exactly the invariant rung 12 found the real ROM's
  `osCreateMesgQueue` establishes and found catastrophic when skipped
  (a queue whose fields still held whatever raw bytes were there before,
  interpreted by the empty-check as "something is blocked").
- **Send/recv as coroutine yield points, not thread ops.** `try_send`/
  `try_recv` return an enum (`Delivered(Mesg)` or `WouldBlock`); the
  `fn64-abi` shim, on `WouldBlock`, registers the current coroutine on the
  appropriate `BlockedList` and yields to the executor — this is
  `osSendMesg`'s blocking path, the exact one rung 18b root-caused as the
  actual (and previously un-suspected) source of the concurrent write. In
  this design that "concurrent write" cannot happen: registering on
  `BlockedList` and yielding are two steps of one sequential function running
  on the single executor thread, with no other coroutine able to observe or
  mutate the queue in between (nothing else is running).
- **Event queue registration (`osSetEventMesg`, VI/SI/PI sources).**
  Modeled as a small `EventTable: HashMap<OsEvent, (QueueHandle, Mesg)>` in
  `fn64-runtime`, populated by `osSetEventMesg_recomp`. VI/timer/SI/PI
  completion (host-driven, §2's yield-sites discussion) posts through this
  table by calling the *same* `MesgQueue` API a blocking guest `osSendMesg`
  would use — one code path, one invariant, whether the sender is "guest
  code" or "the host VI driver," closing the asymmetry that made rung 18b's
  external-vs-game-code distinction a source of confusion in the reference
  runtime (its `dequeue_external_messages` was a structurally separate path
  from `do_send`, per the profile.toml writeup, and telling which one was
  responsible for a given mutation was part of what made that rung hard).

### Implementation notes (wave 2/3, 2026-07-14): what building it taught us

This design's recommendation (option (b), `corosensei`) is implemented as
specified — no deviation from the chosen crate or the core "one host
thread, stackful coroutines, priority-ordered run queue" shape. Three
things the implementation surfaced that this doc didn't originally spell
out, recorded here honestly per `AGENTS.md`'s "mark revisions honestly":

- **`Yield`/`Resume` needed a `may_block` field, not just two "will
  definitely block" variants.** The original sketch modeled
  `BlockOnRecv`/`BlockOnSend` as always-blocking suspend points, with the
  `fn64-abi` shim expected to pre-check via an `Executor` method (e.g.
  `send_mesg`/`recv_mesg`) whether blocking was actually needed before
  deciding to yield. That pre-check is exactly what caused the bug below,
  so the real shape unifies `OS_MESG_BLOCK`/`OS_MESG_NOBLOCK` into ONE
  suspend point per operation: `Yield::BlockOnRecv { mq_addr, may_block }`/
  `Yield::BlockOnSend { mq_addr, msg, may_block }`. The executor's
  `handle_yield` (the only place that safely holds `&mut Executor` at this
  point) does the check-then-deliver-or-block-or-drop logic uniformly; a
  new `Resume::WouldBlock` variant carries the `OS_MESG_NOBLOCK`-on-
  unready-queue outcome back to a coroutine that yielded with
  `may_block: false`, which never gets parked on any blocked list. This is
  a strictly more precise version of the same design intent (§2's
  "Send/recv as coroutine yield points, not thread ops"), not a course
  reversal.
- **A real reentrancy bug, caught by this crate's own tests, in exactly the
  shape the pre-check above created.** `fn64-abi`'s coroutine bodies run
  physically nested inside `Executor::run_one_step`'s call to
  `GameThread::resume` — which itself runs inside whatever outer call
  (`run_one_step`/`run_to_idle`) invoked it. A coroutine body that called
  back into a `RefCell<Executor>`-guarded accessor (to pre-check "would
  this send block?") hit a live "RefCell already borrowed" panic on the
  very first such call, not a theoretical race: the outer borrow was still
  open on the same call stack. The fix (previous bullet, plus `fn64-abi`
  never touching its `EXECUTOR` thread-local from inside a coroutine body
  at all — even "which thread am I" is answered from a second thread-local
  populated alongside the active `Yielder`, never by asking the executor)
  is now load-bearing, commented at the fix site in both crates. This is
  the same *category* of bug rung 18 was — a hidden caller reaching state
  through an API that looked like a safe accessor — just caught by a type
  (`RefCell`'s dynamic borrow check) instead of a debugger, and inside this
  project's own new code rather than the reference runtime's.
- **`osCreateThread`'s real entry-point dispatch is a separate, larger
  piece of work than "wire the thread-lifecycle shim."** Calling the
  actual recompiled function a new `OSThread` should run requires the
  overlay/`get_function` lookup table (§1's `FuncEntry`/`SectionTableEntry`,
  wave 3's last listed item) which doesn't exist yet — `osCreateThread_recomp`/
  `osStartThread_recomp` are implemented as loud, named `unimplemented!()`s
  for exactly that missing piece (per `AGENTS.md`), not silently-succeeding
  stubs. Every other piece of thread/queue/timer machinery those two shims
  would eventually drive (`Executor::create_thread`/`start_thread`/
  `set_thread_pri`, the whole blocking send/recv/wake path) is implemented
  and tested for real, exercised end-to-end by this crate's own test
  harness standing in for the not-yet-written trampoline (see
  `fn64-abi/src/lib.rs`'s `tests::spawn_test_thread`).

### `Executor`/`Peripherals` module split (structure wave, 2026-07-14)

`fn64-runtime::executor::Executor` had grown into holding both its actual
job (run queue, `MesgQueue` registrations, timers, the `event_table`, and
the single `inject_event` door — the scheduling state §2's threading model
is about) AND host-side hardware-model state for three peripherals that
have nothing to do with the single-runnable-coroutine invariant: VI
(mode/y-scale/framebuffer-swap/retrace-ticker), SI/PIF (controller-probe
response shape), and RSP (task-header capture/counting). Every VI/SI/RSP
method lived directly in `impl Executor`, touching private `Executor`
fields (`vi`, `retrace`, `pif`, `tasks`) — a reviewer auditing "does this
change threaten the single-runnable-thread guarantee" had to read past
`osViSetMode`/`PifModel::query_response`-adjacent code to find the actual
scheduling logic, and vice versa.

**The fix**: a new `fn64_runtime::peripherals::Peripherals` struct now owns
those four fields and every method that only touches them
(`vi()`/`vi_set_*`/`vi_swap_buffer`/`arm_retrace`/`advance_retrace`,
`pif()`, `task_log()`/`submit_task`). Hardware RSP memory/register/DMA state
now lives separately in `DeviceFabric`; `Peripherals` retains only the OS-facing
task log. `Executor` holds exactly one
`peripherals: Peripherals` field and re-exposes the same public method
names as one-line delegations, so **no caller outside this crate changed**
— `fn64-abi`'s `with_executor(|exec| exec.vi_set_mode(...))`-shaped call
sites are byte-identical before and after this split; only where the
implementation lives moved.

Two things deliberately did NOT move to `Peripherals`, on purpose, not by
oversight:

- **`event_table`** (the `osSetEventMesg`-populated `OS_EVENT_*` →
  `(queue, msg)` table) stays on `Executor`. It is genuinely shared
  scheduling machinery — a guest `osSetEventMesg` registration and the VI
  retrace ticker's `OS_EVENT_VI` lookup both go through it, and
  `inject_event`'s `ExternalEvent::OsEvent` arm has no notion of which
  peripheral "owns" a given event code. Moving it into `Peripherals` would
  just relocate the god-object problem one file over instead of resolving
  it.
- **Trace recording** (`TraceLog`/`sim_time`) also stays on `Executor`.
  `Peripherals::vi_swap_buffer`/`submit_task` return the plain data
  (framebuffer address; task kind) the old single-body versions used to
  feed straight into `self.trace.record(...)` — `Executor`'s thin wrappers
  do that recording themselves, since `sim_time` is the executor's virtual
  clock, not a peripheral's own state.

This was a pure structural move: every `Peripherals` method's body is
character-for-character what used to be the matching `Executor` method's
body (see `peripherals.rs`'s module doc for the full mapping); no behavior,
field default, or trace-event shape changed. The existing test suite
(`fn64-runtime`'s unit tests, `rung_regressions.rs`, `fn64-abi`'s unit
tests) passes unchanged in both count and behavior — this is the gate a
pure-refactor claim like this one has to clear, not merely "it compiles."

### `ReentrantCell` audit verdict (structure wave, 2026-07-14)

The wave 2/3 implementation notes above record a real reentrancy bug fixed
by replacing `fn64-abi`'s `EXECUTOR: RefCell<Executor>` with
`EXECUTOR: ReentrantCell<Executor>`. This wave's task: is that cell still
earning its keep now that `Yield`/`Resume` (§2, `thread.rs`) already make
one whole class of reentrancy a compile-time non-issue, or was it only ever
papering over something the type system should be asked to catch instead?

**Verdict: still needed, and it guards a genuinely different hazard than
the one `Yield`/`Resume` closes — not a residual instance of the same one.**

- **What `Yield`/`Resume` + `RunToken` already prove, at compile time**: no
  second `GameThread::resume` can ever be invoked while a first is on the
  stack. `RunToken` is non-`Copy`, privately constructed, and
  `Executor::run_one_step` is the only place that both issues one and calls
  `resume` with it (`thread.rs`'s `RunToken` doc comment) — this is a
  *scheduling* reentrancy guarantee about resumes specifically.
- **What `ReentrantCell` guards, which is not a resume at all**: a
  coroutine body, once resumed and running as ordinary synchronous Rust
  code (no suspend, no yield), is free to call any `_recomp` shim as a
  plain nested function call — and several real, common shims
  (`osCreateThread_recomp`, `osSetEventMesg_recomp`, every VI setter,
  `osSetTimer_recomp`, etc.) themselves call `with_executor`. Since the
  OUTER `with_executor` call (`fn64-abi`'s own `run_one_step`/`run_to_idle`
  helpers, which wrap `Executor::run_one_step`/`run_to_idle`) is still
  nominally on the stack when this happens, the inner call is a **second,
  nested `with_executor` invocation while the first is still open** — not
  two threads, not two resumes, just an ordinary call stack `Yield`/`Resume`
  have no vocabulary for, because there is no suspend point here for either
  type to govern. `fn64-abi/src/lib.rs`'s
  `a_running_threads_own_body_can_call_os_create_thread_recomp_without_reentrancy_panic`
  test is the regression test for exactly this shape, reproducing what
  `examples/wm2000-boot`'s boot harness hit for real on its very first
  `osCreateThread` call.
- **Why this is memory-safe despite looking like `&mut` aliasing**: the
  outer `with_executor` closure does not read or write `Executor` state
  again until the inner, nested call returns — the two "live" `&mut`
  references are simultaneously in scope on the call stack but never
  simultaneously dereferenced. A plain `RefCell` cannot express that
  distinction (its borrow tracking is purely dynamic/stack-blind: a second
  `borrow_mut()` panics the instant it happens, regardless of whether the
  first borrow is actually being touched concurrently) — which is exactly
  the "already borrowed" panic that surfaced this bug for real.
- **Why this can't be pushed into the type system the way `Yield`/`Resume`
  were**: doing so would require making "a coroutine body calls another
  shim" itself a suspend point — i.e. a stackless/async redesign where
  every shim call is an awaited yield the executor's loop mediates.
  §2 already evaluated and rejected async for this exact workload
  (recompiled C's call graph has no natural `.await` points; forcing one
  in would mean hand-rolling the same suspend machinery on a worse
  primitive, or collapsing back to option (a)'s per-OS-thread hazards).
  Short of that redesign, this residual case is a property of ordinary
  synchronous Rust call stacks, not something a coroutine-yield type can
  see.
- **What this wave DID do, per the task's option (a)**: confirmed
  `with_executor` (`fn64-abi/src/lib.rs`) is already, structurally, the ONE
  gateway — `EXECUTOR` is a private `thread_local` with no other accessor
  anywhere in the crate, so every one of the ~30 `Executor`-touching call
  sites (every `_recomp` shim, every host-facing helper, every test) already
  funnels through it; there was no second, looser path to close. What was
  missing was the audit itself living at that gateway: `with_executor`'s doc
  comment now states precisely which reentrancy shape the type system
  already closes, which dynamic shape survives, and why, so a future reader
  doesn't have to re-derive this from the bug history to trust the cell is
  still doing real work and not just historical caution left in place.

`ReentrantCell` is not removed. It is not a second, redundant guard next to
`Yield`/`Resume` — it is the only mechanism that can cover this particular
shape at all, given the design this project already committed to (single
executor, stackful coroutines, synchronous shim calls). Removing it would
not be "relying on the type system instead" — it would just reintroduce the
exact panic `examples/wm2000-boot` hit, with no compile-time replacement
available under this architecture.

## 3. Memory model

### rdram buffer ownership

The 8 MB (or however large the target console's RDRAM is configured; N64 =
4/8 MB) `rdram` buffer is one stable allocation created by
`fn64-boot-harness::new_rdram(TvType)` and owned by the process harness for the whole
guest lifetime. `fn64-runtime::Rdram` owns the same layout in isolated core
tests and runtime-only configurations. Every consumer — `fn64-abi` shims, the
executor, and render task marshalling — borrows that one allocation; no
consumer makes a translated framebuffer/DMA copy and later treats it as
RDRAM. This matches the ABI contract directly: every `RECOMP_FUNC`/`_recomp`
shim receives the same `uint8_t* rdram` argument.

### The `MEM_*` accessor contract

`ABI-SURFACE.md` section (c) gives the exact, byte-cited semantics
(`refs/N64RecompSource` codegen, MIT, cited there) that any Rust-side
helper touching rdram from outside generated C (diagnostics, watch hooks,
save-state code) must reproduce exactly:

| Accessor | Width | Byte-lane XOR | Sign |
|---|---|---|---|
| `MEM_W` | i32 | none (word-aligned) | sign-extended |
| `MEM_H` | i16 | `offset ^ 2` | sign-extended |
| `MEM_B` | i8 | `offset ^ 3` | sign-extended |
| `MEM_HU` | u16 | `offset ^ 2` | zero-extended |
| `MEM_BU` | u8 | `offset ^ 3` | zero-extended |

The byte-lane XOR is real, load-bearing big-endian behavior (N64 MIPS is
big-endian; host RDRAM storage is native-endian by 32-bit word, so sub-word
access corrects the lane) — not a bug to "simplify away." The N64Recomp ABI
shape requires a little-endian host; `rdram.rs` rejects other targets at
compile time instead of pretending the native-endian dereferences plus XORs
are portable there.

`fn64-runtime` is the sole owner of the mapping:

- `RdramView` / `RdramViewMut` borrow a sized storage slice and accept only
  logical `RdramAddr`s. Host adapters, framebuffer conversion, diagnostics,
  and device bulk copies use these safe views.
- `RdramPtr` is the deliberately unsafe form for `_recomp` shims whose C ABI
  supplies a raw pointer but no length. It centralizes the same mapping while
  making the missing bounds proof explicit at construction/access.
- Owning `Rdram` methods delegate to the views; DMA, controller structs,
  audio PCM, both framebuffer capture paths, and the ReferenceBackend writer
  therefore exercise one implementation.

`scripts/lint-rdram-layout.py` sweeps production Rust for a hand-written
`^2`/`^3`, raw indexed RDRAM write, or raw-pointer RDRAM write outside
`rdram.rs`. Its self-test includes the former flat-big-endian framebuffer
writer, so the regression shape is mechanically rejected before a live boot.

### `RdramAddr` newtype

```rust
/// An N64 vram/kseg0 address as MIPS code computes it -- i.e. a 32-bit
/// value that may arrive already sign-extended to 64 bits in a `gpr`
/// (recomp_context's register fields are uint64_t, per ABI-SURFACE.md
/// section (b): "gpr is uint64_t; MIPS registers r0..r31 are all 64-bit
/// even though most recompiled ops operate via ADD32/SUB32/S32 32-bit-
/// truncating wrappers"). Constructing one performs the SAME translation
/// math the generated MEM_* macros perform (section (c): subtract the
/// full 64-bit sign-extended KSEG0 base 0xFFFFFFFF80000000, not the naive
/// 32-bit 0x80000000) so a value arriving as either a plain 32-bit vram
/// or its 64-bit sign-extended gpr form lands on the identical rdram-
/// relative byte offset -- this ambiguity is exactly what a hand-rolled
/// `addr - 0x80000000` at a second call site would get wrong for half of
/// its inputs.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct RdramAddr(u32); // stored as the resolved rdram-relative byte offset

impl RdramAddr {
    pub fn from_gpr(reg: u64) -> Self { /* replicates MEM_* base math, tested against
                                            both a plain-32-bit and sign-extended-64-bit
                                            input per ABI-SURFACE.md (c) */ }
}
```

Every layout-aware RDRAM API in `fn64-runtime` takes `RdramAddr`, never a bare
`u32`/`u64`; only allocation sizing and raw-storage construction operate on
host integers/slices. This is the "types before audits" rule from `AGENTS.md`
applied directly: an invariant (correct KSEG0 translation) that could be
silently gotten wrong at any of dozens of call sites is instead computed once,
in one constructor, and every other call site's type signature makes bypassing it
impossible.

### First-class watch/diagnostic hooks

Rung 18/18b is the direct design brief here: the reference runtime's
`WCW_WATCH_ADDR` env-var hook was shown to be **misleading** on the exact
question fn64 needs diagnostics to answer reliably — "who wrote this rdram
address" — for two independently-confirmed reasons in that writeup:

1. **Attribution via `dladdr`/return-address unslide is unreliable under
   compiler inlining/tail-merging.** The rung's own cross-check found a
   watch hit reported as belonging to `func_800E6178` (an unrelated
   trig/waveform routine) that was "very likely an artifact of clang
   tail-merging many near-identical slow-path stubs into a shared block" —
   i.e. the reported call site was a real address, just not a meaningful
   one for "which logical function did this." The rung's own conclusion:
   "do not trust WCW_WATCH_ADDR's fn= naming at face value... without
   cross-checking against a real hw watchpoint."
2. **A watch armed at process start can't distinguish reused-address
   history from the event actually being investigated** — the same rdram
   address the rung cared about had been written earlier in boot, for an
   unrelated purpose, by a totally different function; an always-on watch
   conflates both.

fn64's diagnostic model is designed to make both of these non-issues,
in `fn64-runtime` (not bolted on later, and not env-var-gated production
code with debug-only side doors — per `AGENTS.md`'s "no silent shrugs" and
this crate's testability goal):

- **A global monotonic sequence counter**, incremented on every rdram
  mutation that flows through `Rdram::write_*` (i.e. every write any
  `fn64-abi` shim or the executor itself performs — there is exactly one
  write path per §3.1, so there is exactly one place to increment). Every
  watch/log record carries this sequence number, which turns "is this
  address's write history from the window I care about, or stale reuse
  from earlier in boot" (problem 2 above) into a trivial range filter on
  the log, not a late-arm-the-watchpoint dance done by hand in lldb each
  time.
- **Reliable attribution by construction, not by unslide-and-guess.** Every
  write that goes through `Rdram::write_*` is called from a specific,
  already-known Rust call site — the `fn64-abi` shim function, or the
  specific executor/scheduler method, that invoked it. A watch hook records
  that call site directly (a `&'static str` function name baked in at the
  call site, or `#[track_caller]`'s `Location`) — this is categorically
  different from the reference runtime's approach of reconstructing "which
  function was this" from a raw return address via `dladdr` after the fact,
  which is exactly the step clang's tail-merging was shown to corrupt.
  There is no unslide-and-bisect step for fn64's hook to get wrong, because
  the caller identity was never lost in the first place.
- **Late-arming as a first-class query, not an lldb incantation.** The
  rung's eventual ground truth came from "a hardware watchpoint... armed
  right after the conditional breakpoint on `osCreateMesgQueue(mq_==...)`
  fires, i.e. genuinely late-armed." fn64 exposes this as an ordinary API —
  `Rdram::watch(addr, from_sequence: Option<u64>)` — so "start watching this
  address, but only care about writes after event N" is a query against the
  sequence-numbered log, not a hand-run debugger recipe that has to be
  redone from scratch for the next investigation.

## 4. A/B migration: link-time swap over identical `RecompiledFuncs`

### The core mechanism

Both the reference runtime and fn64 link against the byte-identical
`libRecompiledFuncs.a` that N64Recomp emits for a given game/profile — per
`README.md`: "Both runtimes link the *identical* recompiled code, so every
fn64 behavior gets A/B'd against reality before the swap." The swap is a
**link-time choice of which library provides the `_recomp`/`recomp.h`
extern surface** (`ABI-SURFACE.md` section (a)'s full inventory) that the
same, unmodified `RecompiledFuncs/*.c` object files call into — nothing
about the recompiled game code changes between the two configurations, only
which implementation of `osCreateThread_recomp`/`osRecvMesg_recomp`/etc. the
linker resolves those undefined symbols against. This is exactly the
`nm`-based "truly-external undefined symbol" completeness gate
`ABI-SURFACE.md` already runs per game/archive — the same gate doubles as
the A/B build's correctness precondition (if the symbol set fn64-abi
exports isn't a superset of what a given game's archive needs, the swap
fails to link, loudly, before ever running).

### Shared event-trace format

Both runtimes, when built with tracing enabled, emit the same structured
event stream so a diff tool never has to reconcile two different logging
formats:

```rust
pub struct TraceEvent {
    pub seq: u64,          // the global sequence counter from §3 (fn64 side);
                            // reference-runtime side assigns the same role
                            // to its own monotonic counter at emission time
    pub sim_time: u64,     // OS_CYCLES-comparable virtual time, not wall clock
    pub kind: TraceKind,
}

pub enum TraceKind {
    ThreadSwitch { from: ThreadId, to: ThreadId, reason: SwitchReason },
    QueueOp { queue: RdramAddr, op: QueueOpKind, thread: ThreadId }, // send/recv/block/wake
    Dma { direction: DmaDirection, dram: RdramAddr, dev_addr: u32, len: u32 },
    TaskSubmit { task_kind: TaskKind, ucode: u32 }, // RSP gfx/audio task handoff
}
```

Each event names *what changed*, not implementation-internal state, so it's
comparable across two structurally different implementations (OS-thread
model vs. coroutine model) — a `ThreadSwitch` event is meaningful whether
the "thread" underneath is a host `std::thread` being parked or a
coroutine being suspended; the comparator (below) only ever needs the
logical event stream, never runtime internals from either side.

### Comparator plan

A standalone tool (`fn64-shell`'s `--trace-compare` mode, or a small
separate binary once the format stabilizes) ingests two `TraceEvent`
streams — one from the reference runtime, one from fn64 — for the same
boot/input sequence, and asserts:

1. **Same `QueueOp` sequence per queue address** (modulo interleaving from
   `ThreadSwitch` ordering that both models are free to make differently
   as long as delivery order per queue is preserved — libultra's own
   message-queue contract is FIFO per queue, not a global total order).
2. **Same `Dma`/`TaskSubmit` sequence and payload sizes** — this is the
   direct differential-testing mechanism `AGENTS.md` requires ("Runtime
   behavior changes emit the shared event trace and get diffed against the
   reference runtime over identical recompiled code").
3. A structured diff report (first divergence: sequence number, event kind,
   both sides' payloads) — not a pass/fail bit; per this project's own
   verification-contract precedent (`CLAUDE.md`'s "never a fuzzy/bbox/partial
   match"), a diff that silently drops mismatched-but-similar events is
   worse than one that fails loud.

### Milestones

- **M1 — boot-to-idle parity.** fn64, linked against a real game's
  `RecompiledFuncs`, reaches the same idle/attract-mode depth the reference
  runtime's boot ladder has already validated (the playbook's rung
  progression is the existence proof this depth is reachable at all) —
  trace-compared clean, no divergence, for the deterministic (non-input)
  portion of boot.
- **M2 — current-rung parity.** fn64 reaches whatever rung the reference
  runtime's `profile.toml` most recently closed (today: past rung 18's
  scheduler_mutex fix, at the still-open TOCTOU-adjacent frontier) — i.e.
  fn64 is never the lagging system; its bring-up is paced by and validated
  against the reference's own hard-won ladder, not a separate one climbed
  from scratch.
- **M3 — full swap + shell rewrite + relicense.** fn64-shell replaces the
  reference runtime's own executable/windowing/input entirely; the GPL-3.0
  scaffold (`aki-recomp`'s vendored/forked runtime) is retired from the
  product's runtime dependency graph (it remains, permanently, the
  differential-testing oracle in CI, never the shipping runtime); the
  shipping artifact is MIT OR Apache-2.0 end to end, matching `README.md`'s
  license goal.

## 5. Work packages, sized in waves

Sequenced by dependency; items in the same wave parallelize (independent
files/crates, no shared state):

**Wave 1 — scaffolding (this doc's own deliverable).**
- Workspace skeleton, `fn64-abi`'s first representative symbols, C smoke
  test. (Parallelizes trivially against nothing — it's the prerequisite for
  every later wave.)

**Wave 2 — `fn64-runtime` core types (parallel sub-tasks, no shared state).**
**DONE (2026-07-14).**
- `Rdram` + `MEM_*`-equivalent accessors + `RdramAddr` (§3). Landed wave 1.
- `MesgQueue` + `BlockedList` + `EventTable` (§2) — `mesgqueue.rs` (landed
  wave 1) + `executor.rs`'s `event_table` field.
- The executor/coroutine scheduler (§2) — `executor.rs`'s `Executor`,
  priority-ordered run queue, `thread.rs`'s `GameThread`/`RunToken`/
  `Yield`/`Resume`. Rung regression suite (`rung_12_*`/`rung_14_*`/
  `rung_18_*` + ping-pong/full-queue-block/timer-ordering property tests)
  in `fn64-runtime/tests/rung_regressions.rs`.
- Timer wheel (`osSetTimer`/`osStopTimer` semantics, VI-tick-driven) —
  `timer.rs`'s `TimerWheel`, driven by `Executor::advance_time`'s virtual
  clock (no wall-clock in core, per this doc's requirement).
- Differential-trace scaffolding (`trace.rs`'s `TraceEvent`/`TraceKind`/
  global sequence counter, §4) landed alongside the executor rather than
  deferred to wave 6, since every executor event needed a place to record
  to from day one.
- See "Implementation notes (wave 2/3)" above this section for what
  building it taught us (the `may_block`/`Resume::WouldBlock` unification;
  a real ABI-layer reentrancy bug and its fix).

**Wave 3 — `fn64-abi` surface, by ABI-SURFACE.md's own grouping (parallel
per group once wave 2's matching runtime API exists).**
- `recomp.h` dispatch helpers: `pause_self`/`switch_error`/`do_break`/
  `get_function` **DONE** (M1 wave, 2026-07-14). This wave discovered and
  fixed a real signature mismatch from the prior wave's implementation:
  `pause_self` is `void pause_self(uint8_t *rdram)` (ONE argument, no
  `ctx`), `switch_error`/`do_break` take no `rdram`/`ctx` at all, and
  `recomp_context` is the REAL 32-gpr/32-fpr/hi/lo/f_odd/status_reg struct,
  not the 9-field subset a prior wave modeled — verified directly against
  `aki-recomp/games/NWXE/RecompiledFuncs/recomp.h` (N64Recomp's own
  MIT-licensed generated/vendored header) and real call sites, not
  re-derived from `ABI-SURFACE.md`'s prose alone. `get_function` is backed
  by the new `fn64-runtime::overlay::SectionRegistry` (§1's long-deferred
  overlay/`get_function` lookup table, built this wave — see below).
  The legacy corpus had no `cop0_status_*` call site per `ABI-SURFACE.md`; the
  arbitrary-PC lane now owns typed Status/Cause/EPC state because precise
  exceptions and interrupts require it independently of shim reachability.
- Thread lifecycle shims: `osCreateThread_recomp`/`osStartThread_recomp`
  **DONE** (M1 wave) — real dispatch via `SectionRegistry::resolve`, no
  longer `unimplemented!()`. `osSetThreadPri_recomp` **DONE** (prior wave,
  no dispatch-gap blocker). `osGetThreadPri`/`osGetThreadId` not yet
  reached.
- Message-queue shims: `osCreateMesgQueue_recomp`/`osSendMesg_recomp`/
  `osRecvMesg_recomp`/`osSetEventMesg_recomp`/`osSetTimer_recomp` **DONE**.
  `osJamMesg`/`osStopTimer_recomp` not yet reached.
- PI/SI/EPI DMA shims: `osCreatePiManager_recomp`/`osCartRomInit_recomp`/
  `osEPiStartDma_recomp`/`osVirtualToPhysical_recomp`/`osSetIntMask_recomp`/
  `osInitialize_recomp`/`osAiSetFrequency_recomp` **DONE** (M1 wave), backed
  by the new `fn64-runtime::rom` module (`RomStorage` trait, `PiDma`,
  `InMemoryRom`) — see §3's new "The PI/ROM seam" subsection.
  `__osSiRawStartDma_recomp`/`osSpTaskYielded_recomp` are loud, named
  `unimplemented!()`s (no real PIF-controller/RSP-task-execution model
  exists yet; see their doc comments in `fn64-abi/src/lib.rs` for why a
  silently-succeeding stub would be worse). `osEPiStartDma_recomp`'s
  `OSIoMesg` field-offset assumptions are flagged NOT YET byte-verified
  against a real ROM struct-init call site — honest "not verified," not a
  false "done," per `AGENTS.md`.
- VI/AI shims: `osAiSetFrequency_recomp` **DONE**. The `osVi*` family
  (`osViSetMode`/`osViSetSpecialFeatures`/`osViSetYScale`/`osViSwapBuffer`/
  `osViBlack`) are loud, named `unimplemented!()`s (T2 per
  `aki-recomp/runtime/M1-WORKLIST.md` — needed for the boot chain to
  complete, but no display/VI-hardware backend exists in this crate yet;
  that's `fn64-shell`'s wave-5 windowing piece). Implemented from the
  union (not either game's current subset) per this section's original
  guidance.
- `recomp_overlays.inl` consumption **DONE** (M1 wave):
  `fn64-runtime::overlay::SectionRegistry` (`Section`/`FuncEntry`, §1's
  shapes) resolves `get_function`'s `vram -> recomp_func_t*` lookup,
  correctly modeling NWXE's REAL bank-switch overlap (sections 2/5 and 3/4
  both declare the same `ram_addr` range in the actual
  `recomp_overlays.inl` — verified by reading the generated file directly)
  via an explicit `loaded: HashSet<SectionIndex>` rather than a flat
  address map, so only the currently-PI-mapped bank's functions resolve.

**M1 gate (2026-07-14): WM2000 (NWXE) `RecompiledFuncs` links clean against
`fn64-abi`.** Per `aki-recomp/runtime/M1-WORKLIST.md`'s 23-symbol undefined
set (16 T1 + 7 T2): all 51 `RecompiledFuncs/*.c` files recompiled fresh from
source, archived, and trial-linked (`-force_load` + a stub `main`, the same
method `M1-WORKLIST.md` used to derive the 23-symbol set) against a
release build of `fn64-abi` — **zero undefined symbols remain** beyond
ordinary libc/pthread/dyld/Rust-runtime symbols (confirmed via `nm -u` on
the linked binary, grepped for any `recomp`/`os*`/`switch_error`/`do_break`/
`get_function`-shaped name: none found). T1 symbols are real, tested
implementations; T2 VI-family symbols are loud named traps by design (no
display backend exists yet), which is sufficient for THIS gate (a clean
*link*, not a clean *boot to idle* — that's M1's "boot-to-idle parity"
milestone in §4, separate and not yet attempted).

**M1 boot-host attempt (2026-07-14): `examples/wm2000-boot`, first real boot
run against the linked archive.** Per the task's own scope (a headless boot
host taking `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM` env vars, zero game content
in-repo — `examples/wm2000-boot/build.rs` and the shared
`crates/fn64-boot-harness/bridge/section_bridge.c`): this is
the FIRST time the M1-linked archive was actually RUN, not just linked, and
it surfaced four real, load-bearing bugs the trial-link gate above could not
have caught (a clean link says nothing about correct runtime behavior):

1. **`fn64-abi`'s `EXECUTOR` reentrancy.** A plain `RefCell<Executor>`
   panicked ("already borrowed") the moment ANY non-blocking `_recomp` shim
   (e.g. `osCreateThread_recomp`) ran as part of `Executor::run_one_step`'s
   own coroutine resume — not a rare edge case, the NORMAL path for a
   running thread creating another thread. Fixed via `ReentrantCell`, a
   documented, single-thread-only interior-mutability wrapper (see its doc
   comment in `fn64-abi/src/lib.rs` for the full soundness argument); a new
   regression test drives the exact nested shape.
2. **`osStartThread`/`osSetThreadPri`/`osGetThreadPri` were keyed on the
   wrong identity.** A prior wave's doc comment asserted real call sites
   pass the same `OSId` to `osStartThread` that `osCreateThread` received —
   real disassembly (`funcs_0.c` asm 0x800004AC-0x800004B8) disproves this:
   both calls pass the SAME `OSThread*` handle, never the `OSId` a second
   time, and `osSetThreadPri(t=NULL, pri)` means "the calling thread," a
   documented libultra convention. Fixed via `HostState::thread_handles` (an
   `OSThread* -> OSId` map populated by `osCreateThread_recomp`) and
   `resolve_thread_arg`'s null-means-self handling.
3. **`osCreateThread_recomp` never seeded the new thread's stack pointer.**
   `entry_ctx.r29` was left zeroed; the real `sp` argument (stack-passed,
   per `osCreateThread`'s documented signature) was read but discarded. Any
   real thread entry point touching its own stack (i.e. every one) crashed
   immediately. Fixed by seeding `entry_ctx.r29` with the real `sp` value.
4. **`MEM_W`/`MEM_H`/`MEM_HU` are NATIVE-endian, not big-endian.** The
   single most consequential correction: `fn64-runtime::Rdram`'s word/
   halfword accessors and `fn64-abi`'s `read_stack_word` all used
   `from_be_bytes`/`to_be_bytes`, based on a prior wave's mistranscription
   of `ABI-SURFACE.md` section (c)'s prose summary. The generated `recomp.h`
   macro itself (quoted directly, MIT) is `*(int32_t*)(rdram + ...)` — a
   PLAIN NATIVE POINTER DEREFERENCE. The `^2`/`^3` byte-lane XOR on
   sub-word accessors exists BECAUSE the backing store is native-endian
   (little-endian on every real fn64 host); it corrects sub-word addressing
   relative to that, and would be pointless if the store were actually
   big-endian. First caught when a spawned thread's own real stack pointer
   came back exactly byte-swapped. Fixed throughout `Rdram`'s accessors and
   every `fn64-abi` call site that hand-rolled the same assumption
   (`osRecvMesg_recomp`, `read_os_task_header`, several tests).
5. **`osEPiStartDma_recomp`'s `dramAddr`/`retQueue` fields need KSEG0
   translation, and a sibling double-translation bug.** `dramAddr`/
   `retQueue` are raw vram POINTERS the game computed normally — they need
   `RdramAddr::from_gpr`'s translation like any other vram value, not
   `RdramAddr::from_offset` (no translation, silently wrong). Separately,
   the OTHER `OSIoMesg` fields were being read via `read_stack_word`, which
   itself re-applies the KSEG0 subtraction to an already-resolved
   `mb_addr.offset()` — a double subtraction producing garbage. Fixed via a
   new sibling helper (`read_offset_word`, takes an already-resolved
   offset, never re-translates) plus correcting the two vram-pointer fields
   to `from_gpr`.

**Result, honestly reported:** boot now progresses far past every prior
milestone — thread 0 (`recomp_entrypoint`) runs its real body, spawns and
starts a second real thread with a correctly-seeded stack, that thread
(id 6) runs real recompiled code three call-levels deep
(`func_800222D8` → `func_80003720` → `func_80000660`) into a REAL
`osEPiStartDma_recomp` PI-DMA call that completes without crashing. Boot
then reaches a state that runs for tens of seconds of wall-clock CPU time
inside a single `Executor::run_one_step` call with no crash and no log
output — i.e. the recompiled code is executing a real (long or unbounded)
recompiled loop inside `func_800004D0` that this milestone's stubs never
observed to terminate, most likely because our SI/PIF or PI-DMA completion
model isn't yet posting whatever the game's own poll loop is waiting for.
**Not a false "boot to idle"**: this is the honestly-reported frontier —
three `TraceEvent`s recorded, VI retrace never reached (no `osViSetMode`
call observed before the stall), zero framebuffer swaps, zero RSP tasks
submitted. `fn64-abi`'s 4 real bugs above are fixed and regression-tested;
the stall itself is a new, not-yet-root-caused frontier for the next wave,
not something papered over. The out-of-tree `wm2000_audio.cpp` (RSPRecomp's
own generated audio ucode) could not be linked at all in this wave: RSPRecomp's
codegen template unconditionally emits `#include "librecomp/rsp.hpp"`, which
lives under `N64ModernRuntime`'s GPL-3.0-licensed tree (verified: that repo's
top-level `COPYING` is GPL-3.0; `librecomp/` is not under the MIT-carved-out
`N64Recomp/` subdirectory) — a real, load-bearing clean-room blocker, not
routed around. The audio task-dispatch plumbing now owned by
`osSpTaskStartGo_recomp` (`set_audio_ucode_fn`) is real and tested against a stand-in function; the
genuine ucode requires either an MIT-clean RSP interpreter or a forked
RSPRecomp codegen target, both future work.

**Wave 4 — `fn64-rt64` bridge (parallelizes against wave 3, converges at
the RSP task boundary).**
- RSP audio-ucode task submission (the one RESOLVED boundary per
  `ABI-SURFACE.md` (e): `games/NWXE/rsp/wm2000_audio.toml`'s byte-verified
  `text_offset`/`text_address`/entry points).
- Gfx task handoff — explicitly blocked on real evidence per §1's rationale
  (3): do not guess the shape; wait for a profile.toml rename wave to reach
  an `osSpTaskLoad`/`osSpTaskStartGo` call site, then extract the real
  signature the same mechanical way `ABI-SURFACE.md` extracted everything
  else, before writing this wave's code.

**Wave 5 — `fn64-shell` (depends on wave 3 substantially complete).**
- Window/input/audio-out backend selection.
- ROM/`RecompiledFuncs` intake (user supplies their own recompiler output —
  no game content ships in this repo, ever).

**Wave 6 — differential harness (parallelizes against waves 2-5 once each
lands its first behavior; grows incrementally, never "done" as a single
wave).**
- `TraceEvent`/`TraceKind` types + emission call sites (§4).
- Comparator tool.
- CI wiring: boot a pinned game/profile under both runtimes, diff the trace,
  fail loud on first divergence.

## 6. Provenance appendix

Every source consulted while writing this document, and what it licensed us
to claim:

| Source | License / kind | What it informed |
|---|---|---|
| `aki-recomp/docs/BOOT-LADDER-PLAYBOOK.md` | our own method doc | §2's decision-tree framing, validation-bar language, tool-to-question map |
| `aki-recomp/games/NWXE/profile.toml` rung 12 comment block | our own debugger/disasm evidence trail | §2 and §3's `MesgQueue`/`osCreateMesgQueue` reset invariant |
| `aki-recomp/games/NWXE/profile.toml` rung 18 / 18 follow-up / 18 follow-up #2 comment blocks | our own lldb + hardware-watchpoint evidence trail | §2's threading-model case study; §3's watch/diagnostic-hook design (what failed and why) |
| `aki-recomp/runtime/ABI-SURFACE.md` + `runtime/abi_surface.json` | mechanically extracted from N64Recomp-generated C (both games) + `recomp.h`/`symbol_lists.cpp` (MIT) + `librecomp/include/librecomp/sections.h` (public interface header, ABI only) | §1's crate boundaries and Wave 3's symbol grouping; §3's `recomp_context`/`MEM_*` semantics; §4's link-time-swap/`nm`-completeness mechanism |
| `fn64/README.md`, `fn64/AGENTS.md`, `fn64/CONTRIBUTING.md` | our own project docs | Crate names (final, per README's table), validation bars, clean-room protocol, licensing goal |
| `aki-recomp/AGENTS.md`, `aki-recomp/PINS.md` | our own project docs | Cross-repo context: which repo is the behavioral-spec source, pinned reference commit hygiene |
| Public libultra manual (message-manager / thread-manager sections; general knowledge of `osCreateMesgQueue`/`osSendMesg`/`osRecvMesg`/`osSetEventMesg`/`osCreateThread`/`osSetThreadPri` semantics — priority-based scheduling, FIFO per-queue delivery, blocking vs. non-blocking send) | public documentation | §2's `OSMesgQueue` semantics, priority-based resume ordering |

Explicitly NOT consulted, per the clean-room protocol in `AGENTS.md`:
`vendor/N64ModernRuntime/**/*.cpp,*.hpp` (ultramodern/librecomp
implementation bodies) — every claim about the reference runtime's actual
behavior above is sourced from our own black-box observation (lldb
backtraces, hardware watchpoints, disassembly of the compiled binary, the
mechanically-extracted ABI surface), recorded in `aki-recomp`'s own
evidence trail, never from reading its GPL implementation source.
