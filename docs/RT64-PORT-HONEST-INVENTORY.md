# RT64 port: honest inventory

Measured 2026-08-17 against `port/rt64-conveyor` @ `887ba6c2`. Every number
here is reproduced by `tools/rt64_port_coverage.py`; none are asserted.

## Why this document exists

`docs/rt64-port-inventory.json` reports **61.4% ported**. That number is
`sum(lines)` over files whose `port_state == "ported"`, and `ported` is
awarded on `evidence_state: "source-digests-verified"` — the upstream file's
SHA-256 matches the pinned commit. It verifies *the upstream file has not
drifted*. It does not verify that any of its behavior was reproduced.

The consequence: a 7-line header (`src/apple/rt64_apple.h`) and a 1,791-line
translation unit (`src/render/rt64_texture_cache.cpp`) both count as fully
ported, and 193 upstream files are credited to 81 Rust modules — one
1,689-line module (`rt64_framebuffer_shaders.rs`) carries credit for 16
separate C++ files.

## The three numbers

| metric | value | what it means |
|---|---:|---|
| digest-verified ("ported") | **61.8%** | upstream file hasn't drifted; the current headline |
| line-cited coverage | **34.4%** | upstream lines a Rust module actually cites porting |
| production-wired | **1 / 62 modules** | reaches a rendered frame |

Corpus is 48,065 lines across 276 files.

| state | files | lines | % corpus |
|---|---:|---:|---:|
| ported (digest-verified) | 193 | 29,701 | 61.8% |
| — of which line-cited | | 16,528 | 34.4% |
| authority-gated | 10 | 10,596 | 22.0% |
| refused | 73 | 7,768 | 16.2% |

## Coverage distribution across the 193 "ported" files

| cited coverage | files | upstream lines |
|---|---:|---:|
| 0% (named, no line range) | 47 | 4,466 |
| 1–25% | 31 | 6,067 |
| 26–50% | 35 | 3,426 |
| 51–75% | 20 | 1,897 |
| 76–99% | 12 | 2,426 |
| 100% | 48 | 11,419 |

**78 of 193 files marked `ported` cite ≤25% of their upstream lines.**

### Largest over-credited files

| upstream lines | cited | file |
|---:|---:|---|
| 1,791 | 11.0% | `src/render/rt64_texture_cache.cpp` |
| 1,042 | 0.0% | `src/hle/rt64_game_frame.cpp` |
| 756 | 2.2% | `src/hle/rt64_application.cpp` |
| 561 | 1.2% | `src/gbi/rt64_gbi.cpp` |
| 492 | 7.3% | `src/common/rt64_math.cpp` |
| 486 | 0.0% | `src/preset/rt64_preset_draw_call.cpp` |
| 410 | 0.0% | `src/common/rt64_replacement_database.cpp` |
| 291 | 0.0% | `src/preset/rt64_preset_material.cpp` |
| 244 | 0.0% | `src/gbi/rt64_gbi_f3d.cpp` |
| 212 | 4.7% | `src/gbi/rt64_gbi_f3dex2.cpp` |

## Wiring

62 `rt64_*` modules exist in `fn64-render-wgpu`. All 62 are declared `mod`,
never `pub mod`. There are **zero** `pub use` re-exports.

Exactly **one** module reaches production code:

- `rt64_gbi_rdp_decode::decode_set_scissor` — called from `raw_dpc/mod.rs`
  and `raw_dpc/production_adapter.rs`, replacing a live rejection path.

Two others look wired but are not:

- `rt64_rsp_patch` — referenced only from `rt64_rsp_world_modify/tests.rs`.
- `rt64_float4_quantize` — referenced only from a doc comment in `fbcommon.rs`.

The remaining 59 have no referrer of any kind. The 2,864 inline `#[test]`
cases in these modules assert the modules against themselves and against
hand-computed values; they do not assert against RT64.

## The parity ledger

49 of 50 rows are `RUST_PENDING`. 48 of those give the reason *"The Rust
renderer delegate does not exist yet."*

The one `RUST_PASS` row (`feature::native-renderer-rdram-sync`) is earned by
`fn64-render-reference`, a **pure-Rust CPU rasterizer that predates this
port** — not by any of the 62 ported modules. It is registered under
`delegate_kind="rust_port"`, which makes it read as port progress in the
ledger. It is not. No ported module has ever been executed against an RT64
authority.

## What the port has genuinely produced

Not captured by any percentage, and the real value on this branch:

- **Five measured behavioral disagreements** between RT64 and fn64
  (`setPrimDepth` 1 ULP, VI scale 112-vs-113 rows, RGBA32 TMEM 2× stride,
  `rgbDither` return convention, `visible()` divergence). These are findings
  that exist nowhere else and are pinned by tests.
- **Evidenced `refused` states** for all 73 refused files — each cites an
  assessing commit. Before this branch the same tree read "87 files remain."
- **`check_stale_deferrals`** — found 36 stale "does not port X" claims.

## Reproducing

```
python3 tools/rt64_port_coverage.py
```

Method: parses every `crates/**/*.rs` for upstream citations in two forms —
full-path (`src/hle/rt64_rsp.cpp:120-180`) and bare-basename
(`rt64_render_target.cpp:487-497`, resolved only when the basename is
unambiguous across the corpus) — plus whole-file claims. Union of cited line
numbers per upstream file, clamped to that file's length.

**Known limits of the method.** Citation coverage is an *upper* bound on
ported behavior: a cited line range means a module claims to port it, not
that it does so correctly. It is also a *lower* bound on effort, since a
module may port logic without citing a range. It is a better metric than
whole-file digest credit, and it is not a parity measurement.
