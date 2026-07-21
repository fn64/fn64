# Decoupling plan: fn64-recomp and fn64-render

**Goal (user, 2026-07-14):** forking N64Recomp and RT64 is fine as a *bridge*,
not a destination. Restructure so fn64 depends on its own **interfaces**, with
the upstream forks plugged in behind adapters we can swap out — then rebuild
each correctly, with tests, on our own terms and timeline.

The discipline: turn "fn64 depends on N64Recomp / RT64" into "fn64 depends on a
`Recompiler` trait / a `RenderBackend` trait, and one adapter crate happens to
satisfy it with the fork today." Every other crate talks to the trait. The day
we finish our own implementation, we change one dependency line.

---

## Recompiler seam — `fn64-recomp`

### Our actual dependency surface (measured, small)

We use N64Recomp as a **two-binary CLI over a file contract**, nothing deeper:

- **Input:** a TOML config we generate (`aki_profile`). Keys in use:
  `[input]` entrypoint / rom_file_path / bss_section_suffix / symbols_file_path
  / output_func_path / trace_mode; `[patches]` stubs / ignored;
  `[[patches.instruction]]` func/vram/value; `[[patches.hook]]` func/before_vram/text.
  Plus the symbol TOML (`dump.toml`: sections + functions + data).
- **Output:** generated C (`RecompiledFuncs/*.c` + `recomp_overlays.inl` +
  `lookup.cpp`) against the ABI in `crates/fn64-abi` (`ABI-SURFACE.md`).
- **RSP:** RSPRecomp, same shape, for microcode → C.

That's the whole contract. It is a clean seam because it already IS one.

### The trait (crate: `fn64-recomp`)

```rust
/// A static recompiler: symbol/patch metadata + ROM in, generated C + an
/// ABI manifest out. The fn64 ABI (fn64-abi) is the fixed target; any impl
/// must emit code that links against it.
pub trait Recompiler {
    fn recompile(&self, cfg: &RecompConfig) -> Result<RecompOutput, RecompError>;
    fn recompile_rsp(&self, cfg: &RspConfig) -> Result<RecompOutput, RecompError>;
    /// The ABI version this recompiler targets — checked against fn64-abi so a
    /// mismatch fails loudly at plug-in time, not at link time.
    fn abi_version(&self) -> AbiVersion;
}
```

`RecompConfig` is our own typed representation (not TOML strings) — sections,
functions, stubs, patches, hooks — so callers never hand-serialize N64Recomp's
format. The adapter does that translation.

### Adapter today: `fn64-recomp-n64recomp`

A crate that implements `Recompiler` by (a) serializing `RecompConfig` to
N64Recomp's TOML, (b) shelling out to our pinned MIT fork's binaries, (c)
collecting the generated C. It owns every N64Recomp-specific quirk (the TOML
key names, the CLI flags, the `force_load` archive semantics). **This is the
only crate that knows N64Recomp exists.** It gets its own test suite: golden
tests that a known `RecompConfig` → expected TOML, and a round-trip that a
tiny fixture ROM recompiles + links against fn64-abi.

### Our implementation later: `fn64-recomp-rs`

A Rust MIPS→Rust (or →C) emitter implementing the same trait. Built
incrementally against the SAME golden/round-trip tests the adapter passes, so
we can run both over identical input and diff — the recompiler gets the same
A/B treatment the runtime already has. When it reaches parity, flip the default;
delete the adapter when no one needs the fork.

---

## Renderer seam — `fn64-render` (RT64 today, ours later)

### Our actual dependency surface

We reach RT64 through exactly one boundary: **process a gfx display-list task
→ a rendered frame**, plus lifecycle (create device/window, present, resize).
Everything RT64-specific (D3D12/Vulkan/Metal and native HLE execution) lives
behind that. Backend-neutral content-addressed microcode catalogs, immutable
ordered task/self-load plans, and raw-DPC FullSync inspection live in
`fn64-render`, so native and reference backends cannot drift on those handoff
invariants. `fn64-rt64` (→
`fn64-render-rt64`) already exists as the intended quarantine crate.

