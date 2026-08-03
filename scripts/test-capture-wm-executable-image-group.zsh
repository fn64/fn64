#!/bin/zsh

set -euo pipefail

typeset -r script=${0:A:h}/capture-wm-executable-image-group.zsh
typeset -r scratch=$(mktemp -d /tmp/fn64-capture-group-test.XXXXXX)
cleanup_test() {
    local exit_code=$?
    trap - EXIT HUP INT TERM
    rm -rf -- $scratch
    return $exit_code
}
trap cleanup_test EXIT
touch $scratch/core $scratch/rsp $scratch/rom

cat >$scratch/guard <<'SH'
#!/bin/sh
exec "$@"
SH
cat >$scratch/producer-success <<'SH'
#!/bin/sh
set -eu
[ -n "${FN64_EXECUTABLE_IMAGE_PC:-}" ]
[ -n "${FN64_EXECUTABLE_IMAGE_FIRST_PC:-}" ]
[ -n "${FN64_EXECUTABLE_IMAGE_START:-}" ]
[ -n "${FN64_EXECUTABLE_IMAGE_WORDS:-}" ]
[ -n "${FN64_EXECUTABLE_IMAGE_ID:-}" ]
[ "${FN64_CAPTURE_ONLY:-}" = 1 ]
[ "${FN64_STOP_AFTER_IMAGE:-}" = 1 ]
[ -z "${FORBIDDEN_AMBIENT_VALUE:-}" ]
printf '%s\n' trace >$4
printf '%s\n' boot >$7
printf '%s\n' stable >$FN64_EXECUTABLE_IMAGE
SH
cat >$scratch/producer-unreached <<'SH'
#!/bin/sh
set -eu
printf '%s\n' trace >$4
printf '%s\n' boot >$7
exit 0
SH
cat >$scratch/producer-divergent <<'SH'
#!/bin/sh
set -eu
printf '%s\n' trace >$4
printf '%s\n' boot >$7
printf '%s\n' "$6" >$FN64_EXECUTABLE_IMAGE
SH
cat >$scratch/validator <<'SH'
#!/bin/sh
set -eu
captures=
while [ "$#" -gt 0 ]; do
    if [ "$1" = --capture ]; then
        shift
        if [ -z "$captures" ]; then
            reference=$1
        elif ! cmp -s "$reference" "$1"; then
            exit 9
        fi
        captures="$captures x"
    fi
    shift
done
[ "$(printf '%s' "$captures" | wc -w | tr -d ' ')" -ge 3 ]
printf '%s\n' '{"schema":"fn64.executable-image-group-receipt.v1","status":"validated","capture_count":3}'
SH
chmod +x $scratch/guard $scratch/producer-* $scratch/validator

invoke() {
    local producer=$1 output=$2
    FORBIDDEN_AMBIENT_VALUE=must-not-leak \
    FN64_CAPTURE_GROUP_SELFTEST_MODE=1 \
    FN64_CAPTURE_GROUP_SELFTEST_GUARD=$scratch/guard \
    FN64_CAPTURE_GROUP_SELFTEST_VALIDATOR=$scratch/validator \
    $script --producer $producer --core $scratch/core --rsp $scratch/rsp \
        --rom $scratch/rom --out-dir $output \
        --group-name FN64_EXECUTABLE_IMAGE_GENERAL_EXCEPTION \
        --image-id general-exception-preamble --capture-pc 0x80000180 \
        --first-pc 0x80000180 --start 0x80000180 --word-count 4 \
        --steps 400000 --timeout-seconds 30
}

invoke $scratch/producer-success $scratch/success >$scratch/success.receipt
[[ -s $scratch/success/group-receipt.json ]]
[[ $(find $scratch/success -name image.json | wc -l | tr -d ' ') == 3 ]]
[[ $(find $scratch/success -name boot-context.json | wc -l | tr -d ' ') == 3 ]]
[[ $(cat $scratch/success.receipt) == '{"schema":"fn64.executable-image-group-receipt.v1","status":"validated","capture_count":3}' ]]

if invoke $scratch/producer-unreached $scratch/unreached >$scratch/unreached.stdout 2>$scratch/unreached.stderr; then
    print -u2 -- "capture-group self-test: unreached image was accepted"
    exit 1
fi
[[ ! -e $scratch/unreached && ! -s $scratch/unreached.stdout ]]

if invoke $scratch/producer-divergent $scratch/divergent >$scratch/divergent.stdout 2>$scratch/divergent.stderr; then
    print -u2 -- "capture-group self-test: divergent group was accepted"
    exit 1
fi
[[ ! -e $scratch/divergent && ! -s $scratch/divergent.stdout ]]

grep -Fq '"$guard" cargo run -q -j1 -p fn64-discover --bin validate_executable_image_group --' $script
print -- "capture executable-image group synthetic contract tests: PASS"
