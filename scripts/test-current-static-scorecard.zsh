#!/bin/zsh

# ROM-free end-to-end tests for current-static-scorecard.zsh.

set -eu

typeset -r test_root=${0:A:h:h}
typeset -r test_dir=$(mktemp -d /private/tmp/fn64-current-scorecard-test.XXXXXX)
typeset -r driver=$test_dir/driver
typeset -r rom=$test_dir/fake-nwxe.z64
typeset -r boot=$test_dir/fake-boot-context.json
typeset -r inventory_output=$test_dir/inventory-output
typeset -r full_output=$test_dir/full-output
typeset -r frontier_failed_output=$test_dir/frontier-failed-output
typeset -r failed_output=$test_dir/failed-output
typeset -a captures

trap 'rm -rf -- "$test_dir"' EXIT
mkdir -m 700 $driver
print -n -- synthetic-rom >$rom
print -n -- synthetic-boot-context >$boot
for capture_index in {1..4}; do
    captures+=($test_dir/capture-$capture_index.json)
    print -n -- synthetic-capture-$capture_index >$captures[-1]
done

cat >$driver/fake-static-frontier <<'EOF'
#!/bin/zsh
set -eu
if [[ ${FN64_CURRENT_SCORECARD_SELFTEST_FAIL_FRONTIER:-} == 1 ]]; then
    exit 8
fi
[[ -z ${FN64_DISCOVER_NW4E_ROM:-} && -z ${FN64_DISCOVER_NWXE_DUMP:-} \
    && -z ${FN64_DISCOVER_OOT_ROM:-} && -z ${FN64_EXECUTABLE_IMAGES:-} \
    && -z ${FN64_EMIT_BLOCK_PROGRAM:-} ]]
if [[ ${FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS:-absent} == absent ]]; then
    [[ -z ${FN64_EXECUTABLE_IMAGE_GROUPS:-} ]]
else
    [[ $FN64_EXECUTABLE_IMAGE_GROUPS == $FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS \
        && $FN64_EXECUTABLE_IMAGE_TEST == $FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_TEST \
        && $FN64_EXECUTABLE_IMAGE_OTHER == $FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_OTHER ]]
fi
[[ ! -e $FN64_CLOSURE_AUDIT_DIR/producer.calls ]]
print -- 1 >$FN64_CLOSURE_AUDIT_DIR/producer.calls
print -n -- closure >$FN64_CLOSURE_AUDIT_DIR/nwxe.closure-audit-v3.json
print -n -- source >$FN64_SOURCE_FRONTIER_RECEIPT
print -n -- writers >$FN64_WRITER_CHANNEL_DENOMINATOR_RECEIPT
EOF

