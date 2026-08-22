#!/bin/zsh
# Measure the synthetic wgpu parity corpus against Clang-instrumented RT64.
# This runs no ROM. A parity refusal is a hard measurement failure: startup
# coverage must never be reported as RDP-command coverage.
set -euo pipefail

ROOT=${0:A:h:h}
cd "$ROOT"

: ${FN64_RT64_DIR:=$HOME/Code/no-mercy-recompiled/third_party/rt64}
[[ -d "$FN64_RT64_DIR/src/hle" ]] || {
  print -u2 -- "[rt64-coverage] FATAL: RT64 source tree not found at $FN64_RT64_DIR"
  exit 1
}
export FN64_RT64_DIR

if [[ $(uname -s) != Darwin ]]; then
  print -u2 -- "[rt64-coverage] FATAL: this measurement drives the Metal parity oracle and requires macOS"
  exit 1
fi

CC=${CC:-$(xcrun --find clang)}
CXX=${CXX:-$(xcrun --find clang++)}
LLVM_PROFDATA=${LLVM_PROFDATA:-$(xcrun --find llvm-profdata)}
LLVM_COV=${LLVM_COV:-$(xcrun --find llvm-cov)}
export CC CXX

compiler_version=$($CXX --version | sed -n '1p')
profdata_version=$($LLVM_PROFDATA --version | sed -n '1p')
cov_version=$($LLVM_COV --version | sed -n '1p')
compiler_major=$(print -r -- "$compiler_version" | sed -E 's/.*version ([0-9]+).*/\1/')
profdata_major=$(print -r -- "$profdata_version" | sed -E 's/.*version ([0-9]+).*/\1/')
cov_major=$(print -r -- "$cov_version" | sed -E 's/.*version ([0-9]+).*/\1/')
if [[ "$compiler_major" != "$profdata_major" || "$compiler_major" != "$cov_major" ]]; then
  print -u2 -- "[rt64-coverage] FATAL: compiler/profile tool version mismatch"
  print -u2 -- "  compiler: $compiler_version"
  print -u2 -- "  profdata: $profdata_version"
  print -u2 -- "  llvm-cov: $cov_version"
  print -u2 -- "Use Xcode's xcrun clang++, llvm-profdata, and llvm-cov together; Homebrew LLVM often cannot read AppleClang profraw."
  exit 1
fi

OUTPUT_ROOT=${FN64_RT64_COVERAGE_OUTPUT:-$ROOT/target/rt64-coverage}
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target/rt64-coverage-cargo}
mkdir -p "$OUTPUT_ROOT/runs"
run_dir=$(mktemp -d "$OUTPUT_ROOT/runs/run.XXXXXX")
export CARGO_TARGET_DIR FN64_RT64_COVERAGE=1

print -- "[rt64-coverage] compiler: $compiler_version"
print -- "[rt64-coverage] profdata: $profdata_version"
print -- "[rt64-coverage] llvm-cov: $cov_version"
print -- "[rt64-coverage] build: $CARGO_TARGET_DIR"
print -- "[rt64-coverage] output: $run_dir"
find "$FN64_RT64_DIR/src/hle" -maxdepth 1 -type f \( -name '*.cpp' -o -name '*.h' \) -print | sort > "$run_dir/hle-denominator-52-files.txt"
hle_count=$(wc -l < "$run_dir/hle-denominator-52-files.txt" | tr -d ' ')
if [[ "$hle_count" != 52 ]]; then
  print -u2 -- "[rt64-coverage] FATAL: expected 52 pinned RT64 hle .cpp/.h files, found $hle_count"
  exit 1
fi

cargo build -p fn64-render-conformance --features parity-runner \
  --bin fn64-render-conformance-parity-runner --offline \
  2>&1 | tee "$run_dir/build.log"

runner="$CARGO_TARGET_DIR/debug/fn64-render-conformance-parity-runner"
[[ -x "$runner" ]] || {
  print -u2 -- "[rt64-coverage] FATAL: parity runner missing at $runner"
  exit 1
}

