#!/bin/zsh
# Produce and ingest one bounded, canonical fn64-discover trace through the
# public mupen64plus debugger API. All ROM-derived output stays in one
# create-new directory outside the fn64 worktree.

set -euo pipefail

usage() {
  print -u2 -- "usage: run-black-box-trace.zsh --producer /abs/mupen_trace --discover /abs/fn64-discover --core /abs/libmupen64plus --rsp /abs/rsp-plugin --rom /abs/game.z64 --trace-id ID --steps N --timeout-seconds N --out-dir /abs/new-dir"
}

fail() {
  print -u2 -- "run-black-box-trace: $1"
  exit 2
}

typeset producer="" discover="" core="" rsp="" rom="" trace_id=""
typeset steps="" timeout_seconds="" out_dir=""
while (( $# > 0 )); do
  option=$1
  shift
  (( $# > 0 )) || fail "$option requires a value"
  value=$1
  shift
  case "$option" in
    --producer) [[ -z "$producer" ]] || fail "--producer supplied more than once"; producer=$value ;;
    --discover) [[ -z "$discover" ]] || fail "--discover supplied more than once"; discover=$value ;;
    --core) [[ -z "$core" ]] || fail "--core supplied more than once"; core=$value ;;
    --rsp) [[ -z "$rsp" ]] || fail "--rsp supplied more than once"; rsp=$value ;;
    --rom) [[ -z "$rom" ]] || fail "--rom supplied more than once"; rom=$value ;;
    --trace-id) [[ -z "$trace_id" ]] || fail "--trace-id supplied more than once"; trace_id=$value ;;
    --steps) [[ -z "$steps" ]] || fail "--steps supplied more than once"; steps=$value ;;
    --timeout-seconds) [[ -z "$timeout_seconds" ]] || fail "--timeout-seconds supplied more than once"; timeout_seconds=$value ;;
    --out-dir) [[ -z "$out_dir" ]] || fail "--out-dir supplied more than once"; out_dir=$value ;;
    *) fail "unknown option" ;;
  esac
done