cat >$driver/fake-writer-audit <<'EOF'
#!/bin/zsh
set -eu
typeset rom= boot= output= timeout=
typeset -i groups=0 captures=0
while (( $# > 0 )); do
    case $1 in
        --rom) rom=$2; shift 2 ;;
        --boot-context) boot=$2; shift 2 ;;
        --output) output=$2; shift 2 ;;
        --max-build-seconds) timeout=$2; shift 2 ;;
        --image-group)
            [[ $2 == FN64_EXECUTABLE_IMAGE_TEST || $2 == FN64_EXECUTABLE_IMAGE_OTHER ]]
            (( groups += 1 ))
            shift 2
            while (( $# > 0 )) && [[ $1 != --* ]]; do
                [[ -f $1 ]]
                (( captures += 1 ))
                shift
            done
            ;;
        *) exit 2 ;;
    esac
done
[[ -f $rom && -f $boot && $output == /* && ! -e $output \
    && $timeout == 3000 && $groups == 2 && $captures == 7 \
    && $FN64_GUARD_MAX_RSS_MIB == 4096 ]]
[[ ! -e ${output:h}/writer.calls ]]
print -- 'writer-progress mode=authority phase=build state=start'
print -- 'writer-progress mode=authority phase=build state=complete elapsed_ms=1'
print -- 1 >${output:h}/writer.calls
mkdir -m 700 $output
if [[ ${FN64_CURRENT_SCORECARD_SELFTEST_FAIL_WRITER:-} == 1 ]]; then
    mkdir -m 700 $output/diagnostics
    print -n -- bounded-private-diagnostic >$output/diagnostics/pi.log
    print -n -- partial-writers >$output/partial-writers.json
    print -n -- partial-diagnostic-only >$output/partial-writer-audit.json
    print -- 'writer-progress mode=authority channel=pi phase=series runs=10 state=fail elapsed_ms=1'
    exit 9
fi
print -n -- writers-complete >$output/writers.json
print -n -- audit-complete >$output/writer-audit.json
EOF

cat >$driver/fake-scorecard <<'EOF'
#!/bin/zsh
set -eu
typeset seen_current=0 seen_ack=0 seen_json=0 seen_writer_audit=0
while (( $# > 0 )); do
    case $1 in
        --closure-audit|--source-frontier)
            [[ -f $2 ]]
            shift 2
            ;;
        --writer-denominator)
            [[ -f $2 && $(<$2) == ${FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_WRITER:?} ]]
            shift 2
            ;;
        --writer-audit)
            [[ -f $2 && $(<$2) == audit-complete ]]
            seen_writer_audit=1
            shift 2
            ;;
        --evidence-label)
            [[ $2 == current ]]
            seen_current=1
            shift 2
            ;;
        --ack-current-is-caller-attested)
            seen_ack=1
            shift
            ;;
        --format)
            [[ $2 == json ]]
            seen_json=1
            shift 2
            ;;
        *) exit 2 ;;
    esac
done
(( seen_current && seen_ack && seen_json \
    && seen_writer_audit == ${FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_AUDIT:?} ))
print -- '{"schema":"fn64.static-recomp-scorecard.v1","evidence_label":"current","completion_claim":false}'
EOF
chmod 700 $driver/fake-*

export FN64_DISCOVER_NWXE_ROM=$rom
export FN64_BOOT_CONTEXT=$boot
export FN64_CURRENT_SCORECARD_SELFTEST_MODE=1
export FN64_CURRENT_SCORECARD_SELFTEST_DRIVER=$driver
export FN64_DISCOVER_NW4E_ROM=/should/be/absent
export FN64_DISCOVER_NWXE_DUMP=/should/be/absent
export FN64_DISCOVER_OOT_ROM=/should/be/absent
export FN64_EXECUTABLE_IMAGE_GROUPS=SHOULD_BE_ABSENT
export FN64_EXECUTABLE_IMAGES=/should/be/absent
export FN64_EXECUTABLE_IMAGE_TEST=/should/be/absent
export FN64_EXECUTABLE_IMAGE_OTHER=/should/be/absent
export FN64_EMIT_BLOCK_PROGRAM=/should/be/absent

typeset dry_output
dry_output=$("$test_root/scripts/current-static-scorecard.zsh" --dry-run --output $inventory_output)
[[ $dry_output == *'output=<PRIVATE>'* && $dry_output == *'discovery_passes=1'* \
    && $dry_output != *$test_dir* && ! -e $inventory_output ]]

typeset run_output
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_WRITER=writers
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_AUDIT=0
run_output=$("$test_root/scripts/current-static-scorecard.zsh" --output $inventory_output)
[[ $run_output == *'"evidence_label":"current"'* && $run_output != *$test_dir* ]]
[[ -f $inventory_output/nwxe.closure-audit-v3.json \
    && -f $inventory_output/source.json && -f $inventory_output/writers.json \
    && -f $inventory_output/scorecard.json \
    && $(<$inventory_output/producer.calls) == 1 \
    && ! -e $inventory_output/writer.calls ]]
typeset output_mode
if output_mode=$(stat -c '%a' -- $inventory_output 2>/dev/null); then
    [[ $output_mode == 700 ]]
else
    [[ $(stat -f '%Lp' -- $inventory_output) == 700 ]]
fi

if "$test_root/scripts/current-static-scorecard.zsh" --output $inventory_output >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted an existing output directory"
    exit 1
fi

typeset full_dry_output
full_dry_output=$("$test_root/scripts/current-static-scorecard.zsh" \
    --dry-run --full-writer-audit \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "${captures[@]}" \
    --image-group FN64_EXECUTABLE_IMAGE_OTHER "$captures[1]" "$captures[2]" "$captures[3]" \
    --max-build-seconds 3000 --output $full_output)
[[ $full_dry_output == *'stage=full_writer_audit'* \
    && $full_dry_output == *'selected_build_cargo_jobs=2'* \
    && $full_dry_output == *'max_rss_mib=4096'* \
    && $full_dry_output == *'image_groups=2'* && ! -e $full_output ]]

export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_WRITER=writers-complete
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_AUDIT=1
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS=FN64_EXECUTABLE_IMAGE_TEST,FN64_EXECUTABLE_IMAGE_OTHER
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_TEST=${(j/:/)captures}
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_OTHER=${(j/:/)captures[1,3]}
typeset full_run_output
full_run_output=$("$test_root/scripts/current-static-scorecard.zsh" \
    --full-writer-audit \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "${captures[@]}" \
    --image-group FN64_EXECUTABLE_IMAGE_OTHER "$captures[1]" "$captures[2]" "$captures[3]" \
    --max-build-seconds 3000 --output $full_output)
[[ $full_run_output == *'"evidence_label":"current"'* \
    && $full_run_output == *'stage 1/3 closure, source, and writer producer'* \
    && $full_run_output == *'writer-progress mode=authority phase=build state=start'* \
    && $full_run_output != *$test_dir* \
    && $(<$full_output/producer.calls) == 1 \
    && $(<$full_output/writer.calls) == 1 \
    && $(<$full_output/writers.json) == writers \
    && $(<$full_output/writer-progress.log) == *'phase=build state=complete elapsed_ms=1'* \
    && $(<$full_output/full-writer-audit/writers.json) == writers-complete \
    && -f $full_output/full-writer-audit/writer-audit.json ]]
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_TEST
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_OTHER

export FN64_CURRENT_SCORECARD_SELFTEST_FAIL_FRONTIER=1
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS=FN64_EXECUTABLE_IMAGE_TEST,FN64_EXECUTABLE_IMAGE_OTHER
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_TEST=${(j/:/)captures}
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_OTHER=${(j/:/)captures[1,3]}
if "$test_root/scripts/current-static-scorecard.zsh" \
    --full-writer-audit \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "${captures[@]}" \
    --image-group FN64_EXECUTABLE_IMAGE_OTHER "$captures[1]" "$captures[2]" "$captures[3]" \
    --max-build-seconds 3000 --output $frontier_failed_output >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted a failed static frontier"
    exit 1
fi
unset FN64_CURRENT_SCORECARD_SELFTEST_FAIL_FRONTIER
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_TEST
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_OTHER
[[ -f $frontier_failed_output/frontier.log \
    && ! -e $frontier_failed_output/writer.calls \
    && ! -e $frontier_failed_output/full-writer-audit \
    && ! -e $frontier_failed_output/scorecard.json ]]

if "$test_root/scripts/current-static-scorecard.zsh" \
    --full-writer-audit --output $test_dir/no-groups >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: full writer mode accepted no image groups"
    exit 1
fi
if "$test_root/scripts/current-static-scorecard.zsh" \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "${captures[@]}" \
    --output $test_dir/no-full-mode >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: inventory mode accepted image groups"
    exit 1
fi
if "$test_root/scripts/current-static-scorecard.zsh" \
    --full-writer-audit \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "$captures[1]" "$captures[2]" "$captures[3]" \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "$captures[1]" "$captures[2]" "$captures[3]" \
    --output $test_dir/duplicate-group >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted a duplicate image-group name"
    exit 1
fi

typeset -r colon_capture=$test_dir/capture:colon.json
print -n -- synthetic-capture-colon >$colon_capture
if "$test_root/scripts/current-static-scorecard.zsh" \
    --full-writer-audit \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "$captures[1]" "$captures[2]" $colon_capture \
    --output $test_dir/colon-group >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted a capture path unrepresentable by the static-frontier path-list wire"
    exit 1
fi

export FN64_CURRENT_SCORECARD_SELFTEST_FAIL_WRITER=1
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS=FN64_EXECUTABLE_IMAGE_TEST,FN64_EXECUTABLE_IMAGE_OTHER
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_TEST=${(j/:/)captures}
export FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_OTHER=${(j/:/)captures[1,3]}
if "$test_root/scripts/current-static-scorecard.zsh" \
    --full-writer-audit \
    --image-group FN64_EXECUTABLE_IMAGE_TEST "${captures[@]}" \
    --image-group FN64_EXECUTABLE_IMAGE_OTHER "$captures[1]" "$captures[2]" "$captures[3]" \
    --max-build-seconds 3000 --output $failed_output >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted a failed writer audit"
    exit 1
fi
unset FN64_CURRENT_SCORECARD_SELFTEST_FAIL_WRITER
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_GROUPS
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_TEST
unset FN64_CURRENT_SCORECARD_SELFTEST_EXPECT_FRONTIER_OTHER
[[ -f $failed_output/nwxe.closure-audit-v3.json \
    && -f $failed_output/full-writer-audit/partial-writer-audit.json \
    && -f $failed_output/full-writer-audit/partial-writers.json \
    && -f $failed_output/full-writer-audit/diagnostics/pi.log \
    && ! -e $failed_output/full-writer-audit/writers.json \
    && ! -e $failed_output/full-writer-audit/writer-audit.json \
    && ! -e $failed_output/scorecard.json ]]
if FN64_DISCOVER_NWXE_ROM=relative.z64 \
    "$test_root/scripts/current-static-scorecard.zsh" --output $test_dir/relative >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted a relative ROM input"
    exit 1
fi
if FN64_BOOT_CONTEXT=$test_dir/missing.json \
    "$test_root/scripts/current-static-scorecard.zsh" --output $test_dir/missing >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted a missing BootContext"
    exit 1
fi
if "$test_root/scripts/current-static-scorecard.zsh" \
    --dry-run --output $test_root/.fn64-current-scorecard-selftest-output >/dev/null 2>&1; then
    print -u2 -- "current scorecard self-test: accepted an output inside the repository"
    exit 1
fi

print -- "current static scorecard self-test: PASS"