### The shared seam (crate: `fn64-render`)

```rust
/// A graphics backend: consumes N64 gfx tasks (F3DEX-family display lists from
/// rdram) and produces frames. The runtime submits tasks through the single
/// executor event seam; the backend never reaches back into runtime state.
pub trait RenderBackend {
    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError>;
    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError>;
    fn present(&mut self, request: PresentRequest<'_>) -> Result<(), RenderError>;
    fn resize(&mut self, w: u32, h: u32);
    /// Which microcode GBIs this backend actually implements — a task using an
    /// unlisted ucode traps by name (no silent black frame).
    fn supported_ucodes(&self) -> &[UcodeId];
}
```

The RSP-memory argument is the device fabric's one persistent DMEM/IMEM
image, not backend-private scratch. It is explicit in the type boundary so
`G_DMA_IO`, ucode overlays, CPU SP-memory access, and later commands in the
same task cannot accidentally observe different banks. The backend still has
no callback into runtime state: it receives exactly the two mutable memory
resources the task can affect.

The same crate owns exact SHA-256-to-wire-family catalogs for complete IMEM
images, immutable plans retaining task entry plus every ordered self-load
generation, and the public RDP Command Summary Table 11 width classifier used
to inspect raw DPC ranges for FullSync. These are backend
admission/completion mechanisms, not renderer implementations: the reference
rasterizer and RT64 adapter consume them through the same typed API. The RT64
adapter also binds each task to a schema-checked immutable ABI encoding of that
plan. Its pinned pre-cache observer forces native recognition and requires
exact ordered exhaustion, including same-address and `A -> B -> A` generations;
precommit incompatibility returns typed `NeedsLle`, while divergence after
execution begins poisons the context.

`PresentRequest` co-binds `ViPresentation`'s V-blank-latched scanout state with
a move-only `PhysicalRdramRead` capability for that exact retrace. Integrated
execution creates the capability while the guest is suspended and without a
competing Rust slice; its higher-ranked lifetime prevents a safe backend from
retaining process memory. The `live` constructor requires `Registers`; explicit
backend-resident and physical-memory compatibility constructors require
`BackendOnly`, so neither can silently satisfy live `VI_ORIGIN` registers or
produce authoritative release capture. The reference backend therefore keeps
its RDP image and its RDRAM-derived presented image distinct, rereads
RGBA16/RGBA32 source bytes on every field, and never destroys the RDP image
while applying black, the public 10-bit fade factor, or first-line repetition.
The RT64 adapter passes
the same current allocation and typed controls over its foreign boundary,
waits the native worker queues idle, restores placeholder RDRAM aliases before
returning, and does not rewrite the RDP image during presentation. Native
preflight consumes `fn64-render`'s typed footprint for the source rows selected
by public coordinate arithmetic; the deterministic reference filter halo
remains a separate policy.

### Adapter today: `fn64-render-rt64` (= the current `fn64-rt64` (→ `fn64-render-rt64`), renamed by role)

Implements `RenderBackend` over the MIT RT64 fork via FFI. Owns all C++ interop.
It temporarily also contains the pure-Rust `ReferenceBackend`; that backend is
being extracted without changing the frozen `fn64-render` seam.
The runtime and shell see only `dyn RenderBackend`. Tests: task-fixture replays
(a captured display list → a frame hash), and the trap path (unlisted ucode →
named error, not a crash).

### Our implementation later: `fn64-render-wgpu`

A Rust/wgpu HLE renderer implementing the same trait — the same swap story as
the recompiler. Not started until fn64 renders real frames through the RT64
adapter first (we need the reference to diff against).

---

## Sequencing (small steps, tests at each)

**Status 2026-07-17: steps 1 and 3 shipped; step 2 was overtaken by events.**
This list is kept for the rationale, not as a work queue — read the annotations
before following it.

