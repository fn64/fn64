# Task 19 report: ROM-relevance of the 3 shared wgpu+RT64 divergence domains

**Oracle for every claim below:** angrylion-rdp-plus, invoked as an external process (parity-runner.rs:2712, ANGRYLION_ORACLE_DEFAULT). Angrylion is excluded from fn64 clean-room protocol (AGENTS.md:26-45), so a claim about what *hardware* does sourced ONLY to angrylion is not admissible fn64 authority — flagged inline where it applies.

## Verdict summary, ranked by WM2000 relevance

| Rank | Domain | WM2000 hits the path? | Divergence observable in WM2000? | Recommendation |
|---|---|---|---|---|
| 1 | FORCE_BL + coverage | PROVEN yes — all 60 of entry 0 texrects latch other-mode low 0x005041c8 = the exact test combination | PROVEN no (this window) — CVG_X_ALPHA + ALPHA_CVG_SEL both clear, so no output depends on the coverage term | LOG now; conditional FIX gated on CVG_X_ALPHA/ALPHA_CVG_SEL ever being set |
| 2 | CI4/CI8 TLUT triangle | PROVEN yes for CI/TLUT triangles (255,654/255,654 admitted triangles textured; 1,808 LoadTLUT; CI4/IA4-through-TLUT confirmed in gameplay); INFERRED that the specific divergence (triangle S-plane addressing) is hit | Unknown for the divergence specifically | INVESTIGATE then FIX — highest-confidence real fidelity gap on the triangle path |
| 3 | Fog-color blend | PROVEN no — G_SETFOGCOLOR (0xf8): zero occurrences in the WM2000 census window | N/A — path never entered | LOG as a corpus-only rounding divergence; no fix |

---

## Domain 1 — FORCE_BL + coverage (WM2000 rank 1)

Cases: gen-coverage-force-blend-one-cycle (FORCE_BL | IM_RD | CVG_DST_WRAP, parity-runner.rs:3577-3581) and gen-coverage-all-modes-combined-one-cycle (AA_EN | CVG_DST_WRAP | CLR_ON_CVG | FORCE_BL | IM_RD, :3588-3592). Both hit the identical divergence — 0x21d7 in wgpu/RT64 vs 0x3bef in angrylion at pixel x=2 (fan-out report :125-133) — the combined case being a superset of the force-blend case.

### The hardware rule violated (cited)

FORCE_BL (other-mode low bit 14) forces the last blender cycle general divide-path on for every pixel, bypassing the "b==0 selects P" short-circuit. The general output is (P*A + M*B)/(A+B). Under the AA/coverage config these cases use, the B factor is the framebuffer coverage count (BlendBInput::FramebufferAlpha) and M is the existing framebuffer pixel. On hardware angrylion reads a real per-pixel coverage count and a real memory pixel and runs the integer blend datapath.

What wgpu+RT64 do vs angrylion: neither ported engine emulates the memory coverage count. blend_b for FramebufferAlpha returns sample.coverage_count as f32 / 8.0 (fn64-render-wgpu/src/blend.rs:319), but nothing writes a coverage count — TriangleDrawOutput carries color/depth/status only and "no coverage-count GPU write exists anywhere in this crate" (production.rs BlendRequiresFramebuffer, quoted at RT64-WM2000-REPLAY.md:533-537). The CPU texrect executor "implements no coverage at all" (targets/texrect.rs module doc, quoted at REPLAY :524-527). So the coverage term fed into the forced divide is not the hardware value.

Which sub-path diverges: it is the coverage->alpha (B) supply path, not the forced blender arithmetic. The forced divide is correctly wired (blend.rs:543-552); it is handed a coverage/memory term neither engine reproduces from real hardware coverage state. This is exactly the guard-audit gap the slice was built to surface ("Coverage is not emulated"; RT64 is non-authoritative for these rows — fan-out report :130-133), so angrylion is the only trustworthy reading.

### Does WM2000 hit it? — PROVEN YES (path), PROVEN NO (observable divergence)

PROVEN: WM2000 latches the exact test bit combination. All 60 of entry 0 texrects latch other-mode low 0x005041c8 = cvg_dst=Wrap, IM_RD, AA_EN, CLR_ON_CVG, FORCE_BL (RT64-WM2000-REPLAY.md:493-494, read off the packet own words). That is precisely the combined case combination.

PROVEN: the divergence is NOT observable in this window. In WM2000 packet CVG_X_ALPHA and ALPHA_CVG_SEL are both clear (REPLAY :495). The coverage_fragment result reaches FragmentOutput by exactly two routes: output.color.a (guarded by alpha_coverage_select, dead when ALPHA_CVG_SEL clear) and blend_enabled = force_blend || (antialias_enabled && !wraps) (REPLAY :498-504). With FORCE_BL set the second short-circuits to true for every memory value, so the memory-dependent wraps term never reaches an output. Under this exact bit combination no output is a function of the coverage/memory term the pipeline cannot supply (REPLAY :509-514), and entry 0 now executes end-to-end through WgpuBackend with no refusal and publishes correct pixels (115,200 px all accounted for — REPLAY :539-557).

