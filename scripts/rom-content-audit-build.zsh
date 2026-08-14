#!/bin/zsh
# Build the shipping-shaped wm2000-block-boot binary in BOTH verify-live-words
# lanes, for the binary ROM-content audit (docs/plans/rom-content-in-shipped-artifact.md).
#
# Lane VERIFY_ON  (FN64_WM_SHARD_VERIFY_LIVE_WORDS unset -> default true):
#   emits EXPECTED_WORDS *and* WORDS. This lane is the POSITIVE CONTROL for the
#   byte search: it is known to contain verbatim ROM words, so a search that
#   finds nothing here is a broken search, not a clean binary.
# Lane VERIFY_OFF (FN64_WM_SHARD_VERIFY_LIVE_WORDS=0):
#   drops EXPECTED_WORDS, keeps WORDS. The delta between the two lanes measures
#   what the gate actually removes.
#
# Each lane gets its OWN CARGO_TARGET_DIR so neither clobbers the other and both
# binaries survive for comparison.

set -uo pipefail

REPO=/Users/jer/Code/fn64/.claude/worktrees/rom-corpus-catalog
cd "$REPO" || exit 1

source "$REPO/.claude/local.env"

export ROM="${ROM:-$FN64_DISCOVER_NWXE_ROM}"
C="${FN64_CAPTURES_DIR:-$HOME/Code/aki-recomp/captures}"
G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1
export FN64_RT64_DIR="$HOME/Code/no-mercy-recompiled/third_party/rt64"

for required in "$ROM" "$G/run-1/image.json" "$G/run-2/image.json" "$G/run-3/image.json" \
                "$FN64_BOOT_CONTEXT" "$FN64_RT64_DIR"; do
    if [[ ! -e "$required" ]]; then
        print -u2 "MISSING: $required"; exit 1
    fi
done

print "ROM:        $ROM"
print "captures:   $G"
print "boot ctx:   $FN64_BOOT_CONTEXT"
print "rt64:       $FN64_RT64_DIR"
print ""

build_lane() {
    local lane="$1" verify="$2"
    local tdir="$REPO/target-audit-$lane"
    print "=== LANE $lane : FN64_WM_SHARD_VERIFY_LIVE_WORDS=$verify -> $tdir"
    (
        cd "$REPO/examples/wm2000-block-boot" || exit 1
        export CARGO_TARGET_DIR="$tdir"
        if [[ "$verify" == "unset" ]]; then
            unset FN64_WM_SHARD_VERIFY_LIVE_WORDS
        else
            export FN64_WM_SHARD_VERIFY_LIVE_WORDS="$verify"
        fi
        cargo build --release --features rt64 --bin wm2000-block-boot
    )
    local rc=$?
    print "=== LANE $lane exit=$rc"
    ls -la "$tdir/release/wm2000-block-boot" 2>&1
    return $rc
}

build_lane verifyon unset
ON_RC=$?
build_lane verifyoff 0
OFF_RC=$?

print ""
print "verifyon rc=$ON_RC  verifyoff rc=$OFF_RC"
print "ALL BUILDS DONE"
