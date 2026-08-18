# WM2000 through the Rust port: the measured gap

What separates `fn64-render-wgpu`'s `WgpuBackend` from rendering WM2000 (WWF
WrestleMania 2000, the AKI wrestling lead ROM) in the real shell. Every claim
below is cited to a file and line in this checkout; where a quantity could not
be measured it says UNKNOWN rather than estimating.

Measured at `53525d0e`. Companion docs:
[`RENDER-WGPU-PORT-PLAN.md`](RENDER-WGPU-PORT-PLAN.md) (the port's own slice
plan), [`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never
assert"), [`RT64-GAP-REGISTER.md`](RT64-GAP-REGISTER.md).

---

## 1. Verdict: the raw-DPC seam cannot render WM2000, and the shell never arms it

This is the load-bearing finding and it is structural, not a matter of missing
opcodes.

**WM2000 renders through the gfx-task path.** The recompiled guest calls
libultra's `osSpTaskStartGo`, whose shim
(`crates/fn64-abi/src/task_dispatch/lifecycle.rs:1711`) classifies the task at
`lifecycle.rs:1717` (`header.task_type == M_GFXTASK`, and `M_GFXTASK = 1` at
`crates/fn64-render/src/lib.rs:133`) and routes it into
`dispatch_gfx_task_chunk`
(`crates/fn64-abi/src/task_dispatch/setup.rs:442`), which calls
`backend.process_task_chunk(..)` at `setup.rs:461`. That is the sole non-test
production entry point for a graphics task. `process_task_chunk`'s default body
delegates its `Start` step straight to `process_task`
(`crates/fn64-render/src/lib.rs:1596-1597`).

Independent corroboration that this is WM2000's real path: the preserved crash
log from a live WM2000 run at
`/Users/jer/Code/wm2000-run/last-run.log` shows `_osSpTaskStartGo_recomp` on
the stack directly above `_func_80001024`, i.e. the guest reaching the
gfx-task shim in production.

**`WgpuBackend` refuses that path.** `process_task`
(`crates/fn64-render-wgpu/src/production.rs:1203`) and `present` (`:1218`)
both return `Err(RenderError::Backend { .. "is out of scope" })`. It does not
override `process_rdp_commands` either, so it inherits the trait default's
refusal at `crates/fn64-render/src/lib.rs:1634-1655`.

**The raw-DPC seam is opt-in and nothing outside tests opts in.** Both
`try_dispatch_raw_dpc_via_session`
(`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:956`) and its caller
`dispatch_dpc_submission` (`:1425`) read a thread-local first:

- `rsp_commit.rs:961-964` — `let registered = RAW_DPC_SESSION.with(..);
  if !registered { return None; }`
- `rsp_commit.rs:1436-1437` — the same check, before any work.

When no session is registered, control falls through to the *legacy* branch at
`rsp_commit.rs:1517` onward, which calls `process_rdp_commands`
(`rsp_commit.rs:1548`) — a different trait method again, and one `WgpuBackend`
also refuses. The seam's own doc comment states the fallback is deliberate
(`rsp_commit.rs:923-928`: "Returns `None` (never partially attempted) when no
`RawDpcAbiSession` is registered, so callers fall back to the legacy atomic
`process_rdp_commands` path unconditionally").

The only non-definition caller of `set_raw_dpc_session`
(`crates/fn64-abi/src/task_dispatch/lifecycle.rs:1077`) is
`crates/fn64-abi/src/task_dispatch/tests/raw_dpc_session_integration.rs`
(`:93`, `:654`, `:784`, `:1088`). `crates/fn64-shell` never calls it.

**The shell cannot even select the backend.** `crates/fn64-shell/Cargo.toml:28-30`
declares `fn64-render`, `fn64-render-reference`, `fn64-render-rt64` — there is
no `fn64-render-wgpu` dependency. `FN64_RENDER`
(`crates/fn64-shell/src/main.rs:374-376`) chooses only between
`Rt64Backend` (`:380`) and `ReferenceBackend` (`:396-404`).

> **Verdict.** Raw-DPC and gfx-task are two independent, mutually exclusive
> routing branches. WM2000 uses the gfx-task branch. The raw-DPC IR seam —
> however thoroughly tested — is not on WM2000's path today and cannot render
> it without either (a) `WgpuBackend` implementing `process_task`, or (b) a new
> shell-side component that registers a `RawDpcAbiSession` *and* an RSP-side
> producer that converts WM2000's display lists into raw DPC command streams.
> Neither exists.

### 1a. `present` is not the blocker

Worth stating because it is easy to mis-scope. The shell does not call
`RenderBackend::present` to get pixels on screen. `Shell::present`
(`crates/fn64-shell/src/main.rs:574`) reads the VI framebuffer straight out of
guest RDRAM — `fn64_abi::current_vi_framebuffer()` at `main.rs:579`, the RDRAM
slice at `main.rs:594-600`, `rgba5551_to_rgba8888` at `main.rs:625-631`, and the
window blit at `main.rs:637`.

So the contract a backend must satisfy is not "implement `present`" but
"leave correct bytes in `rdram[output_addr..]`", exactly as
`crates/fn64-render/src/lib.rs:1567-1569` states ("A backend that renders into
its own private surface must copy the result into `rdram[output_addr..]` ... so
the VI-presented frame is not blank"). `ReferenceBackend` honors this via
`finish_reference_task` (`crates/fn64-render-reference/src/backend/render_backend.rs:91`).

`WgpuBackend` does not honor it on any path — see §3.

---

## 2. The command census: not reachable, and exactly what is needed

**No WM2000 display-list or RDP-command capture exists in this checkout or on
this machine.** Rendered *frames* do exist (see below) — what is absent is any
record of the commands that produced them. This is a measured absence, not an
assumption:

- No file in the repository matches `*wm2000*` (case-insensitive) outside
  generic corpus/build plumbing; no `xbus-*.bin` exists anywhere under
  `/Users/jer`, `/tmp`, or `/private/tmp`.
- `crates/fn64-render-conformance`'s own `README.md` states "No such RT64 or
  Rust-port runner is registered yet, so every backend row stays open," and the
  crate contains zero WM2000-derived fixtures — every `ConformanceFixture` in
  `src/lib.rs` is a hand-built synthetic vector.
- `reference/wm2000-frames/` is `.gitignore`d by design (AGENTS.md's "No game
  content ... ever enters git") and does not exist in this worktree. Even when
  populated it holds rendered PNGs, not command streams.

**The capture mechanism does exist.** `dispatch_captured_raw_rdp`
(`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1660`) dumps big-endian
command words when `FN64_XBUS_STREAM_DUMP_DIR` is set, and
`crates/fn64-render-reference/examples/xbus_replay.rs` replays those
`xbus-NNNN.bin` files through `ReferenceBackend`. The env vars are
`FN64_XBUS_STREAM_DUMP_DIR`, `FN64_XBUS_STREAM_DUMP_SKIP`,
`FN64_XBUS_STREAM_DUMP_RDRAM`, and `FN64_XBUS_REPLAY_REPEAT`. The WM2000 ROM
path variable is `FN64_WM2000_ROM`
(`crates/fn64-audio/tests/rsp_predecode_equivalence.rs:74`).

**What blocks a fresh capture today.** Less than expected, and the blocker is
not the ROM. `/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64` exists and is
byte-identical to `/Users/jer/Downloads/WWF WrestleMania 2000 (USA).z64`
(verified with `cmp`; both files' SHA-1 also matches the `rom_sha1` recorded at
`aki-recomp/games/NWXE/profile.toml:14`, which is the authority for that
digest -- this doc deliberately does not restate the hash, since no test here
gates it).

The standalone runner at `/Users/jer/Code/wm2000-run/run.sh` is pinned to an
fn64 checkout 291 commits behind current HEAD, and its recorded run aborted at
`RSP task exceeded deterministic 67108864-instruction admission bound at PC
0x1128`. That bound is `MAX_TASK_STEPS` and the identical `panic!` still exists
in current fn64 at `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:234-237`,
so being newer does not by itself fix it. Whether current HEAD (which has
gained a `wm2000-block-boot` harness the stale tree lacks) clears that bound is
UNKNOWN without running it.

The recompile is a deliberate partial: 51 `.c` files carrying 2,449
`RECOMP_FUNC` definitions, covering the resident image only (ROM
`0x1000`-`0x4C0C0`). `aki-recomp/games/NWXE/wm2000.toml` states the deferred
code and overlay bank "are NOT part of this resident linear map and are out of
scope until loadmap.json resolves their real vram," and
`aki-recomp/games/NWXE/docs/loadmap-notes.md` records that no `loadmap.json`
exists.

**WM2000 demonstrably renders today — through RT64, not through triangles.**
This is the most consequential thing the preserved evidence shows, and it
sharpens the whole gap analysis. `/Users/jer/Code/wm2000-run/artifacts/match4/`
holds 10,045 captured PNG frames, 9,937 of them hash-distinct; named milestone
captures walk the game from the title screen through menu flow to a fully
textured "Steve Austin vs Mankind" VS screen with 3D wrestler models. So the
gfx-task path plus `Rt64Backend` produces real WM2000 output.

But `last-run.log` lines 55-67 record every one of the first five gfx tasks as
`NON-CLEAR (0 tris)`. Those early frames are composed **without triangles** —
consistent with menu/title content built from rectangles and texture blits
rather than geometry. This is a measured observation about five specific early
tasks, not a claim about gameplay frames, whose triangle counts are UNKNOWN
because no per-frame statistics were persisted to disk.


> Note the second-order problem: even a successful XBUS capture would measure
> the *raw-DPC* stream, and §1 establishes WM2000 does not render through
> raw-DPC. The census that actually matters for `process_task` is a **GBI
> display-list** census, and no tooling in `tools/` or `scripts/` produces an
> RDP or GBI opcode histogram at all (searched "census"/"histogram"/"opcode"
> across both; the only hits are a memcpy *cost* census in
> `crates/fn64-abi/src/dpc_copy_census.rs`, a clip-w histogram in
> `crates/fn64-render-reference/src/gbi/projdump.rs:5`, and a MIPS-decode
> histogram in `crates/fn64-cpu-runtime/tests/corpus_decode_sweep.rs`).
> Building that instrumentation is itself a prerequisite task — see §5.

**Which ucode WM2000 uses: UNKNOWN in this checkout.** No file here records it,
and no capture was reachable to derive it.

`supported_ucodes()` returning `&[]`
(`crates/fn64-render-wgpu/src/production.rs:1299`) is nonetheless a *real*
blocker rather than an inapplicable seam, because §1 establishes WM2000 travels
the gfx-task path, and that path is precisely where ucode selection applies
(`crates/fn64-render/src/lib.rs:2071` gates on `self.ucodes.contains(..)`). On
the raw-DPC path the empty list would indeed be irrelevant — but WM2000 is not
on it.

---

## 3. What the raw-DPC seam admits today, measured from the decoder

Notwithstanding §1, the admitted set is worth recording precisely, because it is
the honest inventory of what the port has actually built and because the
standing brief's summary of it is now stale in two directions.

The decoder is `decode_stream`'s opcode match at
`crates/fn64-render-wgpu/src/raw_dpc/mod.rs:1066-1310`; the constants are at
`raw_dpc/mod.rs:51-86` and `crates/fn64-render-wgpu/src/tmem/wire.rs:25-31`.
The width table gating it all is
`crates/fn64-render-ir/src/command.rs:813-828`.

| Opcode | Command | Admitted | Where |
|---|---|---|---|
| `0x00`–`0x07` | NoOp | yes | `raw_dpc/mod.rs:1067` |
| `0x08`–`0x0f` | RawTriangle (all 8 shade/tex/zbuf variants) | yes | `raw_dpc/mod.rs:1288-1298` |
| `0x24` / `0x25` | TextureRectangle / Flip | yes | `raw_dpc/mod.rs:1299-1310` |
| `0x26` | LoadSync | yes | `tmem/wire.rs:25` |
| `0x27` | **SyncPipe** | **no** | falls to `_ =>` at `raw_dpc/mod.rs:1311` |
| `0x28` | **SyncTile** | **no** | same |
| `0x29` | FullSync (site only) | yes | `raw_dpc/mod.rs:1262-1287` |
| `0x2a` / `0x2b` | **SetKeyGB / SetKeyR** | **no** | same |
| `0x2c` | **SetConvert** | **no** | same |
| `0x2d` | SetScissor (tracked state only, no clip applied) | partial | `raw_dpc/mod.rs:1143-1157` |
| `0x2e` | SetPrimDepth | yes | `raw_dpc/mod.rs:1158` |
| `0x2f` | SetOtherMode | yes | `raw_dpc/mod.rs:1069` |
| `0x30` | LoadTLUT | yes | `tmem/wire.rs:26` |
| `0x32`–`0x35` | SetTileSize / LoadBlock / LoadTile / SetTile | yes | `tmem/wire.rs:27-30` |
| `0x36` | FillRectangle | yes | `raw_dpc/mod.rs:1176` |
| `0x37` | SetFillColor | yes | `raw_dpc/mod.rs:1114` |
| `0x38`–`0x3c` | SetFogColor / BlendColor / PrimColor / EnvColor / Combine | yes | `raw_dpc/mod.rs:1120-1174` |
| `0x3d` | SetTextureImage | yes | `tmem/wire.rs:31` |
| `0x3e` | **SetZImage** | **no** | asserted rejected, `raw_dpc/mod.rs:1902` |
| `0x3f` | SetColorImage | yes | `raw_dpc/mod.rs:1074` |

Two corrections to the standing summary, both in the port's favor:

1. **Triangles are admitted and genuinely wired**, not present-but-unwired like
   the 48 inert `rt64_*` modules. The chain is
   `execute_raw_dpc` → `draw_admitted_triangles`
   (`crates/fn64-render-wgpu/src/production.rs:1351`) →
   `TrianglePipelineRenderer::submit_triangles`
   (`production.rs:458`) → real GPU submission. `TextureRectangle` is
   admitted by expansion into two triangles (`TriangleSource::TextureRectangle`,
   `crates/fn64-render/src/render_ir.rs:1596`). Depth testing is real
   fixed-function GPU state, four pipeline variants selected per draw from
   `OtherMode` (`crates/fn64-render-wgpu/src/targets/triangle_pipeline.rs:38-46`).
2. The capability enum value `TransactionalTmemFillFullSyncSiteOnly`
   (`production.rs:1316`) therefore *understates* the decoder. The enum is a
   coarse label, not the admitted set.

And one correction against it: the dead-code signal measured here is **1041**
`never used` warnings from `cargo build -p fn64-render-wgpu` (ANSI-stripped),
not the ~1198 the brief quoted. Baseline measured, not quoted.

### 3a. Three structural restrictions matter more than the missing opcodes

- **Triangles produce no guest-visible bytes.** `draw_admitted_triangles`
  stores its result in `self.triangle_draw_output`
  (`production.rs:474`), reachable only through `last_triangle_draw()`
  (`production.rs:298`) — which has **zero consumers outside
  `production.rs` and the crate README**. The crate's own README calls it
  exactly that: "no guest-visible publication beyond this backend's own
  existing `last_triangle_draw()` diagnostic accessor"
  (`crates/fn64-render-wgpu/README.md:2115-2116`). Meanwhile
  `staged_guest_render_target_writes` (`production.rs:1377-1391`) returns
  writes *only* from `pending_fill_publication`. A frame of triangles produces
  an empty write list.
- **Fill and triangles are mutually exclusive within one packet**, rejected
  loudly as `MixedFillAndTriangles` — "the two run entirely disjoint render
  paths ... Composing the two sources is a follow-on slice"
  (`production.rs:991-1000`).
- **TMEM loads and fill are also mutually exclusive**, rejected as
  `MixedFillAndTmemLoadPacket` (`production.rs:984-990`).

A real game frame is textures **and** triangles **and** a background fill in one
stream. All three combinations are refused.

---

## 4. Ranked: what `WgpuBackend` must gain, hardest constraint first

Ordered by whether the item blocks a *recognizable frame*, not by line count.
Line estimates are given only where an analogous in-tree implementation bounds
them; elsewhere the entry says UNKNOWN, per AGENTS.md.

| # | Gap | Why it is essential | Evidence | Size |
|---|---|---|---|---|
| 1 | **A `process_task` implementation, or a shell-side raw-DPC session + display-list-to-DPC producer** | Without one of these `WgpuBackend` is never called at all on WM2000's path. Everything below is unreachable until this lands. | §1 | UNKNOWN — `ReferenceBackend`'s GBI front end (`crates/fn64-render-reference/src/gbi/`) is the scale reference and is several thousand lines |
| 2 | **RDRAM copyback for triangle output** | The shell reads pixels only from `rdram[output_addr..]` (§1a). Today triangles land in a diagnostic accessor with zero consumers. Without this, a perfectly rasterized frame is invisible. | `production.rs:298`, `:474`, `:1377-1391`; `README.md:2115-2116`; `crates/fn64-render/src/lib.rs:1567-1569` | Bounded — `finish_reference_task` (`render_backend.rs:91`) is the working precedent |
| 3 | **Compose fill + triangles + TMEM in one packet** | Every real frame is a background clear followed by textured geometry. Both combinations are hard-refused today. | `production.rs:984-1000` | UNKNOWN; the refusal comments call it "a follow-on slice" |
| 4 | **`SetZImage` (`0x3e`) and depth-buffer resolution** | A wrestling game is z-buffered 3D — two wrestlers, a ring, a crowd, all interpenetrating. `0x3e` is asserted-rejected. The GPU depth *test* already exists (`triangle_pipeline.rs:38-46`), so this is binding a real depth image, not building depth from scratch. | `raw_dpc/mod.rs:1902`; `crates/fn64-render-reference/src/gbi/types.rs:1762` (`SetDepthImage` is a first-class reference `RenderOp`) | Moderate; depth test already built |
| 5 | **`SetScissor` (`0x2d`) actually applied** | Admitted as tracked state only, "no draw" uses it (`crates/fn64-render/src/render_ir.rs:1440-1442`). Unclipped draws bleed outside the viewport. | `raw_dpc/mod.rs:1143-1157` | Small |
| 6 | **`SyncPipe` (`0x27`) / `SyncTile` (`0x28`)** | Emitted constantly by real display lists. The reference treats them as a no-op group (`crates/fn64-render-reference/src/gbi/stream.rs:1099`), so admitting them is near-free — but rejecting them kills the stream. | `raw_dpc/mod.rs:1311` | Trivial |
| 7 | **`supported_ucodes()`** returning a real list | Gates the gfx-task path (`crates/fn64-render/src/lib.rs:2071`). Cannot be filled until §2's ucode question is answered. | `production.rs:1291` | Trivial once known |
| 8 | **`SetConvert` (`0x2c`), `SetKeyR/GB` (`0x2a`/`0x2b`)** | YUV conversion and chroma key. Genuinely optional for a first frame. | `raw_dpc/mod.rs:1311` | Low priority |

Items 4–8 are the *cheap* half. Items 1–3 are the project.

---

## 5. Honest ordering: what to build first

**The first thing to build is not renderer code.** It is the measurement §2
could not take — and §2's evidence shows that measurement is closer to hand
than it looked. Two prerequisites, in order:

1. **Re-point the WM2000 runner at current HEAD and run it.** The ROM is
   verified byte-identical and present; the recompile boots and previously
   produced ~10,000 distinct frames. The only unknown is whether the
   `MAX_TASK_STEPS` abort still fires on current fn64. This is a build and a
   run, not a rebuild of the pipeline, and it should be attempted before any
   larger scoping decision.
2. **Add a GBI/RDP opcode census** to `ReferenceBackend`'s task walk — it
   already decodes every command into `RenderOp`
   (`crates/fn64-render-reference/src/gbi/types.rs:1757-1767`), so a counter
   per opcode is a small, honest addition that answers §2's census, §2's ucode
   question, and §4 item 7 in one run. The `NON-CLEAR (0 tris)` lines already
   in `last-run.log` show the harness has a place to hang such counters.

Only then does the ordering below become actionable.

**First visible milestone: a correct WM2000 background fill in the real shell.**
Not a textured triangle. Two reasons, and the second is new evidence.

First, the fill path is the *only* path in `WgpuBackend` that already produces
a guest-visible write (`pending_fill_publication` →
`staged_guest_render_target_writes`), so it is the shortest route to proving
the whole chain — shell → `fn64-abi` → `WgpuBackend` → RDRAM → VI → window —
carries real bytes end to end. A textured triangle is the more impressive demo
and the more finished subsystem, but it terminates in a diagnostic accessor
with no consumers, so shipping it first would prove a subsystem rather than a
pipeline.

Second, §2's `NON-CLEAR (0 tris)` observation suggests WM2000's early screens
may be composed largely from rectangles and texture blits rather than geometry.
If a census confirms that, the rectangle/fill path is not merely the cheapest
milestone — it is a substantial fraction of the actual early-frame workload,
and `TextureRectangle` (already admitted, already expanded to two triangles at
`crates/fn64-render/src/render_ir.rs:1596`) becomes the highest-value next
step rather than a sideshow. That reordering hinges entirely on the census, so
do not act on it before §5's step 2.

Concretely, the shortest honest path to that milestone is:

1. Add `fn64-render-wgpu` to `crates/fn64-shell/Cargo.toml` and a
   `FN64_RENDER=wgpu` arm at `crates/fn64-shell/src/main.rs:374-404`.
2. Implement a minimal `process_task` on `WgpuBackend` that handles only the
   background-clear prefix of WM2000's display list and copies back to
   `rdram[output_addr..]` (items 1 and 2 of §4, in their narrowest form).
3. Then item 3 (composition), then item 4 (depth). Item 2's copyback is the
   piece that converts every subsequent triangle-side win into something a
   human can see, so it should not be deferred behind item 3.

---

## 6. Scope honesty

**Is "WM2000 renders through the Rust port" weeks or months?** On the evidence
here: **months**, and the dominant cost is not the renderer.

The dominant cost is item 1 of §4 — a display-list front end. `WgpuBackend`
today implements a *raw-DPC command executor*: it consumes already-decoded RDP
commands. What WM2000 hands the system is a *GBI display list*, which must be
walked, its matrix/vertex/lighting/texture state maintained, and its geometry
transformed into RDP triangles. That is the entire body of work
`crates/fn64-render-reference/src/gbi/` represents, and none of it exists in
`fn64-render-wgpu`. The 1041 dead-code warnings in that crate are consistent
with a large body of ported-but-unreferenced material, matching the previously
recorded finding that ported `rt64_*` modules are inert.

**Is the goal reachable on the current architecture?** Yes — the architecture
is not the obstacle. `RenderBackend` is a clean trait seam; `ReferenceBackend`
and `Rt64Backend` both satisfy it; a third implementation is a normal amount of
work, not an architectural fight. The triangle pipeline, the TMEM sampler, the
combiner, and the depth variants are real, tested, GPU-executing code, and they
are the parts that are usually hardest.

One thing cuts the other way and should be weighed. §2 establishes that
WM2000 already renders real, recognizable frames through the gfx-task path with
`Rt64Backend`. So the *system* around the renderer — boot, recompile, task
dispatch, VI, RDRAM presentation, input, audio — works for this title. The
missing piece is genuinely one backend implementation, not an emulator. That is
what keeps the estimate at months rather than longer, and it is why the
architecture verdict above is "yes."

Two things should be flagged rather than glossed:

- **The raw-DPC IR seam is a large, well-tested subsystem that is not on
  WM2000's path.** It is not wasted — it is the executor a future
  `process_task` would feed — but the project should stop treating progress on
  it as progress toward the stated goal. It is upstream of the goal by one
  entire missing layer.
- **No number in this document is a WM2000 measurement.** Every command-set
  fact is measured from fn64's own decoder; the WM2000 side is inference from
  the genre and from the reference backend's own `RenderOp` set. The census
  remains UNKNOWN until §5's prerequisites land, and any scoping decision that
  needs a count should wait for it rather than quote this document.
