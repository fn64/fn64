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

### Our implementation later: `fn64-recomp-native`

A Rust-native MIPS→Rust (or →C) emitter implementing the same trait. Built
incrementally against the SAME golden/round-trip tests the adapter passes, so
we can run both over identical input and diff — the recompiler gets the same
A/B treatment the runtime already has. When it reaches parity, flip the default;
delete the adapter when no one needs the fork.

---

## Renderer seam — `fn64-render` (RT64 today, ours later)

### Our actual dependency surface

We reach RT64 through exactly one boundary: **process a gfx display-list task
→ a rendered frame**, plus lifecycle (create device/window, present, resize).
Everything RT64-specific (D3D12/Vulkan/Metal, HLE microcode handling) lives
behind that. `fn64-rt64` (→ `fn64-render-rt64`) already exists as the intended quarantine crate.

### The trait (crate: `fn64-render`)

```rust
/// A graphics backend: consumes N64 gfx tasks (F3DEX-family display lists from
/// rdram) and produces frames. The runtime submits tasks through the single
/// executor event seam; the backend never reaches back into runtime state.
pub trait RenderBackend {
    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError>;
    fn process_task(&mut self, rdram: &[u8], task: &OsTask) -> Result<FrameStatus, RenderError>;
    fn present(&mut self) -> Result<(), RenderError>;
    fn resize(&mut self, w: u32, h: u32);
    /// Which microcode GBIs this backend actually implements — a task using an
    /// unlisted ucode traps by name (no silent black frame).
    fn supported_ucodes(&self) -> &[UcodeId];
}
```

### Adapter today: `fn64-render-rt64` (= the current `fn64-rt64` (→ `fn64-render-rt64`), renamed by role)

Implements `RenderBackend` over the MIT RT64 fork via FFI. Owns all C++ interop.
The runtime and shell see only `dyn RenderBackend`. Tests: task-fixture replays
(a captured display list → a frame hash), and the trap path (unlisted ucode →
named error, not a crash).

### Our implementation later: `fn64-render-wgpu`

A Rust/wgpu HLE renderer implementing the same trait — the same swap story as
the recompiler. Not started until fn64 renders real frames through the RT64
adapter first (we need the reference to diff against).

---

## Sequencing (small steps, tests at each)

1. **Define the traits** (`fn64-recomp`, `fn64-render`) with the typed configs +
   the ABI/ucode version checks. No behavior yet. Compiles, documented.
2. **Wrap the recompiler fork** as `fn64-recomp-n64recomp`; move `aki_profile`'s
   shell-out logic behind it; golden + round-trip tests. `aki_profile` now calls
   the trait. **N64Recomp is named in exactly one crate.**
3. **Rename `fn64-rt64` (→ `fn64-render-rt64`) → `fn64-render-rt64`** behind the `RenderBackend` trait when
   the first-frame wave gives us a real task to render; fixture-replay tests.
4. **Rebrand the forks by role**: `fn64/n64recomp` stays (it's literally a fork,
   honest name), but our crates and docs speak in fn64-recomp / fn64-render terms
   so the *project's* vocabulary is already decoupled before the code is.
5. Native implementations (`-native`, `-wgpu`) land later against the frozen
   trait + shared test suites, A/B-diffed, swapped when at parity.

## Why adapters, not a rewrite-now

A rewrite-now stalls everything behind a from-scratch recompiler/renderer. The
adapter makes the *dependency* swappable immediately (one crate knows the fork),
lets both games keep climbing on the working fork, and gives the eventual native
build a ready-made test harness to prove itself against. Decoupled today,
rebuilt correctly on our schedule.

---

## Crate plan (2026-07-14) — what earns a crate, what doesn't

A crate marks a **swap boundary** (backend you can replace) or a **cross-tool shared type**.
Not a topic. MMIO / save / libultra are modules *inside* `fn64-runtime`, not crates.

**Exists:** fn64-runtime, fn64-abi, fn64-render (trait), fn64-render-rt64 (RT64 stub + reference
raster), fn64-shell.

**To add, prioritized:**
1. **fn64-audio** — the `AudioBackend` trait (consume AI samples → host stream) + a cpal backend +
   the RSPRecomp'd-ucode path behind it. Symmetric with fn64-render. Land the audio work AS this
   crate, not scattered into fn64-abi/runtime. (Refactor wave, on a green tree.)
2. **fn64-recomp** — the Recompiler adapter trait (see top of this doc). Overdue: N64Recomp
   shell-out still lives in aki-recomp/aki_profile. Home for fn64-recomp-native later.
3. **fn64-shell promotion** — NOT a new crate: move the common boot-host logic (load ROM → register
   sections → install rdram → run entrypoint → drive backends) out of examples/{wm2000,oot}-boot
   into fn64-shell so the examples become thin mains and the shell is a real product binary.
4. **fn64-trace** — extract the differential-trace types (thread switch / queue op / DMA / task
   submit) from fn64-runtime WHEN the A/B comparator exists (a cross-tool shared type). Not before.
5. **fn64-cpu / semantics spec** — the recomp_context + MEM/sign-extension/COP1 contract that
   translated instructions target. Extract WHEN fn64-recomp-native starts (it must emit against a
   spec, not fn64-abi's incidental layout). Not before.

**Deliberately NOT fn64 crates:** the game-profile toolchain (AKI-specific, stays in aki-recomp
until proven general — keeps fn64 game-agnostic); fn64-mmio / fn64-save (runtime modules, not
boundaries); fn64-libultra (inseparable from the executor).

**Orchestration rule learned the hard way (2026-07-14):** serialize waves that edit the same
crate's source. fn64-abi is a shared chokepoint; render + bulk-shim ran parallel on it and left
the tree non-compiling. Boot ladders and per-game profiles are naturally disjoint; shared fn64
crates are not — one wave at a time on fn64-abi.