The parity divergence exists because the synthetic coverage cases leave the coverage term observable in a way WM2000 own bit combination does not.

### Recommendation: LOG now, conditional FIX gated on CVG_X_ALPHA/ALPHA_CVG_SEL

- Log the FORCE_BL+coverage divergence as a known shared wgpu==RT64 gap where angrylion is the sole authority (coverage not emulated). For WM2000 boot/logo/attract window it is provably non-observable.
- Conditional fix trigger: the window ends at the 0x1CC MMIO abort before gameplay (RT64-WM2000-CYCLE-MODES.md:222-228). If in-match capture ever shows a draw with CVG_X_ALPHA OR ALPHA_CVG_SEL set under IM_RD, the coverage term becomes observable and this becomes a real fidelity gap. Fix site: add a coverage-count attachment (the "specific missing attachment" named in REPLAY :537) so blend_b FramebufferAlpha reads a true memory coverage value instead of an unwritten count, in the wgpu triangle/texrect pipeline. Cost is high (new GPU attachment + accumulation); do not build until a real draw makes the term observable.

---

## Domain 2 — CI4/CI8 textured-triangle via TLUT (WM2000 rank 2)

Cases: gen-triangle-ci4-bilerp (21 px) and gen-triangle-ci8-bilerp (24 px). Both set BI_LERP_0 and en_tlut (ci4_textured_triangle, parity-runner.rs:3680-3710; ci8_textured_triangle, :3716-3747). First diff for CI4: pixel x=3 reads 0x7fff in both wgpu and RT64 where angrylion reads 0x0001 (fan-out report :100-115).

### The hardware rule violated (cited)

The palette lookup and post-lookup filter are NOT the diverging step — those are proven correct on the texrect path. The diverging step is the triangle S-plane texture-coordinate addressing for CI formats.

Evidence it is the addressing, not the lookup:
- 0x7fff is PALETTE[3] (parity-runner.rs:981, entry 3). CI_INDICES[0]=3 (:966), so 0x7fff is the value that should appear at x=0, not x=3. Both engines read the x=0 index while rasterizing x=3 — an S-coordinate origin/advance offset on the triangle path.
- angrylion 0x0001 is the decode of an unwritten palette slot (runner own PALETTE doc :975-979: staging the palette off-by-0x40 makes both engines return 0x0001, "the decode of an unwritten palette"). Angrylion lands on an index mapping to an unwritten TLUT entry — a different S coordinate than the ported engines compute.
- The palette-lookup fold is confirmed correct three ways: fn64 (palette<<4)|texel4 (tmem/texel.rs:365) equals angrylion (tpal<<4)|p (tmem.c:271) exactly (RT64-WM2000-TEXTURE-STATE.md:220-223, RT64-WM2000-COMBINER-CENSUS.md:86-109). The CI-without-TLUT->I8 alias is byte-exact with angrylion and REFUTED as a defect (COMBINER-CENSUS :79-116, :358). So the TLUT decode is right; the S coordinate feeding it differs.
- This is the first time either CI format has been driven through the triangle path rather than texrect in this corpus (fan-out report :100-104); the existing texrect CI cases (one_ci4_rect/one_ci8_rect, parity-runner.rs:1040,1088) pass. The regression is specific to triangle S-plane addressing under non-perspective texture coordinates (triangle_span::texture_coordinates_s10_5, raw_dpc/triangle_span.rs:608; tmem/sample.rs:296-332 relative_axis_coordinate).

Precise statement: wgpu+RT64 compute a triangle S coordinate that indexes the CI tile one texel-origin off from what angrylion rasterizer computes; the palette lookup then faithfully returns the wrong-but-consistent entry. PROVEN that the palette fold matches; INFERRED (from the 0x7fff=PALETTE[3] shift) that the exact cause is the S-plane origin/advance — a follow-up should pin the delta.

### Does WM2000 hit it? — PROVEN yes for CI/TLUT triangles; INFERRED for the divergence

- PROVEN: WM2000 draws almost exclusively textured triangles. 255,654 of 255,654 admitted triangles are textured and every one reaches sample_point (RT64-WM2000-COMBINER-CENSUS.md:14-16). RDP_TRI_SHADE_TEX (0x0e) is 10,380 in the census window and is the only triangle variant used (RT64-WM2000-CENSUS.md:183,220-221).
- PROVEN: WM2000 loads TLUTs and samples CI/nibble-palette formats. G_LOADTLUT x1,808 (CENSUS.md:191). Gameplay draws sample CI4 and IA4-through-TLUT via the nibble palette path (RT64-WM2000-TEXTURE-STATE.md:207-223, a 2.38M-draw / 1M-raw-triangle in-match corpus). So CI/TLUT texels ARE fetched by triangles in this title.
- INFERRED, not proven: that WM2000 CI/TLUT triangles hit the specific S-plane addressing offset the parity case exposes. The texrect CI cases pass; whether the same one-texel S offset manifests on WM2000 actual triangle coordinates has not been directly measured. WM2000 own texture-state investigation is already chasing dark/blocky glyphs and colour bands whose candidate cause is TLUT residency/addressing upstream of lut_mode (COMBINER-CENSUS :111-116) — consistent with, but not proof of, this offset.

