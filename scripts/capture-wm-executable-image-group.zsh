#!/bin/zsh

# Acquire and validate one independently repeated executable-image group using
# only the public mupen debugger producer interface. Private artifacts remain
# in one create-new directory outside the repository.

set -eu
set -o pipefail

typeset -r script_path=$0
typeset -r repo_root=${0:A:h:h}
typeset -r default_runs=3
typeset producer= core= rsp= rom= out_dir= group_name= image_id=
typeset capture_pc_text= first_pc_text= start_text= word_count_text=
typeset steps_text= timeout_text= runs_text=$default_runs

usage() {
    print -u2 -- "usage: $script_path --producer ABS --core ABS --rsp ABS --rom ABS --out-dir ABS_NEW --group-name FN64_EXECUTABLE_IMAGE_NAME --image-id ID --capture-pc U32 --first-pc U32 --start U32 --word-count N --steps N --timeout-seconds N [--runs N]"
    print -u2 -- "       $script_path --selftest"
}

fail() {
    print -u2 -- "capture executable-image group: $1"
    exit 2
}

set_once() {
    local name=$1 value=$2
    [[ -z ${(P)name} ]] || fail "an option was supplied more than once"
    typeset -g "$name=$value"
}

parse_u32() {
    local text=$1
    if [[ $text =~ '^0[xX][0-9a-fA-F]+$' ]]; then
        local digits=${text[3,-1]}
        local value=$(( 16#$digits ))
        (( value <= 4294967295 )) || return 1
        print -- $value
    elif [[ $text =~ '^[0-9]+$' ]]; then
        local value=$(( 10#$text ))
        (( value <= 4294967295 )) || return 1
        print -- $value
    else
        return 1
    fi
}

if [[ ${1:-} == --selftest ]]; then
    (( $# == 1 )) || { usage; exit 2; }
    exec "$repo_root/scripts/test-capture-wm-executable-image-group.zsh"
fi

while (( $# > 0 )); do
    (( $# >= 2 )) || { usage; exit 2; }
    option=$1
    value=$2
    shift 2
    case $option in
        --producer) set_once producer $value ;;
        --core) set_once core $value ;;
        --rsp) set_once rsp $value ;;
        --rom) set_once rom $value ;;
        --out-dir) set_once out_dir $value ;;
        --group-name) set_once group_name $value ;;
        --image-id) set_once image_id $value ;;
        --capture-pc) set_once capture_pc_text $value ;;
        --first-pc) set_once first_pc_text $value ;;
        --start) set_once start_text $value ;;
        --word-count) set_once word_count_text $value ;;
        --steps) set_once steps_text $value ;;
        --timeout-seconds) set_once timeout_text $value ;;
        --runs) runs_text=$value ;;
        *) fail "unknown option" ;;
    esac
done

for required in producer core rsp rom out_dir group_name image_id capture_pc_text \
    first_pc_text start_text word_count_text steps_text timeout_text; do
    [[ -n ${(P)required} ]] || fail "a required option is missing"