1. ~~**Define the shared seam**~~ **DONE.** `fn64-render`'s `RenderBackend`,
   content-addressed ordered admission, and raw-DPC completion inspection live at
   `crates/fn64-render/src/lib.rs`; the recomp side landed as the `c`/`rs` lane
   split (DESIGN.md §1.1).
2. ~~**Wrap the recompiler fork** as `fn64-recomp-n64recomp`~~ **NOT DONE, and
   not the plan any more.** No such crate exists or should be created. The
   project went further than wrapping: `fn64-recomp`/`fn64-recomp-rs` are our
   own Rust-emitting recompiler, and the fork is consumed as the `c` lane
   instead of being adapter-wrapped. `aki_profile` is legacy (ROADMAP Phase H).
3. ~~**Rename `fn64-rt64` → `fn64-render-rt64`**~~ **DONE**, behind the
   `RenderBackend` trait; fixture-replay tests live in
   `crates/fn64-render-rt64/tests/`.
4. **Rebrand the forks by role**: `fn64/n64recomp` stays (it's literally a fork,
   honest name), but our crates and docs speak in fn64-recomp / fn64-render terms
   so the *project's* vocabulary is already decoupled before the code is.
5. Rust implementations (`-rs`, `-wgpu`) land later against the frozen
   trait + shared test suites, A/B-diffed, swapped when at parity.

## Why adapters, not a rewrite-now

A rewrite-now stalls everything behind a from-scratch recompiler/renderer. The
adapter makes the *dependency* swappable immediately (one crate knows the fork),
lets both games keep climbing on the working fork, and gives the eventual Rust
build a ready-made test harness to prove itself against. Decoupled today,
rebuilt correctly on our schedule.

---

## Crate plan (2026-07-14) — what earns a crate, what doesn't

A crate marks a **swap boundary** (backend you can replace) or a **cross-tool shared type**.
Not a topic. MMIO / save / libultra are modules *inside* `fn64-runtime`, not crates.

**Exists:** fn64-runtime, fn64-abi, fn64-render (trait + neutral render
mechanisms), fn64-render-rt64 (RT64 adapter + reference raster pending
extraction), fn64-shell.

**To add, prioritized:**
1. **fn64-audio** — the `AudioBackend` trait (consume AI samples → host stream) + a cpal backend +
   the RSPRecomp'd-ucode path behind it. Symmetric with fn64-render. Land the audio work AS this
   crate, not scattered into fn64-abi/runtime. (Refactor wave, on a green tree.)
2. **fn64-recomp** — the Recompiler adapter trait (see top of this doc). Overdue: N64Recomp
   shell-out still lives in aki-recomp/aki_profile. Home for fn64-recomp-rs later.
3. **fn64-shell promotion** — NOT a new crate: move the common boot-host logic (load ROM → register
   sections → install rdram → run entrypoint → drive backends) out of examples/{wm2000,oot}-boot
   into fn64-shell so the examples become thin mains and the shell is a real product binary.
4. **fn64-trace** — extract the differential-trace types (thread switch / queue op / DMA / task
   submit) from fn64-runtime WHEN the A/B comparator exists (a cross-tool shared type). Not before.
5. **fn64-cpu / semantics spec** — the recomp_context + MEM/sign-extension/COP1 contract that
   translated instructions target. Extract WHEN fn64-recomp-rs starts (it must emit against a
   spec, not fn64-abi's incidental layout). Not before.

**Deliberately NOT fn64 crates:** the game-profile toolchain (AKI-specific, stays in aki-recomp
until proven general — keeps fn64 game-agnostic); fn64-mmio / fn64-save (runtime modules, not
boundaries); fn64-libultra (inseparable from the executor).

**Orchestration rule learned the hard way (2026-07-14):** serialize waves that edit the same
crate's source. fn64-abi is a shared chokepoint; render + bulk-shim ran parallel on it and left
the tree non-compiling. Boot ladders and per-game profiles are naturally disjoint; shared fn64
crates are not — one wave at a time on fn64-abi.