### Recommendation: INVESTIGATE then FIX (highest-confidence real gap)

This is the one domain where the diverging step is a coordinate-addressing bug both ported engines share and that WM2000 demonstrably exercises the surrounding path of (textured triangles + TLUT + CI). Recommend a targeted follow-up: instrument the triangle S coordinate for ci4_textured_triangle and compare against angrylion rasterizer S to pin the exact delta (origin vs advance vs half-texel). Fix site: the shared triangle S-plane addressing — raw_dpc/triangle_span.rs:608 (texture_coordinates_s10_5) and/or tmem/sample.rs:296-332 (relative_axis_coordinate). Because this may be a genuine palette-indirection/addressing defect on the live triangle path — the path WM2000 uses for 100% of its draws — it outranks fog and outranks the non-observable coverage case for a real fix, and must NOT be auto-dismissed as a fixture limitation.

---

## Domain 3 — Fog-color blend (WM2000 rank 3)

Case: gen-blender-fog-color-over-mem (12 px). Blender P=FogColor (m1=3), A=CombinedAlpha (m2=0), M=Framebuffer (m3=1), B=1-A (m4=0), with FORCE_BL+IM_RD and SetFogColor 0x2060a0ff (parity-runner.rs:4204-4209, blend_other_modes(3,0,1,0,true,true)). Both wgpu and RT64 read 0x2329 where angrylion reads 0x22e7 at pixel x=2 — 2 of 3 RGB channels off by one quantization step (fan-out report :119-124).

### The hardware rule violated (cited)

The blend-mux is correct; the divergence is rounding in the general divide path. blend_color correctly selects the fog register for P (fn64-render-wgpu/src/blend.rs:285-289, BlendColorInput::Fog). The general blender then computes (P*A + M*B)/(A+B) in floating point and rounds: blender_rgb[channel] = (numerator / divisor) * COLOR_SCALE (blend.rs:543-552), then round_clamp_u8 = value.round().clamp(0,255) (blend.rs:380-381). The fn64 reference does the same float divide + .round() (fn64-render-reference/src/raster/blend.rs:212-233), and RT64 real mechanism is fixed-function dual-source blending (rt64_raster_shader.cpp:332-339, cited in blend.rs:31-34).

Hardware (angrylion) runs the N64 blender exact integer datapath with the silicon own division-by-sum and rounding, which differs from software round-to-nearest. This is explicitly acknowledged in-source as an unverified nonclaim: "RT64 actual GPU blend-unit rounding mode is unverified... fixed-function blend hardware typically rounds differently from this software round-to-nearest" (blend.rs:376-379). So the diverging step is rounding/quantization in the divide, not the blend-mux and not the fog selection. The 1-LSB-per-channel size is the signature of a rounding-mode mismatch, not a wrong operand.

### Does WM2000 hit it? — PROVEN NO

G_SETFOGCOLOR (0xf8): zero occurrences in the WM2000 census window (RT64-WM2000-CENSUS.md:217-219, "all admitted already, all unexercised by this title in this window"). With no SetFogColor, the fog register is never programmed and no draw selects BlendColorInput::Fog. WM2000 does not enter this path. (Window caveat: boot/logo/attract, 383 VI fields, bounded by the 0x1CC abort — CYCLE-MODES :222-228 — so "unexercised" is window-bounded, but fog color is a deliberate effect a title would program early if used.)

### Recommendation: LOG, no fix

Purely theoretical for WM2000 (fog never programmed) and a sub-LSB rounding difference even where hit. Fixing it would mean replacing the shared float-divide + round-to-nearest with a bit-exact integer emulation of the N64 blender datapath in both wgpu and the RT64 port — which would deliberately diverge fn64 from pinned RT64 (RT64 fixed-function rounding is the thing being matched) for a 1-LSB effect no shipping ROM in the corpus reaches. Record it in the known-divergence ledger; do not build it. Reopen only if a real ROM programs SetFogColor with a fog blend AND the 1-LSB error proves visible.

---

## Ranking rationale (WM2000 relevance)

1. FORCE_BL+coverage — WM2000 provably latches the exact bits (0x005041c8), so it is the most path-relevant. Conditional fix only because the divergence is provably non-observable under WM2000 own CVG_X_ALPHA/ALPHA_CVG_SEL-clear configuration. Highest ROM-contact, lowest current harm.
2. CI4/CI8 TLUT triangle — WM2000 draws 100% textured triangles and loads TLUTs; the diverging step is a real shared coordinate-addressing bug on the live triangle path. Highest confidence of a real fidelity gap a fix would improve, hence the only FIX recommendation — but the link from the parity offset to WM2000 exact glyphs is INFERRED, not yet PROVEN.
3. Fog-color blend — WM2000 never programs fog (zero SetFogColor); a 1-LSB rounding curiosity. Lowest relevance; log only.
