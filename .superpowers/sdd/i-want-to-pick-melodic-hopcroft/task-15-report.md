# Task 15: TexRectFlip (0x25) — DONE

**Commit:** c8ba2cb5 (branch worktree-wm2000-playable)

## Where the refusal was
`plan_texture_rectangle` (raw_dpc/mod.rs) returned `Ok(())` early on
`rectangle.flip()`, declaring no destination write. With no declared write the
executor never ran the draw, so wgpu produced nothing while RT64 and angrylion
both rendered the flip.

## The fix
- `TexrectDraw` gains a `flipped_axes: bool` + `with_flipped_axes()` + a new
  `coordinates_at(column, row)` that, when flipped, advances **S down rows** and
  **T across columns** (opcode 0x25's transposed screen-axis assignment). The
  destination footprint is identical to the unflipped rect; only S/T stepping
  swaps. `execute_texture_rectangle` now calls `coordinates_at`.
- `plan_texture_rectangle` no longer refuses flip — flip declares the same rows.
- `PlanCollector::texrect_commands` tuple gains a trailing `bool`, decoded from
  `((word0 >> 24) & 0x3f) == 0x25`, threaded to `execute_scheduled_texrect`.

## Before/after parity (gen-texrect-flip)
- **Before:** wgpu refused.
- **After:** wgpu completed, `wgpu_vs_angrylion_diff_pixels: 0`,
  `rt64_vs_angrylion_diff_pixels: 0` → `pass-all-match-hardware`.
- Parity gate: PASS 33/37, 4 expected non-identical outcomes intact.

## Mutation/revert behavior
Reverting the flip stepping is caught by
`raw_dpc::texture_rectangle::tests::missing_flip_swap_would_be_caught` and the
new `flipped_axes_advance_s_by_row_and_t_by_column`; reverting the planner gate
change re-refuses the draw (the old "declares no write" assertion, now inverted
to `a_texture_rectangle_flip_declares_the_unflipped_destination_rows`, would
fail).

## Note
Codex b8bij20oz authored these edits but never built/committed/reported (the
recurring sandbox build-block). Verified and committed from the orchestrator.
