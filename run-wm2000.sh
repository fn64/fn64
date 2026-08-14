#!/bin/zsh
# Open a live window running the recompiled WM2000.
#
# The window renders through fn64-render-reference and drives the SAME
# certified dense-AOT block program the headless gate runs -- it is the real
# recompile, not a preview.
set -e
cd "${0:A:h}/examples/wm2000-block-boot"
source ../../.claude/local.env
export ROM="$FN64_DISCOVER_NWXE_ROM"
C=~/Code/aki-recomp/captures
G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1
export FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
# Scripted input: boot -> Exhibition -> Single Match -> entrance.
# Drop this line to sit at the title screen instead.
export FN64_CONTROLLER_SCHEDULE=../../reference/wm2000-routes/entrance-to-match.schedule
exec ./target/release/wm2000-shell