export LLVM_PROFILE_FILE="$run_dir/rt64-%p.profraw"
print -- "[rt64-coverage] LLVM_PROFILE_FILE=$LLVM_PROFILE_FILE"
set +e
"$runner" > "$run_dir/parity.json" 2> "$run_dir/runner.stderr"
runner_status=$?
set -e
if (( runner_status != 0 )); then
  print -u2 -- "[rt64-coverage] FATAL: parity runner exited $runner_status; see $run_dir/runner.stderr"
  exit "$runner_status"
fi
if ! python3 scripts/check_rt64_parity.py < "$run_dir/parity.json" | tee "$run_dir/parity-check.txt"; then
  if rg -q 'SDL video initialization failed' "$run_dir/parity.json"; then
    print -u2 -- "[rt64-coverage] BLOCKED: RT64 refused the corpus because SDL video initialization failed."
  else
    print -u2 -- "[rt64-coverage] FATAL: parity corpus did not complete authoritatively."
  fi
  print -u2 -- "No coverage percentage was produced from this invalid run; evidence is in $run_dir"
  exit 1
fi

profraw=("$run_dir"/*.profraw(N))
(( ${#profraw} > 0 )) || {
  print -u2 -- "[rt64-coverage] FATAL: no .profraw file landed in $run_dir"
  exit 1
}
"$LLVM_PROFDATA" merge -sparse "${profraw[@]}" -o "$run_dir/rt64.profdata"
"$LLVM_COV" export "$runner" -instr-profile="$run_dir/rt64.profdata" -summary-only \
  > "$run_dir/coverage-summary.json"

python3 - "$run_dir/coverage-summary.json" "$FN64_RT64_DIR" "$run_dir" <<'PY'
import json
import pathlib
import sys

summary_path, rt64_dir, run_dir = sys.argv[1:]
rt64_dir = str(pathlib.Path(rt64_dir).resolve())
run_dir = pathlib.Path(run_dir)
files = [entry["filename"] for entry in json.load(open(summary_path))["data"][0]["files"]]

def generated_overlay(name):
    base = pathlib.Path(name).name
    return "/rt64-cmake-build/" in name and base.startswith("fn64_")

narrow = sorted(name for name in files if
    f"{rt64_dir}/src/hle/" in name or
    (generated_overlay(name) and pathlib.Path(name).name in {
        "fn64_rt64_interpreter.cpp", "fn64_rt64_state.cpp", "fn64_rt64_vi.cpp"
    }))
whole = sorted(name for name in files if name.startswith(f"{rt64_dir}/") or generated_overlay(name))
for path, values in [
    (run_dir / "narrow-mapped-sources.txt", narrow),
    (run_dir / "whole-tree-mapped-sources.txt", whole),
]:
    path.write_text("".join(value + "\n" for value in values))
PY

narrow_sources=()
while IFS= read -r source; do
  narrow_sources+=("$source")
done < "$run_dir/narrow-mapped-sources.txt"
whole_sources=()
while IFS= read -r source; do
  whole_sources+=("$source")
done < "$run_dir/whole-tree-mapped-sources.txt"
"$LLVM_COV" report "$runner" -instr-profile="$run_dir/rt64.profdata" \
  --sources "${narrow_sources[@]}" | tee "$run_dir/narrow-report.txt"
"$LLVM_COV" report "$runner" -instr-profile="$run_dir/rt64.profdata" \
  --sources "${whole_sources[@]}" | tee "$run_dir/whole-tree-report.txt"
"$LLVM_COV" report "$runner" -instr-profile="$run_dir/rt64.profdata" \
  --show-functions --line-coverage-lt=100 --sources "${narrow_sources[@]}" \
  > "$run_dir/narrow-uncovered-functions.txt"
"$LLVM_COV" show "$runner" -instr-profile="$run_dir/rt64.profdata" \
  --show-line-counts-or-regions --sources "${narrow_sources[@]}" \
  > "$run_dir/narrow-show.txt"

print -- "[rt64-coverage] reports and exact source manifests: $run_dir"
