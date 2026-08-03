#!/bin/zsh

set -euo pipefail

script=${0:A:h}/run-black-box-trace.zsh
scratch=$(mktemp -d /tmp/fn64-black-box-trace-test.XXXXXX)
scratch=${scratch:A}
cleanup_test() {
  local exit_code=$?
  trap - EXIT HUP INT TERM
  rm -rf -- "$scratch"
  return $exit_code
}
trap cleanup_test EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

touch "$scratch/core" "$scratch/rsp" "$scratch/rom"

make_success_tools() {
  local producer=$1 discover=$2
  cat > "$producer" <<'SH'
#!/bin/sh
set -eu
printf '%s\n' "$@" > "$(dirname "$4")/producer.args"
printf '%s\n' '{"event":"header"}' > "$4"
printf '%s\n' '{"boot":"context"}' > "$7"
SH
  cat > "$discover" <<'SH'
#!/bin/sh
set -eu
trace=$3
printf '%s\n' "$@" > "$(dirname "$trace")/discover.args"
printf '%s\n' '{"summary":{"schema_version":1},"receipt_sha256":"synthetic"}'
SH
  chmod +x "$producer" "$discover"
}

invoke() {
  "$script" \
    --producer "$1" --discover "$2" \
    --core "$scratch/core" --rsp "$scratch/rsp" --rom "$scratch/rom" \
    --trace-id synthetic-1 --steps 17 --timeout-seconds "${4:-5}" --out-dir "$3"
}

producer="$scratch/producer"
discover="$scratch/discover"
make_success_tools "$producer" "$discover"
out="$scratch/success"
invoke "$producer" "$discover" "$out"
[[ -f "$out/trace.jsonl" && -f "$out/boot-context.json" ]]
[[ -f "$out/discovery-summary.json" && -f "$out/pipeline-receipt.txt" ]]
expected_producer=$(printf '%s\n' "$scratch/core" "$scratch/rom" "$scratch/rsp" \
  "$out/trace.jsonl" 17 synthetic-1 "$out/boot-context.json")
[[ "$(cat "$out/producer.args")" == "$expected_producer" ]]
expected_discover=$(printf '%s\n' "$scratch/rom" --trace "$out/trace.jsonl" --summary)
[[ "$(cat "$out/discover.args")" == "$expected_discover" ]]

existing="$scratch/existing"
mkdir "$existing"
print -r -- untouched > "$existing/marker"
if invoke "$producer" "$discover" "$existing" >"$scratch/refusal.stdout" 2>"$scratch/refusal.stderr"; then
  print -u2 -- "existing output directory was accepted"
  exit 1
fi
[[ "$(cat "$existing/marker")" == untouched ]]
[[ ! -s "$scratch/refusal.stdout" ]]

relative_out=relative-output
if "$script" --producer relative --discover "$discover" --core "$scratch/core" \
  --rsp "$scratch/rsp" --rom "$scratch/rom" --trace-id synthetic-1 --steps 1 \
  --timeout-seconds 1 --out-dir "$scratch/relative" >/dev/null 2>&1; then
  print -u2 -- "relative input path was accepted"
  exit 1
fi
[[ ! -e "$scratch/relative" ]]

inside_repo=${script:A:h:h}/.black-box-trace-refusal
[[ ! -e "$inside_repo" ]]
if invoke "$producer" "$discover" "$inside_repo" >/dev/null 2>&1; then
  print -u2 -- "in-worktree output directory was accepted"
  exit 1
fi
[[ ! -e "$inside_repo" ]]

failing="$scratch/failing-producer"
cat > "$failing" <<'SH'
#!/bin/sh
printf '%s\n' partial > "$4"
exit 9
SH
chmod +x "$failing"
failed_out="$scratch/failed"
if invoke "$failing" "$discover" "$failed_out" >"$scratch/failure.stdout" 2>/dev/null; then
  print -u2 -- "producer failure was accepted"
  exit 1
fi
[[ ! -e "$failed_out" && ! -s "$scratch/failure.stdout" ]]

sleeping="$scratch/sleeping-producer"
cat > "$sleeping" <<'SH'
#!/bin/sh
sleep 10
SH
chmod +x "$sleeping"
timed_out="$scratch/timed-out"
if invoke "$sleeping" "$discover" "$timed_out" 1 >/dev/null 2>&1; then
  print -u2 -- "producer timeout was accepted"
  exit 1
fi
[[ ! -e "$timed_out" ]]

mutating_discover="$scratch/mutating-discover"
cat > "$mutating_discover" <<'SH'
#!/bin/sh
printf '%s\n' mutation >> "$3"
printf '%s\n' '{"summary":{},"receipt_sha256":"synthetic"}'
SH
chmod +x "$mutating_discover"
mutated_out="$scratch/mutated"
if invoke "$producer" "$mutating_discover" "$mutated_out" >/dev/null 2>&1; then
  print -u2 -- "trace mutation during ingest was accepted"
  exit 1
fi
[[ ! -e "$mutated_out" ]]

print -r -- "run-black-box-trace synthetic contract tests: ok"
