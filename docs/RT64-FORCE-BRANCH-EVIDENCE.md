# RT64 force-branch enhancement evidence

Status: causal synthetic-HLE behavior proven on the certified Metal host; the
broader platform and recognized-game rows remain open.

## Public mechanism

Pinned MIT RT64 exposes
[`EnhancementConfiguration::F3DEX::forceBranch`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/common/rt64_enhancement_configuration.h#L29-L32)
and tests it alongside the Extended-GBI override before both
[depth- and W-branch decisions](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_rsp.cpp#L837-L857).
This is an optional host enhancement, not compiled game state. The
fn64 typed policy already carried that exact boolean through
`RenderEnhancementSettings::f3dex_force_branch`; this gate proves the field
changes executed render control flow rather than only changing its policy
digest.

The fixture uses the public F3DEX2 `gSPBranchLessZraw` envelope documented in
`F3DEX2-CONCEPTS.md`: `G_RDPHALF_1` stages a tail-branch target and
`G_BRANCH_Z` compares one transformed vertex's screen Z. Its threshold is zero
while the selected vertex has positive screen Z, making the normal condition
false. The fallthrough draws one red triangle; the branch target draws the
same triangle in green.

## Measured result

[`rt64_force_branch_behavior.rs`](../crates/fn64-render-rt64/examples/rt64_force_branch_behavior.rs)
creates one pinned Metal backend and switches the enhancement off, on, then
off again through the live policy path. Every phase submits the same
hand-authored, non-ROM F3DEX2 display list through the non-default
`synthetic-f3dex2-evidence` transport and captures exact post-VI BGRA8 bytes.
The gate also calls normal `process_task` before and after the synthetic
interval and requires `NeedsLle`, proving test admission did not leak into the
production hash-recognition path.

Ten consecutive fresh processes passed on 2026-07-20 using clean pinned RT64
on macOS 26.5 arm64 / Apple M5 Pro:

| Phase | Active-policy SHA-256 | Post-VI SHA-256 | Exact color |
|---|---|---|---|
| force off | `0ae411439f1b742ee2017a8f537212767925b71810b4813461becefdee40f3e9` | `b7116e2234e90cc2eaa468cd8506204c1015285bcb03c5b5672c118c38b22e61` | 161 red pixels, zero green |
| force on | `62563b745de9c410e35f8b472388eb51c7146c9260aa748688de8b11c5547b97` | `17899a3ce23323c0c0c84b4d26afa12a5d0664ab85dd7cbe22156f5569c1692b` | 161 green pixels, zero red |
| force off restored | exact first-phase identities | exact first-phase identity | exact first-phase colors |

This closes the local force-branch control/effect gap. It does not close
widescreen, HFR, Extended GBI, full-ROM, or cross-platform claims, and it does
not claim that a synthetic display list is recognized game microcode.
