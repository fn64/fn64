#!/bin/zsh
# ACCEPTANCE TEST for the geometry substitution: the shipped binary must contain
# ZERO verbatim ROM words, with the verify-live-words gate on.
#
# Builds one lane -- geometry `WORDS` removed AND FN64_WM_SHARD_VERIFY_LIVE_WORDS=0
# -- then runs the audit's own search against it.
#
# WHY THE CONTROLS MATTER MORE HERE THAN THEY DID FOR THE AUDIT
#
# The audit used the un-gated binary as its positive control: a lane known to
# embed ROM words, so a zero there meant the search was broken. After the
# geometry substitution BOTH lanes are clean, so that control has nothing left
# to find. A search validated only that way would report a clean zero whether it
# worked or not -- exactly the false-zero shape the audit's own Mach-O parser bug
# produced (it mapped 0 sections and every hit read "outside any section").
#
# So acceptance rests on three controls, in increasing strength:
#
#   1. SYNTHETIC (built into rom-content-audit-search.py, always on): plants a
#      known ROM run in all four orderings into a buffer and requires the matcher
#      to find each, plus a never-planted run it must NOT find. Depends on no
#      binary at all. Exits 2 if the matcher itself is broken.
#   2. ARCHIVED PRE-CHANGE BINARY (target-audit-verifyon): a real Mach-O of this
#      same program known to carry ~1.82 MiB of ROM words. Passed via
#      --require-control, so a zero there is a hard failure. This is what proves
#      the search still works against a REAL binary of this shape, not just a
#      synthetic buffer.
#   3. --require-clean on the new binary: the assertion under test.
#
# Only with 1 and 2 passing does the zero in 3 mean anything.
#
# ORDERINGS: all four are searched (z64/swap4/swap2/swap2of4). Every hit the
# audit found was swap4 and none was raw z64, so a single-endianness search
# returns a false zero across the board.

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

CONTROL="$REPO/target-audit-verifyon/release/wm2000-block-boot"
TDIR="$REPO/target-audit-geometry"
BIN="$TDIR/release/wm2000-block-boot"

for required in "$ROM" "$FN64_BOOT_CONTEXT" "$FN64_RT64_DIR" \
                "$G/run-1/image.json" "$G/run-2/image.json" "$G/run-3/image.json"; do
    [[ -e "$required" ]] || { print -u2 "MISSING: $required"; exit 1; }
done

print "=== BUILD (geometry lane; verify-live-words OFF)"
(
    cd "$REPO/examples/wm2000-block-boot" || exit 1
    export CARGO_TARGET_DIR="$TDIR"
    export FN64_WM_SHARD_VERIFY_LIVE_WORDS=0
    cargo build --release --features rt64 --bin wm2000-block-boot
)
BUILD_RC=$?
print "=== BUILD exit=$BUILD_RC"
[[ $BUILD_RC -eq 0 ]] || exit $BUILD_RC

# Rule 19: a size delta proves "something changed", never "the thing you meant".
# Check the GENERATED SOURCE for both arrays before trusting any search result.
print ""
print "=== RULE 19: both word channels absent from the GENERATED SOURCE"
EXPECTED=$(grep -rl 'EXPECTED_WORDS' "$TDIR"/release/build/*/out/runner.rs 2>/dev/null | wc -l | tr -d ' ')
WORDS=$(grep -rlE 'static WORDS' "$TDIR"/release/build/*/out/metadata.rs 2>/dev/null | wc -l | tr -d ' ')
META=$(ls "$TDIR"/release/build/*/out/metadata.rs 2>/dev/null | wc -l | tr -d ' ')
GEOM=$(grep -rl 'ROM_START' "$TDIR"/release/build/*/out/metadata.rs 2>/dev/null | wc -l | tr -d ' ')
print "  metadata.rs files:            $META"
print "  ...with 'static WORDS':       $WORDS   (must be 0)"
print "  ...with ROM_START geometry:   $GEOM   (must equal metadata.rs count)"
print "  runner.rs with EXPECTED_WORDS: $EXPECTED   (must be 0)"
SRC_OK=1
[[ "$WORDS" == "0" ]] || { print -u2 "  FAIL: WORDS arrays still emitted"; SRC_OK=0; }
[[ "$EXPECTED" == "0" ]] || { print -u2 "  FAIL: EXPECTED_WORDS still emitted"; SRC_OK=0; }
[[ "$META" != "0" && "$GEOM" == "$META" ]] || { print -u2 "  FAIL: geometry not emitted in every shard"; SRC_OK=0; }
[[ $SRC_OK -eq 1 ]] && print "  PASS"

print ""
print "=== SIZES"
stat -f "%z bytes  %N" "$CONTROL" "$BIN" 2>/dev/null

print ""
print "=== ROM-CONTENT SEARCH (synthetic control + archived control + clean requirement)"
python3 "$REPO/scripts/rom-content-audit-search.py" \
    --rom "$ROM" \
    --binary "$BIN" \
    --binary "$CONTROL" \
    --require-control "$CONTROL" \
    --require-clean "$BIN" \
    --json-out "${CLAUDE_JOB_DIR:-/tmp}/tmp/rom-content-accept.json"
SEARCH_RC=$?

print ""
print "=== RESULT"
print "  build=$BUILD_RC  generated-source=$([[ $SRC_OK -eq 1 ]] && print PASS || print FAIL)  search=$SEARCH_RC"
[[ $BUILD_RC -eq 0 && $SRC_OK -eq 1 && $SEARCH_RC -eq 0 ]] \
    && { print "  ACCEPTED"; exit 0 } \
    || { print "  NOT ACCEPTED"; exit 1 }
