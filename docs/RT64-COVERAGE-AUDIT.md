# Coverage audit: nonclaims, unreachable refusals, surviving mutants

Measured at `1020f1d0` in a fresh worktree. Every number here was re-measured,
none quoted. Baselines: workspace **8319 passed / 13 skipped**; dead code
**1218 items** (1198 of them `fn64-render-wgpu`'s lib alone -- the inert ported
modules); `scripts/lint-docs.py` **1 error, 3 warnings before and after**.

## 1. Nonclaims: declared absent vs. actually exercised

The dangerous class declares a *pipeline stage* absent. 219 `Nonclaim`
mentions across 82 files; all but a handful are RT64 port-scope statements on
inert modules. The live-path intersection:

| Declared absent | Site | WM2000 actually exercises? | Verdict |
|---|---|---|---|
| "no alpha-compare / blend / coverage-write / `SetCombine` decode" | `targets/triangle_pipeline.rs:77-93` | **Yes** -- all four are wired below the same doc (`:225`, `:289-349`, `:202`) | **Stale prose**, not a gap. Code is right, doc contradicts it |
| "No scissor" | `targets/texrect.rs:122` | `G_SETSCISSOR` x932, 218/218 frames | Safe **today**: the capture's scissor is 480x240 = the whole color image, so it is the identity; a non-identity scissor hits `NegativeViewportOrigin`, a loud refusal |
| RGB dither is identity | `targets/texrect.rs:92-100` | high word `0x0000acef` -> `rgb_dither=Disabled` | Correct and measured |
| "No two-cycle, no Fill cycle" | `targets/texrect.rs:72` | 2,520/2,520 one-cycle | Correct |
| "No Shade / Texel1 / Combined / LOD / noise / chroma key" | `targets/texrect.rs:60-68` | two programs read only Texel0/Prim/Env/Zero/One | Correct, refused **by name** |

**No stage-absence nonclaim in the live path is both declared absent and
exercised.** Defect 4's shape is closed here. The window limit stands: 383 VI
fields of boot/logo/attract bounded by the unrelated `0x1CC` MMIO abort, so
"not exercised" is never "not exercised by the game" -- it specifically
weakens the scissor, two-cycle, Shade and Z-variant conclusions, all of which
gameplay could reach.

## 2. The replay doc is stale, and a bare assert refuses 3 of 4 entries

Re-ran `a_real_wm2000_packet_replayed_through_wgpu_backend` on the captured
2,457-command dump. `RT64-WM2000-REPLAY.md` records all four entries refusing
at `LoadTLUT`; that is no longer true.

| Entry | Result now |
|---|---|
| 0 | **EXECUTED through `WgpuBackend` with no refusal** |
| 1,2,3 | Refused by `assert_eq!` -- "v11's admitted TMEM source plan is exactly one journal access wide" (`raw_dpc/production_adapter.rs:1227`) |

That refusal is a **bare panic, not a named typed error**, against AGENTS.md's
"loud traps" convention that every refusal names itself. It is the binding
frontier on 3 of 4 real entries. `production_adapter.rs` is another lane's
file; reported, not touched.

## 3. Named error variants with no test

**300 variants across 37 enums; 147 (49%) have no test constructing their
trigger** (131 mentioned-only, 16 never constructed anywhere). Verified no
`use ...Error::*` glob exists, so qualified-path matching is authoritative.
The gap is concentrated at **geometry/bounds and TMEM-transfer boundaries**,
not at pixel math -- every texrect combiner/blender refusal *is* tested.

Highest risk: the three TMEM load executors (`load_tile`/`load_block`/
`load_tlut`) are near-identical clones sharing **8 untested variant names each
with byte-identical `Display` text**, so a test written against one appears to
cover all three under text grepping. That is defect 3's shape, tripled.

## 4. Mutation survivors

Eight mutants run, each restored and verified byte-identical by `shasum -a
256`. Five were killed (coverage `blend_enabled` disjunct, `ALPHA_CVG_SEL`
alpha write, `CoverageDestination::Full`, `ReservedAlphaCompare`, and
`track_rdp_renderer_mutation` -- the historic survivor, now genuinely killed
by `an_admitted_fill_reaches_the_write_barrier_journal`). Three survived:

| # | Exact mutation | Result | Classification |
|---|---|---|---|
| M6 | `targets/texrect.rs:250-255` -- delete **both** the `NegativeViewportOrigin` and `EmptyViewport` guards | 8319/8319 green | **Genuine coverage gap.** Guards every one of 2,520 texrects |
| M7 | `targets/texrect.rs:260-265` -- delete **both** the `NonIntegralTexcoord` and `TexcoordOutOfRange` guards, leaving a silent `as i16` truncation | 8319/8319 green | **Genuine coverage gap.** Silent truncation is exactly the "no silent shrugs" ban |
| M8 | `tmem/execute/load_tlut.rs:322-331` -- delete **both** `validate_transfer_shape` `TransferMismatch` checks | 8319/8319 green | **Coverage gap or unreachable-by-construction -- undetermined.** Nothing in the crate distinguishes the two. Pinned by an `#[ignore]`d marker test in that file |

M6/M7 sit in `targets/texrect.rs`, another lane's file: reported, not pinned
in place.

## 5. Conformance-harness verdict: green, but structurally cannot do it

`fn64-render-conformance` is **not stale** -- it builds clean, 2/2 tests pass,
`check_rt64_port_parity.py` is clean over 50 rows, and there is no `todo!()`
or stub anywhere. The prior "rust_port delegate unimplemented" finding is
**refuted**: a real non-RT64 delegate runs and passes.

But it cannot diff two backends over a captured packet, for three independent
reasons: both runners hard-reject any packet but their own reviewed fixture
(`reference-runner.rs:213-227`); the expected answer is hand-derived
arithmetic, not a golden file, so it does not generalize to a display list;
and no capture->packet path exists from a running ROM. Decisively, **no row
has both backends qualified** -- the two live delegates run different fixtures
at different observable layers, and wgpu is not wired in at all. It is a
10-run determinism attestation harness ("does backend X match its own
authority"), never a comparator. There is no command line to report.

## 6. Verified side-claims

- `flip_wire_position` matches **no test, and no source at all** in this
  checkout -- prior lane confirmed correct.
- `texture_rectangle_at` is a substring of
  `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position`,
  which is `#[cfg(feature = "host-gpu-tests")]` and therefore absent from the
  default suite entirely.