for input in "$producer" "$discover" "$core" "$rsp" "$rom"; do
  [[ "$input" == /* ]] || fail "all input paths must be absolute"
  [[ -f "$input" && ! -L "$input" ]] || fail "an input is not a regular non-symlink file"
done
[[ -x "$producer" && -x "$discover" ]] || fail "producer and discover must be executable"
[[ "$out_dir" == /* ]] || fail "--out-dir must be absolute"
[[ -n "$trace_id" && ${#trace_id} -le 128 ]] ||
  fail "trace ID must be 1-128 portable identifier characters"
case "$trace_id" in
  [!A-Za-z0-9]*|*[!A-Za-z0-9._:-]*) fail "trace ID must be 1-128 portable identifier characters" ;;
esac
[[ "$steps" == <1-100000000> ]] || fail "steps must be an integer from 1 through 100000000"
[[ "$timeout_seconds" == <1-3600> ]] || fail "timeout must be an integer from 1 through 3600 seconds"

script_dir=${0:A:h}
repo_root=${script_dir:h}
out_parent=${out_dir:h}
out_leaf=${out_dir:t}
[[ -d "$out_parent" && ! -L "$out_parent" ]] || fail "output parent must be an existing non-symlink directory"
out_parent=${out_parent:A}
out_dir="$out_parent/$out_leaf"
case "$out_dir" in
  "$repo_root"|"$repo_root"/*) fail "output directory must be outside the fn64 worktree" ;;
esac
[[ ! -e "$out_dir" && ! -L "$out_dir" ]] || fail "output directory already exists"

hash_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -- "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "$1" | awk '{print $1}'
  else
    fail "no SHA-256 utility is available"
  fi
}

# The Python standard library supplies a portable process-group timeout on
# macOS and Linux. Child stdout/stderr go only to private files in out_dir.
run_bounded() {
  local stdout_path=$1 stderr_path=$2
  shift 2
  python3 - "$timeout_seconds" "$stdout_path" "$stderr_path" "$@" <<'PY'
import os
import signal
import subprocess
import sys

timeout = int(sys.argv[1])
stdout_path, stderr_path = sys.argv[2:4]
argv = sys.argv[4:]
with open(stdout_path, "xb") as stdout, open(stderr_path, "xb") as stderr:
    try:
        child = subprocess.Popen(argv, stdout=stdout, stderr=stderr, start_new_session=True)
    except OSError as error:
        stderr.write(("launch failed: " + error.strerror + "\n").encode())
        raise SystemExit(125)
    try:
        raise SystemExit(child.wait(timeout=timeout))
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGTERM)
        try:
            child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
        raise SystemExit(124)
PY
}

mkdir -m 700 -- "$out_dir" || fail "could not create output directory"
typeset completed=0
cleanup() {
  local exit_code=$?
  trap - EXIT HUP INT TERM
  if (( ! completed )); then
    # out_dir was resolved, checked outside the worktree, and created by this
    # process only after an atomic absence check.
    rm -rf -- "$out_dir"
  fi
  return $exit_code
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -m 700 -- "$out_dir/tmp"

typeset -A before
before[producer]=$(hash_file "$producer")
before[discover]=$(hash_file "$discover")
before[core]=$(hash_file "$core")
before[rsp]=$(hash_file "$rsp")
before[rom]=$(hash_file "$rom")

trace="$out_dir/trace.jsonl"
boot_context="$out_dir/boot-context.json"
producer_log="$out_dir/producer.log"
producer_stdout="$out_dir/producer.stdout"
typeset -a producer_command
producer_command=(
  env -i "PATH=$PATH" "TMPDIR=$out_dir/tmp"
  "$producer" "$core" "$rom" "$rsp" "$trace" "$steps" "$trace_id" "$boot_context"
)
if [[ -n ${FN64_FAST_FORWARD_PC:-} ]]; then
  producer_command=(
    env -i "PATH=$PATH" "TMPDIR=$out_dir/tmp" "FN64_FAST_FORWARD_PC=$FN64_FAST_FORWARD_PC"
    "$producer" "$core" "$rom" "$rsp" "$trace" "$steps" "$trace_id" "$boot_context"
  )
fi
set +e
run_bounded "$producer_stdout" "$producer_log" "${producer_command[@]}"
producer_status=$?
set -e
(( producer_status == 0 )) || fail "producer failed or exceeded its timeout; private diagnostics were removed"
[[ -s "$trace" && -s "$boot_context" ]] || fail "producer omitted a required output"

for input_name input_path in producer "$producer" core "$core" rsp "$rsp" rom "$rom"; do
  [[ "${before[$input_name]}" == "$(hash_file "$input_path")" ]] || fail "a producer input changed during capture"
done
trace_sha256=$(hash_file "$trace")

summary="$out_dir/discovery-summary.json"
discover_log="$out_dir/discover.log"
typeset -a discover_command
discover_command=(
  env -i "PATH=$PATH" "TMPDIR=$out_dir/tmp"
  "$discover" "$rom" --trace "$trace" --summary
)
set +e
run_bounded "$summary" "$discover_log" "${discover_command[@]}"
discover_status=$?
set -e
(( discover_status == 0 )) || fail "trace normalization/ingestion failed or exceeded its timeout; private diagnostics were removed"
[[ -s "$summary" ]] || fail "discover omitted its summary receipt"
[[ "$trace_sha256" == "$(hash_file "$trace")" ]] || fail "trace changed while it was being ingested"
[[ "${before[discover]}" == "$(hash_file "$discover")" ]] || fail "discover executable changed during ingestion"
[[ "${before[rom]}" == "$(hash_file "$rom")" ]] || fail "ROM changed during ingestion"

# The trace header's normalized-ROM digest is checked by fn64-discover before
# it folds any observation. Reaching this receipt therefore binds the captured
# producer stream to the normalized ROM without exposing that digest here.
{
  print -r -- "schema=fn64.black-box-trace-pipeline.v1"
  print -r -- "trace_sha256=$trace_sha256"
  print -r -- "producer_sha256=${before[producer]}"
  print -r -- "discover_sha256=${before[discover]}"
  print -r -- "core_sha256=${before[core]}"
  print -r -- "rsp_sha256=${before[rsp]}"
} > "$out_dir/pipeline-receipt.txt"

rm -rf -- "$out_dir/tmp"
completed=1
trap - EXIT HUP INT TERM
