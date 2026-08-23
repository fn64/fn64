# Task 34 (measure): is the RSP gfx interpreter separate perf headroom? READ-ONLY

## The signal
Task 22 measured, on WM2000's slow render fields (rs+wgpu attract):
**`rsp_steps_gfx ≈ 383,170` per render field**, `rsp_entries ≈ 9.3` (vs ~4,000
steps / 1.3 entries on off-fields). The RSP gfx microcode interpreter is HEAVILY
exercised on exactly the render fields — but the report does NOT resolve whether
that cost is:
- (a) already counted INSIDE `gfx_lle_rdp` / the execute stage (i.e. the same ~88%
  Task 32 owns — no new headroom), or
- (b) a SEPARATE cost upstream of the raw-DPC dispatch (the RSP interpreter that
  produces the DPC command stream) — which would be real, untapped headroom that
  neither Task 32 (rasterizer) nor Task 33 (plan phase) addresses.

This task RESOLVES that and, if it's separate + reducible, scopes it. NO code changes.

## Investigate (READ-ONLY)
1. Find the RSP gfx interpreter and how `rsp_steps_gfx` is counted. Grep for
   `rsp_steps_gfx`, `rsp_entries`, the RSP microcode step loop (likely in
   fn64-runtime / an rsp module, and/or `rt64_rsp_process.rs` in fn64-render-wgpu).
   Determine what one "step" is and what 383k steps/field actually execute.
2. RESOLVE the bucketing: is the RSP interpreter's time inside the `gfx_lle_rdp_ns`
   phase bracket (the ~88% execute domain) or measured separately? Read where
   `gfx_lle_rdp_ns` starts/stops relative to the RSP step loop. This is the crux —
   if it's already in the 88%, there's no new opportunity and Task 32 covers it;
   if it's separate, it's new headroom.
3. If separate: estimate its share of the render field (steps × per-step cost, or a
   bracketed measurement). Is it above the resolution floor? What drives 383k
   steps — is WM2000's gfx microcode doing redundant work, or is that inherent to
   the display list size? Is there a decode/dispatch hot spot in the step loop?
4. If it IS a real separate cost: a scoping sketch of what a fix would touch and its
   size (e.g. step-loop dispatch overhead, per-step bookkeeping) — with the caveat
   that RSP interpreter correctness is load-bearing (it produces the command stream;
   any change must be byte-identical, validated against the parity/replay path).

## Constraints
- READ-ONLY / measurement + scoping. No code changes, no worktree, no commits.
- Invoke `fn64-perf-method`; numbers renderer-tagged (wgpu+rs); thermal drift real
  (min-of-N / bounded runs).
- Do NOT overlap Task 32 (rasterizer/execute) or Task 33 (plan phase). Your domain
  is the RSP gfx interpreter step loop and its bucketing ONLY.

## Deliverable
The bucketing verdict (RSP interp cost is INSIDE execute-88% = no new opportunity,
OR SEPARATE = real headroom), and if separate, its measured share + a scoping sketch
+ size estimate. If it's inside the 88%, say so plainly — that's a valuable negative
(confirms Task 32 already covers it).

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-34-report.md` + concise verdict.