done
for input in $producer $core $rsp $rom; do
    [[ $input == /* && -f $input && ! -L $input ]] || fail "every input must be an absolute regular non-symlink file"
done
[[ -x $producer ]] || fail "producer must be executable"
[[ $group_name =~ '^FN64_EXECUTABLE_IMAGE_[A-Z0-9_]+$' ]] || fail "group name must be an FN64_EXECUTABLE_IMAGE_* token"
[[ ${#image_id} -le 128 && $image_id =~ '^[A-Za-z0-9._:-]+$' ]] || fail "image ID must be a 1-128 character portable identifier"
[[ $word_count_text =~ '^[0-9]+$' && $word_count_text -ge 1 && $word_count_text -le 262144 ]] || fail "word count must be 1..262144"
[[ $steps_text =~ '^[0-9]+$' && $steps_text -ge 1 && $steps_text -le 100000000 ]] || fail "steps must be 1..100000000"
[[ $timeout_text =~ '^[0-9]+$' && $timeout_text -ge 1 && $timeout_text -le 7200 ]] || fail "timeout must be 1..7200 seconds"
[[ $runs_text =~ '^[0-9]+$' && $runs_text -ge 3 && $runs_text -le 100 ]] || fail "runs must be 3..100"

capture_pc=$(parse_u32 $capture_pc_text) || fail "capture PC must be a 32-bit decimal or 0x value"
first_pc=$(parse_u32 $first_pc_text) || fail "first PC must be a 32-bit decimal or 0x value"
image_start=$(parse_u32 $start_text) || fail "start must be a 32-bit decimal or 0x value"
(( capture_pc % 4 == 0 && first_pc % 4 == 0 && image_start % 4 == 0 )) || fail "capture addresses must be four-byte aligned"
image_end=$(( image_start + word_count_text * 4 ))
(( image_end <= 4294967296 && first_pc >= image_start && first_pc < image_end )) || fail "image range overflows or does not contain first PC"

[[ $out_dir == /* ]] || fail "output directory must be absolute"
out_parent=${out_dir:h}
out_leaf=${out_dir:t}
[[ -d $out_parent && ! -L $out_parent && $out_leaf != '.' && $out_leaf != '..' ]] || fail "output parent must be an existing non-symlink directory"
out_parent=${out_parent:A}
out_dir=$out_parent/$out_leaf
case $out_dir in
    $repo_root|$repo_root/*) fail "output directory must be outside the fn64 worktree" ;;
esac
[[ ! -e $out_dir && ! -L $out_dir ]] || fail "output directory already exists"

typeset -r guard=${FN64_CAPTURE_GROUP_SELFTEST_GUARD:-$repo_root/scripts/memory-guard.zsh}
typeset -r fake_validator=${FN64_CAPTURE_GROUP_SELFTEST_VALIDATOR:-}
if [[ -n ${FN64_CAPTURE_GROUP_SELFTEST_MODE:-} ]]; then
    [[ $FN64_CAPTURE_GROUP_SELFTEST_MODE == 1 && -x $guard && -x $fake_validator ]] || fail "invalid self-test injection"
elif [[ -n $fake_validator || ${FN64_CAPTURE_GROUP_SELFTEST_GUARD:-} != '' ]]; then
    fail "self-test injection requires self-test mode"
else
    [[ -x $guard ]] || fail "memory guard is unavailable"
fi

export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-2048}
export FN64_GUARD_MIN_FREE_PERCENT=${FN64_GUARD_MIN_FREE_PERCENT:-40}
export CARGO_BUILD_JOBS=1

mkdir -m 700 -- $out_dir || fail "could not create private output directory"
typeset -i completed=0
cleanup() {
    local exit_code=$?
    trap - EXIT HUP INT TERM
    if (( ! completed )); then
        rm -rf -- $out_dir
    fi
    return $exit_code
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

typeset -a captures
typeset -i run
for (( run = 1; run <= runs_text; run++ )); do
    run_dir=$out_dir/run-$run
    tmp_dir=$run_dir/tmp
    mkdir -m 700 -- $run_dir $tmp_dir
    trace_path=$run_dir/trace.jsonl
    boot_path=$run_dir/boot-context.json
    image_path=$run_dir/image.json
    trace_id="capture-$run"
    set +e
    FN64_GUARD_MAX_SECONDS=$timeout_text "$guard" env -i \
        "PATH=$PATH" "TMPDIR=$tmp_dir" \
        "FN64_EXECUTABLE_IMAGE_PC=$capture_pc" \
        "FN64_EXECUTABLE_IMAGE_FIRST_PC=$first_pc" \
        "FN64_EXECUTABLE_IMAGE_START=$image_start" \
        "FN64_EXECUTABLE_IMAGE_WORDS=$word_count_text" \
        "FN64_EXECUTABLE_IMAGE_ID=$image_id" \
        "FN64_EXECUTABLE_IMAGE=$image_path" \
        FN64_CAPTURE_ONLY=1 FN64_STOP_AFTER_IMAGE=1 \
        "$producer" "$core" "$rom" "$rsp" "$trace_path" "$steps_text" "$trace_id" "$boot_path" \
        >$run_dir/producer.stdout 2>$run_dir/producer.log
    producer_status=$?
    set -e
    (( producer_status == 0 )) || fail "a guarded producer run failed or timed out"
    [[ -s $image_path && -s $boot_path ]] || fail "a producer run omitted the image or BootContext"
    captures+=($image_path)
    rm -rf -- $tmp_dir
done

typeset -a validator_args
validator_args=(
    --rom $rom --group-name $group_name --image-id $image_id
    --capture-pc $capture_pc --first-pc $first_pc --start $image_start
    --word-count $word_count_text
)
for image_path in $captures; do
    validator_args+=(--capture $image_path)
done
receipt_tmp=$out_dir/group-receipt.tmp
validator_log=$out_dir/validator.log
set +e
if [[ -n $fake_validator ]]; then
    "$fake_validator" "${validator_args[@]}" >$receipt_tmp 2>$validator_log
else
    "$guard" cargo run -q -j1 -p fn64-discover --bin validate_executable_image_group -- "${validator_args[@]}" >$receipt_tmp 2>$validator_log
fi
validator_status=$?
set -e
(( validator_status == 0 )) || fail "canonical capture-group validation failed"
[[ -s $receipt_tmp ]] || fail "capture-group validator emitted no receipt"
mv -- $receipt_tmp $out_dir/group-receipt.json
completed=1
trap - EXIT HUP INT TERM
command cat $out_dir/group-receipt.json
