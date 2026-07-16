# Decision memo: wrap RT64 as fn64's render backend vs. continue the all-Rust software renderer

**Date:** 2026-07-16
**Scope:** read-only evaluation spike, no code changed.
**Verdict up front:** RT64's core is cleanly MIT and the GPL exposure is avoidable; wrapping it is the *already-documented intended design* of fn64, not a pivot. Recommendation is conditional on the goal — see §6.

---

## 0. Key finding that reframes the question

fn64 **already decided to wrap RT64.** This is not a fork in the road being discovered now; it is the plan of record that hasn't been executed yet:

- The crate is literally named `fn64-render-rt64` and its module doc says it is "the `RenderBackend` adapter crate **reserved for RT64 (MIT, C++) interop** ... the ONLY crate in the workspace permitted to contain C++ or call into RT64's C++ API." (`/Users/jer/Code/fn64/crates/fn64-render-rt64/src/lib.rs:1`)
- `docs/DESIGN.md` §1 describes `fn64-rt64` (this crate's prior name) as "FFI bridge to RT64 (C++) -- all C++ interop quarantined here" and lays out the license/language-boundary rationale in full (`/Users/jer/Code/fn64/docs/DESIGN.md:17,54-80`).
- The C++ FFI is a **named, loud stub today** (`Rt64Backend`, `lib.rs:405`): every method returns an error naming exactly what's missing. It was never claimed done.
- The all-Rust renderer in this crate — `ReferenceBackend` (`raster.rs`, 1161 lines; `gbi.rs`, 4014 lines) — is explicitly a **bootstrap / seam-proof**, described in its own doc as "the thing every future real backend (RT64 adapter, wgpu HLE) can be A/B-diffed against for seam-level correctness -- **not a claim of RDP-accurate output**."

So the honest framing is not "should we abandon the Rust renderer for RT64" but "**should we now execute the RT64 wrap that was always planned, or promote the bootstrap ReferenceBackend into the real long-term renderer.**" The combiner code in `raster.rs` even cites RT64's MIT `shared/rt64_color_combiner.h` line-by-line as its algorithm source (`raster.rs:17-24`) — the Rust path is already being written *by reading RT64*.

---

## 1. License verdict: **stays clean/permissive — with one avoidable trap**

RT64 core is unambiguously permissive:

- **RT64 itself: MIT.** (`third_party/rt64/LICENSE`, "Copyright (c) 2024 RT64 Contributors", standard MIT.)
- The `plume` GPU abstraction it links (`plume_render_interface.h`, backends `plume_vulkan.h`/`plume_d3d12.h`/`plume_metal.h`) and `re-spirv`: **MIT** (`src/contrib/plume/LICENSE`, `src/contrib/re-spirv/LICENSE`, both "Copyright (c) 2024 renderbag").
- GPU sub-deps compiled/linked into the core lib: `imgui`/`implot`/`im3d` (MIT, built in for the inspector), `hlslpp` (MIT), `VulkanMemoryAllocator` (MIT, AMD), `xxHash` (BSD-2), `zstd` (BSD — the checked-out `LICENSE` is BSD with no GPL clause pulled), `nativefiledialog-extended` (Zlib), `spirv-cross` (Apache-2.0), `stb`/`ddspp` (MIT/public-domain), `dxc` (LLVM/Apache-with-exception — and it's a **build-time shader compiler binary**, not linked into the runtime; `CMakeLists.txt:39-61`).

**The one GPL item: `mupen64plus-core` (GPLv2).** Confirmed GPLv2 (`src/contrib/mupen64plus-core/LICENSES`: "licensed under the GNU General Public License version 2"). **But it is not linked** — RT64 puts only `src/contrib/mupen64plus-core/src/api` on the *include path* (`CMakeLists.txt:421`), i.e. it consumes the mupen **plugin ABI headers** (`m64p_*` types), which are the permissive/API-descriptor part, and no RT64 source even `#include`s an `m64p` header in the current tree (grep for `m64p` in `src/` outside `contrib/` returned nothing). That include exists for RT64's *own* future emulator-plugin build, a feature the README explicitly says is "**not available in this repository yet**" (`README.md:6`).

**License bottom line:** wrapping the RT64 *core render library* keeps fn64 MIT/Apache-clean. The concrete requirement is: **build RT64 as a static lib for its render/HLE path only, and do NOT enable the (unbuilt, GPL-header-touching) mupen plugin target.** fn64's own DESIGN.md already states the whole reason the FFI is quarantined in one crate is so "a ... manual audit of 'where is this workspace not memory-safe Rust' has exactly one crate to look at" — the same quarantine cleanly contains the license-boundary audit. No GPL enters fn64's linked binary.

---

## 2. Integration surface & effort: **medium, and RT64 was built to be embedded this way**

RT64's public embedding contract is the `RT64::Application::Core` struct (`src/hle/rt64_application.h:60-92`). A host hands RT64 **exactly the register-level state a recomp already has**:

- `uint8_t *RDRAM, *DMEM, *IMEM` (fn64 already owns a single `rdram: &mut [u8]` buffer)
- The `DPC_*` RDP command registers, `VI_*` registers, `MI_INTR_REG`, and a `checkInterrupts()` callback.

The actual DL entry is `Interpreter::processDisplayLists(dlStartAddress, DisplayList*)` / `processRDPLists(...)` (`src/hle/rt64_interpreter.h:28-29`). RT64 does its **own** RSP vertex transform (compute shader), TMEM decode, combiner, blend, framebuffer tracking — it takes **raw F3DEX2 display lists + rdram**, not a pre-decoded scene. This is the good case for fn64: it hands RT64 the DL pointer from the gfx `OSTask` and the rdram buffer, and gets a presented frame back. fn64 does **not** need to decode F3DEX2 itself for RT64 (its `gbi.rs` decoder becomes unnecessary on the RT64 path).

The FFI/build glue fn64 must add (all inside `fn64-render-rt64`):

1. **Build integration:** a `build.rs` (or `cmake` crate) that compiles RT64 static + plume + the MIT sub-deps; a C++17 toolchain + a GPU dev stack (Vulkan SDK on Linux/Win, Metal on macOS) enters the build. dxc (shader compiler binary) is invoked at build time.
2. **A thin C++ shim** exposing a C ABI: `create(width,height,api)`, `process_gfx_task(rdram_ptr, dl_addr, regs...)`, `present() -> framebuffer`, `destroy()`. Wrap with `cxx` or `bindgen` per DESIGN.md §1(3).
3. **Register wiring:** fn64 has to populate the `Core` DPC/VI register pointers. fn64 already models VI (oot-boot handles `os_vi_swap_buffer`, `os_vi_get_next_framebuffer` at `main.rs:257,295,307`) and tracks `vi_swap_count()` — the register surface exists; it needs to be exposed to the shim.

**The one genuinely unresolved seam** (fn64's own docs flag it, not invented here): the **gfx task handoff signature**. `ABI-SURFACE.md`/DESIGN.md §1(2) note that no `osSpTaskLoad`/`osSpTaskStartGo` `_recomp` call site has yet been observed in either game's generated corpus, so the exact shape of "here is the gfx task" from recompiled code to the backend is not nailed down. **This blocks BOTH options equally** — the ReferenceBackend has the identical `process_task(rdram, OsTask, output_addr)` seam and the same open question. It is not an argument for staying Rust; it's orthogonal.

**Integration effort:** medium. RT64's API is explicitly designed for exactly this (the README §"This repository has been made public to provide a working implementation to native ports that wish to use RT64 as their renderer", `README.md:8`). The hard parts are toolchain/build glue and the FFI shim, not reverse-engineering RT64.

---

## 3. What fn64 GAINS: **~the entire RDP fidelity backlog, for free**

RT64 is a real, fast, GPU-accelerated, accuracy-focused RDP renderer. Wrapping it hands fn64, already-correct:

- Full **color combiner** (both cycles) — fn64 is currently hand-porting this from RT64's headers.
- **Blender / alpha blend / alpha compare**, coverage.
- **All N64 texture formats + TMEM** — RT64 has "one of the most accurate TMEM loaders to date ... directly reverse engineered by observing console behavior" with homebrew test ROMs (`README.md`), plus Tharo's color-conversion research. This is a large, subtle body of work fn64 has barely started.
- **Perspective-correct texturing, scissor, TEXRECT, fog, z-buffer, mipmap/filter/clamp-wrap**, N64 3-point filtering.
- **Framebuffer effects / framebuffer-to-texture detection** (RT64's deferred-RDP framebuffer manager) — needed for a lot of "faithful" N64 rendering fn64's scanline rasterizer doesn't approach.
- Free upside fn64 would otherwise never build: high-res, widescreen, texture packs, interpolation, and a path to path-tracing.

**Rough fidelity fraction for "faithful OoT frame":** the fn64 ReferenceBackend today does flat/shade-triangle rasterization + a partial combiner into an RGBA5551 framebuffer (`write_rgba5551_framebuffer`, `lib.rs`), enough to prove the seam and make non-blank pixels — call it well under ~15% of a faithful frame. RT64 delivers essentially **~95%+** of a faithful OoT render out of the box (OoT is one of the most-exercised titles in the N64-recomp ecosystem RT64 comes from). The remaining fraction is integration correctness (feeding it the right rdram/regs), not renderer fidelity.

---

## 4. What fn64 LOSES / costs

- **All-Rust type safety & memory safety on the render path.** fn64's runtime/scheduler stays pure safe Rust; but ~30,744 lines of RT64 C++ (core, excluding contrib) enter the linked binary behind an FFI boundary. DESIGN.md already accepts this and quarantines it to one crate.
- **Build & CI weight.** Today `cargo test -p fn64-runtime -p fn64-abi` is pure-Rust and fast (DESIGN.md §1(3) calls this out as a deliberate property). The RT64 path requires a C++17 toolchain + GPU SDK; **headless CI needs a GPU or a software Vulkan (lavapipe/SwiftShader)** to render — a real CI cost. Mitigation is already designed in: only `fn64-shell`/examples pull in `fn64-render-rt64`; runtime/abi crates stay pure and testable without it. The ReferenceBackend can also stay as the CI-friendly headless seam-test backend even after RT64 is the product renderer.
- **Portability.** Pure-Rust software raster runs anywhere (incl. wasm, deterministic golden-image tests). RT64 needs Vulkan/D3D12/Metal — no trivial wasm path, and platform-specific GPU behavior enters the picture.
- **The "we built it from specs" clean-room story — mild concern, not a blocker.** RT64 is HLE "directly reverse engineered by observing console behavior" (`README.md`). **This is different in kind from the matching-decompilations this project rejects:** RT64 is not a decompilation of copyrighted game code — it's an independently-authored, MIT-licensed renderer that studied *hardware output* (like every N64 graphics plugin ever). Using it is like using any MIT dependency; it does not taint fn64's own from-ROM-bytes provenance for the *game* code, which is the provenance that actually matters here. Provenance verdict: **fine.** (fn64's own combiner code already reads RT64 as reference, so this boundary is already accepted in practice.)

---

## 5. Effort estimate & crossover

- **Wrap RT64 to first faithful OoT frame:** dominated by (a) build/toolchain glue + FFI shim, (b) wiring fn64's rdram/VI/DPC registers into RT64's `Core`, (c) resolving the shared gfx-task-handoff seam. Once those land, fidelity is *already there*. Order of magnitude: a focused integration effort, not a renderer-authoring effort. Most risk is in build/CI plumbing and the one open ABI seam.
- **Continue all-Rust to the same faithful-OoT bar:** re-implementing accurate TMEM (all formats), full blender/coverage, perspective-correct + filtering, framebuffer-to-texture effects, scissor/TEXRECT edge cases — i.e. re-deriving most of what RT64's ~30k lines already do correctly, verified only by the user's eyes per this project's visual contract. This is a **large, open-ended, multi-wave** effort (the combiner alone is being ground out now).
- **Crossover:** the all-Rust path only wins on total effort if the fidelity bar stays low (seam-proof / structural correctness / deterministic golden images) OR if a GPU dependency is categorically unacceptable. For anything approaching *faithful playable OoT*, RT64 is far less total work.

---

## 6. Recommendation

**Wrap RT64 — it is the plan of record and the right one, *unless* an all-Rust/no-GPU constraint is a hard product requirement.**

- **If the goal is a faithful, playable render ASAP (and it is — CLAUDE.md's "faithful outcome not impl" memory says the target is the render *outcome* vs. the emulator, not bit-emulating the pipeline): WRAP RT64.** It hands fn64 ~95% of a faithful OoT frame immediately, keeps the binary MIT/permissive (just don't enable the GPL mupen plugin target), and matches the crate's stated purpose. Keep `ReferenceBackend` as the pure-Rust, headless, CI/seam-test backend and A/B oracle — don't delete it. Execute the FFI wrap now; the only real shared blocker (gfx-task handoff signature) blocks the Rust path identically, so it's not a reason to wait.

- **Stay all-Rust ONLY if** a hard requirement forbids a C++/GPU dependency in the shipped runtime (e.g. a wasm target, a "zero C++ in the binary" mandate, or CI that categorically cannot host a GPU/software-Vulkan). Then accept that reaching faithful-OoT fidelity is a large, eyes-only, multi-wave renderer-authoring project, and scope the visual bar accordingly.

**Concrete tradeoff in one line:** RT64 trades ~30k lines of quarantined MIT C++ + a GPU/C++ build dependency for essentially the entire RDP-fidelity backlog done correctly; the all-Rust path trades a slow, eyes-verified re-derivation of that backlog for a pure-safe-Rust, GPU-free, portable renderer. Given this project's stated "faithful outcome, not pipeline-emulation" goal, **the C++/GPU cost is worth paying — wrap RT64.**

---

### Cited files
- RT64 license: `/Users/jer/Code/no-mercy-recompiled/third_party/rt64/LICENSE` (MIT)
- GPL sub-dep (headers-only, not linked): `.../third_party/rt64/src/contrib/mupen64plus-core/LICENSES` (GPLv2); include-only at `.../rt64/CMakeLists.txt:421`
- MIT GPU stack: `.../rt64/src/contrib/plume/LICENSE`, `.../re-spirv/LICENSE`; VMA `.../plume/contrib/VulkanMemoryAllocator/LICENSE.txt`
- RT64 embedding API: `.../rt64/src/hle/rt64_application.h:60-92` (Core struct), `.../rt64/src/hle/rt64_interpreter.h:28-29` (processDisplayLists)
- RT64 provenance / "made public for native ports": `.../rt64/README.md:6-8`, architecture §Deferred RDP/RSP/TMEM
- fn64 render crate + stub: `/Users/jer/Code/fn64/crates/fn64-render-rt64/src/lib.rs` (module doc, `Rt64Backend` @ 405, `ReferenceBackend`, `write_rgba5551_framebuffer`); `raster.rs:17-24` (cites RT64 combiner header)
- fn64 render seam: `/Users/jer/Code/fn64/crates/fn64-render/src/lib.rs:196` (RenderBackend trait); `/Users/jer/Code/fn64/crates/fn64-abi/src/lib.rs:1900` (dispatch_gfx_task)
- fn64 design intent: `/Users/jer/Code/fn64/docs/DESIGN.md:17,54-80` (RT64 = the planned backend, C++ quarantine, gfx-task seam open)
- fn64 present path: `/Users/jer/Code/fn64/examples/oot-boot/src/main.rs:257,295,307,481-501`
