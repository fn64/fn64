# WM2000 frame 0, three ways: does an independent lineage agree?

[`RT64-WM2000-VALIDATION.md`](RT64-WM2000-VALIDATION.md) §1 reports **0 of
115,200 pixels differ** between `WgpuBackend` and `fn64-render-reference` on
WM2000's real captured frame-0 packet, once `alpha_dither` is controlled to
`Disabled`.

That result has a weakness its own author named, and this card exists to
address it: **`fn64-render-reference` is an independent *implementation*, not
an independent *authority*.** Both it and the wgpu port derive from public SGI
documentation and from this project's reading of it. Their agreement proves
internal consistency. It does not prove fidelity to silicon, because a
misreading shared by both lanes agrees with itself.

RT64 is described in this repo as the eyes-verified faithful renderer and is a
genuinely separate lineage — separate authors, separate source tree, a real GPU
pipeline rather than a CPU model. So the question this card asks is narrow and
answerable: **does RT64 agree too?**

It does. All three pairings are exact.

Companion docs: [`RT64-WM2000-VALIDATION.md`](RT64-WM2000-VALIDATION.md),
[`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md),
[`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-PORT-AUTHORITY.md`](RT64-PORT-AUTHORITY.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never
assert").

---

## 1. Does the RT64 adapter build and run on this machine?

**Yes — it builds, and it renders live on this machine's GPU.**

Prior circulating belief held that RT64 could not be exercised here. That is
false on this host, and the evidence is a rendered image rather than a
successful link:

- `cargo build -p fn64-render-rt64 --features rt64` succeeds.
- The existing `rt64_pixel_differential` example initialises RT64's native
  Metal device and prints `Device Name: Apple M5 Pro`, `Device Vendor:
  0x106B`, then produces real non-black pixels through RT64's own pipeline.

### 1.1 The one thing an operator must set

The build's default RT64 location is resolved **relative to the crate
manifest**:

```
crates/fn64-render-rt64/build.rs:273
    let default_rt64 = manifest_dir.join("../../../no-mercy-recompiled/third_party/rt64");
```

That default only resolves for a checkout sitting beside
`no-mercy-recompiled`. From a worktree under `/private/tmp` it does not
resolve at all, and the build fails with the correct, named refusal:

```
RT64 source checkout not found (No such file or directory (os error 2));
set FN64_RT64_DIR to its MIT source tree
```

**This is a worktree-location fact, not a missing dependency.** The fix is to
export the path the panic already names:

```
FN64_RT64_DIR=/Users/jer/Code/no-mercy-recompiled/third_party/rt64
```

That checkout is at `f0728a2520d5aa735886240de3fee75cc805f6d6` — the
`oracle` pin — and is clean, which is what `build.rs`'s
`rt64_source_identity` requires. **Neither authority pin was touched.** The
two pins in [`rt64-port-authority.json`](rt64-port-authority.json) are correct
by design (`RT64-PORT-CARD-BRIEF.md` §2); this card moved nothing and
"fixed" nothing to make a build succeed.

### 1.2 Wall-clock cost

| Step | Wall clock |
|---|---|
| `cargo build -p fn64-render-rt64 --features rt64` | **36 s** |
| One `rt64_wm2000_three_way` run (three backends, warm) | ~50 s |

The 36 s figure is a **warm** CMake configure/build: the RT64 native core had
already been compiled in this checkout's `target/` by an earlier step in this
session. A cold first build compiles RT64's C++ core, HLE target and the
crate-local shim from scratch and costs substantially more. Anyone re-running
this from a fresh `target/` should budget for a full native build, not 36 s.

---

## 2. The three-way comparison

### 2.1 The controlled variable, stated so it is not read as tuning

`alpha_dither` is rewritten to `Disabled` (encoding 3) in the **one shared
word stream** handed to **all three** backends, exactly as the existing
two-way comparison does.

This is a control, not a tuning:

- The rewrite is applied once, to the packet, *before* any backend sees it.
  All three decode identical bytes. No backend is configured differently from
  another.
- It touches only other-mode-high bits 4:5. The example asserts that every
  other bit of every word is unchanged, so the controlled packet is the same
  packet with one variable held, not a different packet.
- The mode being removed is `Noise`, whose oracle implementation is a
  SplitMix64 stream that self-disclaims as non-silicon. Requiring any
  implementation to match an invented sequence would measure transcription,
  not fidelity.
- The control is verified by an instrument independent of the code it
  controls: the example re-decodes bits 4:5 off the wire by hand and asserts
  they read `3`, rather than asking a backend's own accessor.

**No implementation was modified.** No refusal was weakened. The three
backends are stock.

### 2.2 The table

Captured WM2000 decode entry 0 — 366 commands, 60 `G_FILLRECT`, 60
`G_TEXRECT`, 0 triangles (census facts asserted against the fixture by the
example itself, so a synthetic stand-in cannot masquerade as the capture).
Target 480x240, color image `0x0038f800`, 115,200 pixels.

| Pair | Differing pixels |
|---|---|
| port (`fn64-render-wgpu`) vs reference (`fn64-render-reference`) | **0 of 115,200** |
| port vs **RT64** (`fn64-render-rt64`) | **0 of 115,200** |
| reference vs **RT64** | **0 of 115,200** |

All three publish the identical histogram:

| Halfword | Count | What it is |
|---|---|---|
| `0xdef7` | 114,481 | the texrects' flat `Primitive` blend |
| `0x0001` | 719 | the fill pixels |

The `0xdef7` value is hand-derived rather than read off any implementation:
the texrects' combiner is flat `Primitive` with prim alpha `0xdf` = 223 over a
zero destination, so the composite is `255 * 223/255 = 223`, whose five-bit
channel is `223 >> 3 = 27`, giving `0xdef7`.

### 2.3 Disagreements

**There are none.** Every pairing is exact, so there is no disagreement to
diagnose and no side to adjudicate.

This is worth stating plainly because the card that commissioned this work
expected a disagreement to be the more valuable outcome — the shape of the
`setPrimDepth`, VI-scale and S2DEX `lrs` findings already recorded in
`docs/`. It did not happen here. On this packet, in this configuration, the
three lineages are pixel-identical.

The scope of that agreement is bounded in §4.

---

## 3. Why the comparison is believed

Agreement is the weakest possible signal if the instrument cannot tell images
apart, or if the fixture is not the real capture, or if the backends are all
declining to draw. Each of those is closed by a control that predicts its
number **before** measuring it.

### 3.1 Determinism

**10 consecutive runs, byte-identical output** (one distinct digest across ten
captured run logs). RT64's native Metal path is included in those ten; it did
not drift.

### 3.2 The comparison discriminates

A `+1` five-bit step on the port's red channel is predicted to move **every**
pixel, and does: `0xdef7`'s red field is 27 → 28, and `0x0001`'s is 0 → 1, so
neither surviving value is a fixed point.

| Mutant | Predicted | Measured |
|---|---|---|
| port `+1` red step, vs unmutated port | 115,200 | **115,200** |
| the same mutant vs the reference | 115,200 | **115,200** |

### 3.3 Positive control: all three actually rasterize this packet

Three backends that all leave the target untouched would also "agree". So the
packet's **own** `SetPrimColor` red byte is rewritten and each backend re-run.

Hand-derived prediction, stated before measurement: the texrect pixels are the
prim colour and must move; the 719 fill pixels come from `SetFillColor` and
must not. Expected **114,481** — not "some pixels", and not the whole target.

| Backend | Predicted moved | Measured moved |
|---|---|---|
| port | 114,481 | **114,481** |
| reference | 114,481 | **114,481** |
| **RT64** | 114,481 | **114,481** |

RT64 responds to the packet's own prim colour with exactly the texrect pixel
count. It is executing this packet, not ignoring it.

---

## 4. What this does and does not prove

This section is the point of the card, and it holds regardless of the numbers
above.

### 4.1 What the reference-only result proved

`fn64-render-reference` shares no rendering code with `fn64-render-wgpu`: a CPU
rasterizer with its own decoder, TMEM model and combiner. Agreement between
them proves the two lanes are **internally consistent** — that one is not a
transcription of the other, and that a defect in one would have to be mirrored
exactly in the other to hide.

It does **not** prove fidelity to N64 silicon. Both lanes read the same public
SGI documentation, through the same project's interpretation of it. A shared
misreading produces two implementations that agree with each other and with
nothing else. **An independent implementation is not an independent
authority.** That distinction is the whole reason this card was written, and
it stays true even now that the comparison has been widened.

### 4.2 What adding RT64 does prove

RT64 raises the evidence one real step, because it breaks three of the
shared-lineage assumptions at once:

- **Different authors and source tree.** RT64 is upstream MIT C++ pinned at
  `f0728a25`, not a fn64 artifact. Its reading of the RDP was done by people
  outside this project.
- **Different execution substrate.** RT64 rendered these pixels through its
  native Metal pipeline on a real GPU, not through a CPU model or through
  fn64's wgpu path.
- **Different code path into the comparison.** See §5 — RT64 and the
  reference implement `process_rdp_commands`; the port does not, and was
  driven through the production ABI shim instead.

So the claim can now be stated more strongly than before: three
implementations, two of them outside fn64's rendering lineage entirely, agree
to the pixel on this packet.

### 4.3 What it still does not prove

Honest bounds, because the temptation to over-read a clean result is exactly
what this document is guarding against:

- **RT64 is not silicon either.** It is a widely-eyes-verified renderer with a
  strong reputation for fidelity, and that is genuinely better evidence than a
  same-lineage sibling. It is still a third implementation, not hardware, and
  not a hardware capture. Agreement with RT64 is strong corroboration; it is
  not proof.
- **One packet, one entry, one frame.** This is WM2000 decode entry 0, whose
  census shape is 60 fills and 60 texrects with **zero triangles**. Nothing
  here says anything about triangle rasterization, depth, LOD, mipmapping,
  multi-tile combines, or any of the surface the port has not reached.
- **One mode of one field.** The agreement holds with `alpha_dither =
  Disabled`. `Pattern` remains **refused, not compared**
  (`RT64-WM2000-VALIDATION.md`), because this workspace's two ports disagree
  about the Bayer table at 8 of 16 cells. `Noise` remains unmatchable by
  construction. Neither refusal was weakened to widen this table.
- **The packet's own content is narrow.** Two distinct output halfwords across
  115,200 pixels. A flat-primitive blend and a fill exercise far less of the
  RDP than the pixel count suggests, and three implementations agreeing on a
  flat colour is a weaker statement than three implementations agreeing on a
  textured, depth-tested scene.

**Verdict.** The port's frame 0 is now corroborated by an implementation
outside its own lineage, which is a real strengthening over the previous
same-lineage-only result. It is **not** validated against hardware, and no
claim in this repo should read it that way. The remaining gap to an actual
authority is a hardware capture or an accepted silicon-derived reference — and
that gap is unchanged by this card.

---

## 5. Finding: `WgpuBackend` does not implement `process_rdp_commands`

This card was dispatched on the premise that `process_rdp_commands` is "the
same entry point the reference oracle was driven through", and that the same
captured words could therefore go to all three backends through it. **Half of
that premise is wrong, and the error is recorded here rather than quietly
routed around.**

Driving `WgpuBackend` through `RenderBackend::process_rdp_commands` produces:

```
Backend { backend: "render",
          reason: "raw RDP command execution [0x00001000, 0x00001d48) is unsupported" }
```

That string is the **trait default** at `crates/fn64-render/src/lib.rs:1643`.
`WgpuBackend` does not override the method:

| Backend | implements `process_rdp_commands`? |
|---|---|
| `fn64-render-reference` | yes — `backend/render_backend.rs:107` |
| `fn64-render-rt64` | yes — `lib.rs:1595` |
| `fn64-render-wgpu` | **no** — inherits the refusing default |

The port's raw-RDP path is the `RawDpcAbiSession` seam. Measuring it through
the trait method would have measured the trait default's refusal and reported
it as a port failure — a false negative that would have looked exactly like a
real disagreement.

The example therefore drives the port through
`fn64_abi::osDpSetNextBuffer_recomp`, the real `libultra` shim a recompiled
game calls, rather than through a test-internal helper. All three backends
still receive the identical staged bytes over the identical `[start, end)`
range; only the entry point differs, because the port has no other one.

`fn64-render-rt64` additionally declares
`RawDpcBatchCapability::Unsupported` and refuses `process_raw_dpc_batch` by
name ("RT64 raw-DPC batching requires a native separate-command-buffer
seam"). That refusal was left intact; the comparison uses
`process_rdp_commands`, which RT64 does implement.

---

## 6. Reproducing

```sh
FN64_RT64_DIR=/Users/jer/Code/no-mercy-recompiled/third_party/rt64 \
FN64_WM2000_PACKET_TSV=<packet.tsv> \
cargo run -p fn64-render-rt64 --features rt64 --example rt64_wm2000_three_way
```

The example is
[`crates/fn64-render-rt64/examples/rt64_wm2000_three_way.rs`](../crates/fn64-render-rt64/examples/rt64_wm2000_three_way.rs).
It reuses the existing comparison's harness shape — the same packet parser
with its contiguity check, the same `alpha_dither` control, the same
`copy_logical_bytes` readback through `fn64-runtime`'s single authority on the
`^3` storage lane — rather than introducing a second comparison methodology.

The packet dump is game-derived and is **not** tracked in git. It must be
supplied via `FN64_WM2000_PACKET_TSV`; the example refuses by name when the
variable is unset rather than passing silently.

It lives as an example rather than a test because `fn64-abi` cannot depend on
`fn64-render-rt64` (that crate dev-depends on `fn64-abi`, so the dependency
would be circular), and because the `rt64` feature is opt-in and not built by
CI.

The only production-tree change this card makes is adding `fn64-render-wgpu`
as a **dev-dependency** of `fn64-render-rt64`, so all three backends can be
compared in one process. No implementation was touched.

---

## 7. Verification

Measured in this worktree at `9499d078`, not quoted from a brief.

| Check | Result |
|---|---|
| Workspace suite | **8322 passed, 13 skipped** — unchanged (this card adds an example and a doc, no tests) |
| Determinism | **10 consecutive runs, one distinct digest** across ten captured logs |
| Mutation control | `+1` red step predicted 115,200 moved, measured **115,200** |
| Positive control | prim-red rewrite predicted 114,481 moved per backend, measured **114,481** on all three |
| `scripts/lint-docs.py` | **1 error, 3 warnings before and after — byte-identical output**, pre-existing and preserved |
| Dead-code warnings | **1218** (`never used`/`never read`/`never constructed`, `--workspace --all-targets`), none from this card's files |

Circulating dead-code figures conflict (1041/1060/1201/1218); 1218 is this
worktree's own measurement and is the only one this document vouches for.
