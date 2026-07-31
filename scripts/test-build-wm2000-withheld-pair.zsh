#!/bin/zsh

# ROM-free contract tests for the pair builder. Fake tools record only
# feature/order facts and create synthetic executables in Cargo's target dir.

set -eu

typeset -r test_root=${0:A:h:h}
typeset -r test_dir=$(mktemp -d /private/tmp/fn64-wm-pair-build-test.XXXXXX)
trap 'rm -rf -- "$test_dir"' EXIT
typeset -r fake_bin=$test_dir/bin
typeset -r private_inputs="$test_dir/private inputs"
mkdir -p "$fake_bin" "$private_inputs"
typeset -r test_rom=$private_inputs/private-rom.z64
typeset -r test_boot=$private_inputs/private-boot.json
typeset -r test_capture_a=$private_inputs/private-capture-a.json
typeset -r test_capture_b=$private_inputs/private-capture-b.json
typeset -r test_capture_c=$private_inputs/private-capture-c.json
print -n -- rom > "$test_rom"
print -n -- boot > "$test_boot"
print -n -- capture-a > "$test_capture_a"
print -n -- capture-b > "$test_capture_b"
print -n -- capture-c > "$test_capture_c"

cat > "$fake_bin/cargo" <<'EOF'
#!/bin/zsh
set -eu
[[ ${0:t} == cargo ]] || {
    print -u2 -- "fake rustup proxy must be invoked through its cargo symlink"
    exit 97
}
if [[ "${1:-}" == tree ]]; then
    print -r -- "cargo-tree $*" >> "$FN64_PAIR_TEST_ORDER"
    if [[ " $* " == *" --features dynamic-withheld "* ]]; then
        if [[ "${FN64_PAIR_TEST_DYNAMIC_GRAPH_FAIL:-}" != missing-aot-runtime ]]; then
            print -r -- 'fn64-recomp-rs feature "aot-runtime"'
        fi
        if [[ "${FN64_PAIR_TEST_DYNAMIC_GRAPH_FAIL:-}" != missing-production ]]; then
            print -r -- 'fn64-recomp-rs feature "production-aot"'
        fi
        if [[ "${FN64_PAIR_TEST_DYNAMIC_GRAPH_FAIL:-}" == dev ]]; then
            print -r -- 'fn64-recomp-rs feature "dev-interpreter"'
        elif [[ "${FN64_PAIR_TEST_DYNAMIC_GRAPH_FAIL:-}" != missing-runtime ]]; then
            print -r -- 'fn64-recomp-rs feature "dynamic-mapped-runtime"'
        fi
    else
        if [[ "${FN64_PAIR_TEST_AOT_GRAPH_FAIL:-}" != missing-aot-runtime ]]; then
            print -r -- 'fn64-recomp-rs feature "aot-runtime"'
        fi
        if [[ "${FN64_PAIR_TEST_AOT_GRAPH_FAIL:-}" != missing-production ]]; then
            print -r -- 'fn64-recomp-rs feature "production-aot"'
        fi
        if [[ "${FN64_PAIR_TEST_AOT_GRAPH_FAIL:-}" == dynamic ]]; then
            print -r -- 'fn64-recomp-rs feature "dynamic-mapped-runtime"'
        elif [[ "${FN64_PAIR_TEST_AOT_GRAPH_FAIL:-}" == dev ]]; then
            print -r -- 'fn64-recomp-rs feature "dev-interpreter"'
        fi
    fi
    exit 0
fi
[[ "${CARGO_BUILD_JOBS:-}" == 1 ]]
[[ -n "${CARGO_TARGET_DIR:-}" ]]
print -r -- "cargo $* ROM=$ROM BOOT=$FN64_BOOT_CONTEXT GROUP=$FN64_EXECUTABLE_IMAGE_FIXTURE" >> "$FN64_PAIR_TEST_ORDER"
print -r -- "building ROM=$ROM BOOT=$FN64_BOOT_CONTEXT GROUP=$FN64_EXECUTABLE_IMAGE_FIXTURE"
if [[ " $* " == *" --features dynamic-withheld "* \
    && "${FN64_PAIR_TEST_DYNAMIC_BUILD_FAIL:-}" == 1 ]]; then
    exit 96
fi
mkdir -p "$CARGO_TARGET_DIR/debug"
if [[ " $* " == *" --features dynamic-withheld "* ]]; then
    print -r -- '#!/bin/zsh\nexit 0\ndynamic' > "$CARGO_TARGET_DIR/debug/wm2000-block-boot"
    if [[ -n "${FN64_PAIR_TEST_DRIFT_CAPTURE:-}" ]]; then
        print -n -- drift >> "$FN64_PAIR_TEST_DRIFT_CAPTURE"
    fi
else
    print -r -- '#!/bin/zsh\nexit 0\naot' > "$CARGO_TARGET_DIR/debug/wm2000-block-boot"
fi
chmod +x "$CARGO_TARGET_DIR/debug/wm2000-block-boot"
EOF
mv "$fake_bin/cargo" "$fake_bin/rustup"
ln -s rustup "$fake_bin/cargo"
cat > "$test_dir/fake-guard" <<'EOF'
#!/bin/zsh
set -eu
if [[ "${1:-}" == /bin/zsh && "${2:-}" == -c ]]; then
    print -r -- "guard aot-feature-gate" >> "$FN64_PAIR_TEST_ORDER"
else
    print -r -- "guard $*" >> "$FN64_PAIR_TEST_ORDER"
fi
guard_reason=complete
[[ "${FN64_PAIR_TEST_THRESHOLD:-}" == 1 ]] && guard_reason=tree_rss
guard_tree_rss=1
[[ "${FN64_PAIR_TEST_GUARD_BOOL:-}" == 1 ]] && guard_tree_rss=true
print -r -- "{\"schema\":\"fn64.memory-guard.sample.v1\",\"elapsed_seconds\":0,\"tree_rss_mib\":$guard_tree_rss,\"peak_tree_rss_mib\":1,\"reason\":\"$guard_reason\"}" > "$FN64_GUARD_JSONL"
exec "$@"
EOF
cat > "$test_dir/fake-checker" <<'EOF'
#!/bin/zsh
set -eu
print -r -- checker >> "$FN64_PAIR_TEST_ORDER"
if [[ "${FN64_PAIR_TEST_CHECKER_SPOOFS_REQUIRED:-}" == 1 ]]; then
    print -r -- 'fn64-recomp-rs feature "aot-runtime"'
    print -r -- 'fn64-recomp-rs feature "production-aot"'
fi
EOF
chmod +x "$fake_bin/rustup" "$test_dir/fake-guard" "$test_dir/fake-checker"

typeset -r output=$test_dir/output
typeset -r order=$test_dir/order.log
if ! PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$order \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$output" >"$test_dir/positive.log" 2>&1
then
    command cat -- "$test_dir/positive.log" >&2
    exit 1
fi
[[ -x "$output/wm2000-block-boot.aot" && -x "$output/wm2000-block-boot.dynamic-withheld" ]]
[[ -s "$output/aot-feature-check-memory-guard.jsonl" && -s "$output/dynamic-feature-check-memory-guard.jsonl" ]]
[[ -s "$output/aot-memory-guard.jsonl" && -s "$output/dynamic-withheld-memory-guard.jsonl" ]]
[[ "$(shasum -a 256 "$output/wm2000-block-boot.aot" | awk '{print $1}')" != "$(shasum -a 256 "$output/wm2000-block-boot.dynamic-withheld" | awk '{print $1}')" ]]
[[ "$(sed -n '1p' "$order")" == 'guard aot-feature-gate' ]]
[[ "$(sed -n '2p' "$order")" == cargo-tree* ]]
[[ "$(sed -n '2p' "$order")" != *dynamic-withheld* ]]
[[ "$(sed -n '2p' "$order")" == *' -e features '* ]]
[[ "$(sed -n '2p' "$order")" == *' -i fn64-recomp-rs'* ]]
[[ "$(sed -n '3p' "$order")" == checker ]]
[[ "$(sed -n '4p' "$order")" == *'guard '*cargo*' tree '*dynamic-withheld* ]]
[[ "$(sed -n '5p' "$order")" == cargo-tree*dynamic-withheld* ]]
[[ "$(sed -n '5p' "$order")" == *' -e features '* ]]
[[ "$(sed -n '5p' "$order")" == *' -i fn64-recomp-rs'* ]]
[[ "$(sed -n '6p' "$order")" == *'guard /usr/bin/env '* ]]
[[ "$(sed -n '6p' "$order")" != *dynamic-withheld* ]]
[[ "$(sed -n '7p' "$order")" == cargo\ build* ]]
[[ "$(sed -n '7p' "$order")" != *dynamic-withheld* ]]
[[ "$(sed -n '8p' "$order")" == *'guard /usr/bin/env '*dynamic-withheld* ]]
[[ "$(sed -n '9p' "$order")" == cargo\ build*dynamic-withheld* ]]
! rg -q 'cargo check' "$order"
typeset -r aot_target=$(sed -n '6p' "$order" | sed -n 's/.*CARGO_TARGET_DIR=\([^ ]*\).*/\1/p')
typeset -r dynamic_target=$(sed -n '8p' "$order" | sed -n 's/.*CARGO_TARGET_DIR=\([^ ]*\).*/\1/p')
[[ "$aot_target" == "$output/cargo-target/aot" ]]
[[ "$dynamic_target" == "$output/cargo-target/dynamic-withheld" ]]
rg -q '<PRIVATE_INPUT>' "$output/aot-build.log"
rg -q '<PRIVATE_INPUT>' "$output/dynamic-withheld-build.log"
for retained in "$output/aot-build.log" "$output/dynamic-withheld-build.log" "$output/receipt.json"; do
    if rg -Fq "$private_inputs" "$retained"; then
        print -u2 -- "test-build-wm2000-withheld-pair: private input path leaked into ${retained:t}"
        exit 1
    fi
done
python3 - "$output/receipt.json" <<'PY'
import hashlib
import json
import pathlib
import sys
receipt = json.load(open(sys.argv[1], encoding="utf-8"))
output = pathlib.Path(sys.argv[1]).parent
assert receipt["schema"] == "fn64.wm2000.withheld-pair-build-receipt.v4"
assert receipt["authority"] == "build_local_non_authoritative"
assert receipt["identity_relation"] == "pre_use_hashes_verified_unchanged_after_both_builds"
assert receipt["guard"]["cargo_build_jobs"] == 1
assert receipt["guard"]["max_seconds"] == 3600
assert receipt["guard"]["max_rss_mib"] == 4096
assert receipt["guard"]["aot_measurement"]["peak_tree_rss_mib"] == 1
assert receipt["guard"]["dynamic_withheld_measurement"]["last_reason"] == "complete"
assert receipt["guard"]["aot_feature_check_measurement"]["last_reason"] == "complete"
assert receipt["guard"]["dynamic_feature_check_measurement"]["last_reason"] == "complete"
assert receipt["artifacts"]["aot_feature_tree_log_sha256"] == hashlib.sha256(
    (output / "aot-feature-check.log").read_bytes()
).hexdigest()
assert receipt["artifacts"]["pure_aot_checker_log_sha256"] == hashlib.sha256(
    (output / "pure-aot-checker.log").read_bytes()
).hexdigest()
assert receipt["artifacts"]["dynamic_feature_tree_log_sha256"] == hashlib.sha256(
    (output / "dynamic-feature-check.log").read_bytes()
).hexdigest()
assert receipt["artifacts"]["cargo_cache_seed"] == "none"
groups = receipt["inputs"]["executable_image_capture_groups"]
assert [group["name"] for group in groups] == ["FN64_EXECUTABLE_IMAGE_FIXTURE"]
assert len(groups[0]["ordered_capture_sha256"]) == 3
assert all(len(digest) == 64 for digest in groups[0]["ordered_capture_sha256"])
assert receipt["commands"]["aot"][-1] == "wm2000-block-boot"
assert receipt["commands"]["dynamic_withheld"][-2:] == ["--features", "dynamic-withheld"]
assert receipt["commands"]["pure_aot_feature_gate"][-3:] == [
    "wm2000-block-boot", "-i", "fn64-recomp-rs"
]
assert receipt["commands"]["dynamic_feature_gate"][-2:] == ["--features", "dynamic-withheld"]
assert "normal,features" not in receipt["commands"]["pure_aot_feature_gate"]
assert "normal,features" not in receipt["commands"]["dynamic_feature_gate"]
assert "dynamic_source_check" not in receipt["commands"]
assert "dynamic_source_check_log_sha256" not in receipt["artifacts"]
assert "dynamic_source_check_jsonl" not in receipt["guard"]
assert "dynamic_source_check_measurement" not in receipt["guard"]
assert receipt["commands"]["pure_aot_feature_gate"][0] == "<MEMORY_GUARD_BY_RECORDED_SHA256>"
assert "<PURE_AOT_CHECKER_BY_RECORDED_SHA256>" in receipt["commands"]["pure_aot_checker_gate"]
assert "<CARGO_BY_RECORDED_SHA256>" in receipt["commands"]["aot"]
assert all("<PRIVATE" in item or not item.startswith("/") for item in receipt["commands"]["aot"])
PY

typeset -r dynamic_build_output=$test_dir/dynamic-build-failure
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/dynamic-build-order.log FN64_PAIR_TEST_DYNAMIC_BUILD_FAIL=1 \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$dynamic_build_output" >"$test_dir/dynamic-build.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: failing dynamic build was accepted"
    exit 1
fi
rg -q 'dynamic-withheld build failed' "$test_dir/dynamic-build.log"
[[ -x "$dynamic_build_output/wm2000-block-boot.aot" ]]
[[ ! -e "$dynamic_build_output/wm2000-block-boot.dynamic-withheld" ]]
[[ ! -e "$dynamic_build_output/receipt.json" ]]

typeset -r seed=$test_dir/cache-seed
typeset -r seeded_output=$test_dir/seeded-output
mkdir "$seed"
print -n -- retained-cache > "$seed/seed-marker"
if ! PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/seed-order.log \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_SEED=$seed \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$seeded_output" >"$test_dir/seeded.log" 2>&1
then
    command cat -- "$test_dir/seeded.log" >&2
    exit 1
fi
[[ -f "$seeded_output/cargo-target/aot/seed-marker" ]]
[[ -f "$seeded_output/cargo-target/dynamic-withheld/seed-marker" ]]
python3 - "$seeded_output/receipt.json" <<'PY'
import json
import sys
receipt = json.load(open(sys.argv[1], encoding="utf-8"))
assert receipt["artifacts"]["cargo_cache_seed"] == "caller_provided_untrusted_acceleration"
PY
if rg -Fq "$seed" "$seeded_output/receipt.json" "$seeded_output/aot-build.log" "$seeded_output/dynamic-withheld-build.log"; then
    print -u2 -- "test-build-wm2000-withheld-pair: Cargo cache seed path leaked"
    exit 1
fi

typeset -r lane_seed=$test_dir/lane-cache-seed
typeset -r lane_seeded_output=$test_dir/lane-seeded-output
mkdir -p "$lane_seed/aot" "$lane_seed/dynamic-withheld"
print -n -- aot-cache > "$lane_seed/aot/aot-marker"
print -n -- dynamic-cache > "$lane_seed/dynamic-withheld/dynamic-marker"
if ! PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/lane-seed-order.log \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_SEED=$lane_seed \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$lane_seeded_output" \
        >"$test_dir/lane-seeded.log" 2>&1
then
    command cat -- "$test_dir/lane-seeded.log" >&2
    exit 1
fi
[[ -f "$lane_seeded_output/cargo-target/aot/aot-marker" ]]
[[ ! -e "$lane_seeded_output/cargo-target/aot/dynamic-marker" ]]
[[ -f "$lane_seeded_output/cargo-target/dynamic-withheld/dynamic-marker" ]]
[[ ! -e "$lane_seeded_output/cargo-target/dynamic-withheld/aot-marker" ]]

typeset -r partial_lane_seed=$test_dir/partial-lane-cache-seed
mkdir -p "$partial_lane_seed/aot"
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/partial-lane-seed-order.log \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_SEED=$partial_lane_seed \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" \
        "$test_dir/partial-lane-seeded-output" >"$test_dir/partial-lane-seeded.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: partial lane cache seed was accepted"
    exit 1
fi
rg -q 'must contain aot and dynamic-withheld directories' "$test_dir/partial-lane-seeded.log"

typeset -r persistent_cache=$test_dir/persistent-cache
typeset -r persistent_output=$test_dir/persistent-output
mkdir "$persistent_cache"
if ! PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/persistent-order.log \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_ROOT=$persistent_cache \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$persistent_output" \
        >"$test_dir/persistent.log" 2>&1
then
    command cat -- "$test_dir/persistent.log" >&2
    exit 1
fi
[[ -x "$persistent_cache/aot/debug/wm2000-block-boot" ]]
[[ -x "$persistent_cache/dynamic-withheld/debug/wm2000-block-boot" ]]
[[ -x "$persistent_output/cargo-target/aot/debug/wm2000-block-boot" ]]
[[ -x "$persistent_output/cargo-target/dynamic-withheld/debug/wm2000-block-boot" ]]
[[ ! -e "$persistent_cache/.fn64-wm-pair.lock" ]]
python3 - "$persistent_output/receipt.json" <<'PY'
import json
import sys

receipt = json.load(open(sys.argv[1], encoding="utf-8"))
assert receipt["artifacts"]["cargo_cache_seed"] == "caller_provided_untrusted_acceleration"
assert "CARGO_TARGET_DIR=<PRIVATE_PERSISTENT_CACHE>/aot" in receipt["commands"]["aot"]
assert "CARGO_TARGET_DIR=<PRIVATE_PERSISTENT_CACHE>/dynamic-withheld" in receipt["commands"]["dynamic_withheld"]
PY

typeset -r persistent_failure_output=$test_dir/persistent-failure-output
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/persistent-failure-order.log \
    FN64_PAIR_TEST_GUARD_BOOL=1 \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_ROOT=$persistent_cache \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$persistent_failure_output" \
        >"$test_dir/persistent-failure.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: persistent early-failure fixture was accepted"
    exit 1
fi
rg -q 'memory guard JSONL tree_rss_mib is invalid' "$test_dir/persistent-failure.log"
[[ ! -e "$persistent_cache/.fn64-wm-pair.lock" ]]

if rg -Fq "$persistent_cache" "$persistent_output/receipt.json" \
    "$persistent_output/aot-build.log" "$persistent_output/dynamic-withheld-build.log"; then
    print -u2 -- "test-build-wm2000-withheld-pair: persistent Cargo cache path leaked"
    exit 1
fi

typeset -r locked_cache=$test_dir/locked-persistent-cache
mkdir -p "$locked_cache/aot" "$locked_cache/dynamic-withheld" \
    "$locked_cache/.fn64-wm-pair.lock"
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/locked-persistent-order.log \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_ROOT=$locked_cache \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" \
        "$test_dir/locked-persistent-output" >"$test_dir/locked-persistent.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: locked persistent cache was accepted"
    exit 1
fi
rg -q 'already locked by another pair build' "$test_dir/locked-persistent.log"
[[ -d "$locked_cache/.fn64-wm-pair.lock" ]]

if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/mutual-cache-order.log \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_SEED=$seed \
    FN64_WM_PAIR_CARGO_CACHE_ROOT=$persistent_cache \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" \
        "$test_dir/mutual-cache-output" >"$test_dir/mutual-cache.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: simultaneous seed and persistent cache were accepted"
    exit 1
fi
rg -q 'are mutually exclusive' "$test_dir/mutual-cache.log"

if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/repo-seed-order.log \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    FN64_WM_PAIR_CARGO_CACHE_SEED=$test_root \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$test_dir/repo-seed-output" >"$test_dir/repo-seed.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: repository Cargo cache seed was accepted"
    exit 1
fi
rg -q 'CARGO_CACHE_SEED must remain outside the repository' "$test_dir/repo-seed.log"

if ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$output" >"$test_dir/existing.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: existing output was accepted"
    exit 1
fi
rg -q 'must not already exist' "$test_dir/existing.log"
if ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$test_dir/too-few" >"$test_dir/captures.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: incomplete capture group was accepted"
    exit 1
fi
rg -q 'at least three' "$test_dir/captures.log"
if ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$test_root/repository-output" >"$test_dir/repo.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: repository output was accepted"
    exit 1
fi
rg -q 'outside the repository' "$test_dir/repo.log"
mkdir "$test_dir/symlink-target"
ln -s "$test_dir/symlink-target" "$test_dir/symlink-parent"
if ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$test_dir/symlink-parent/output" >"$test_dir/symlink.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: symlink parent was accepted"
    exit 1
fi
rg -q 'non-symlink directory' "$test_dir/symlink.log"

for invalid_spec in rss:0 free:101 seconds:0; do
    invalid_name=${invalid_spec%%:*}
    invalid_value=${invalid_spec#*:}
    invalid_output=$test_dir/invalid-$invalid_name
    typeset -a invalid_env
    invalid_env=()
    case $invalid_name in
        rss) invalid_env+=(FN64_GUARD_MAX_RSS_MIB=$invalid_value) ;;
        free) invalid_env+=(FN64_GUARD_MIN_FREE_PERCENT=$invalid_value) ;;
        seconds) invalid_env+=(FN64_GUARD_MAX_SECONDS=$invalid_value) ;;
    esac
    if /usr/bin/env "${invalid_env[@]}" PATH=$fake_bin:$PATH \
        ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
        FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
        FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
        FN64_PAIR_TEST_ORDER=$order \
        FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
        FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
        "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$invalid_output" \
        >"$test_dir/invalid-$invalid_name.log" 2>&1
    then
        print -u2 -- "test-build-wm2000-withheld-pair: invalid $invalid_name limit was accepted"
        exit 1
    fi
    [[ ! -e "$invalid_output" ]]
done
rg -q 'MAX_RSS_MIB must be positive' "$test_dir/invalid-rss.log"
rg -q 'MIN_FREE_PERCENT must be 0..100' "$test_dir/invalid-free.log"
rg -q 'MAX_SECONDS must be positive' "$test_dir/invalid-seconds.log"

typeset -r graph_output=$test_dir/graph-failure
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/graph-order.log FN64_PAIR_TEST_DYNAMIC_GRAPH_FAIL=dev \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$graph_output" >"$test_dir/graph.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: invalid dynamic feature graph was accepted"
    exit 1
fi
rg -q 'unexpectedly enables dev-interpreter' "$test_dir/graph.log"
[[ ! -e "$graph_output/wm2000-block-boot.aot" ]]

typeset missing_mode
for missing_feature in production-aot aot-runtime dynamic-mapped-runtime; do
    case $missing_feature in
        production-aot) missing_mode=missing-production ;;
        aot-runtime) missing_mode=missing-aot-runtime ;;
        dynamic-mapped-runtime) missing_mode=missing-runtime ;;
    esac
    typeset missing_output=$test_dir/dynamic-missing-$missing_feature
    if PATH=$fake_bin:$PATH \
        ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
        FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
        FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
        FN64_PAIR_TEST_ORDER=$test_dir/dynamic-missing-$missing_feature-order.log \
        FN64_PAIR_TEST_DYNAMIC_GRAPH_FAIL=$missing_mode \
        FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
        FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
        "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$missing_output" \
        >"$test_dir/dynamic-missing-$missing_feature.log" 2>&1
    then
        print -u2 -- "test-build-wm2000-withheld-pair: dynamic graph lacking $missing_feature was accepted"
        exit 1
    fi
    rg -q "dynamic-withheld feature graph lacks $missing_feature" \
        "$test_dir/dynamic-missing-$missing_feature.log"
    [[ ! -e "$missing_output/wm2000-block-boot.aot" ]]
done

typeset aot_missing_mode
for missing_feature in production-aot aot-runtime; do
    case $missing_feature in
        production-aot) aot_missing_mode=missing-production ;;
        aot-runtime) aot_missing_mode=missing-aot-runtime ;;
    esac
    typeset aot_missing_output=$test_dir/aot-missing-$missing_feature
    if PATH=$fake_bin:$PATH \
        ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
        FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
        FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
        FN64_PAIR_TEST_ORDER=$test_dir/aot-missing-$missing_feature-order.log \
        FN64_PAIR_TEST_AOT_GRAPH_FAIL=$aot_missing_mode \
        FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
        FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
        "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$aot_missing_output" \
        >"$test_dir/aot-missing-$missing_feature.log" 2>&1
    then
        print -u2 -- "test-build-wm2000-withheld-pair: AOT graph lacking $missing_feature was accepted"
        exit 1
    fi
    rg -q "pure-AOT feature graph lacks $missing_feature" \
        "$test_dir/aot-missing-$missing_feature.log"
    [[ ! -e "$aot_missing_output/wm2000-block-boot.aot" ]]
done

for forbidden_feature in dynamic dev; do
    typeset aot_graph_output=$test_dir/aot-$forbidden_feature-graph-failure
    if PATH=$fake_bin:$PATH \
        ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
        FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
        FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
        FN64_PAIR_TEST_ORDER=$test_dir/aot-$forbidden_feature-graph-order.log \
        FN64_PAIR_TEST_AOT_GRAPH_FAIL=$forbidden_feature \
        FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
        FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
        "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$aot_graph_output" \
        >"$test_dir/aot-$forbidden_feature-graph.log" 2>&1
    then
        print -u2 -- "test-build-wm2000-withheld-pair: forbidden $forbidden_feature AOT graph was accepted"
        exit 1
    fi
    rg -q 'pure-AOT feature graph unexpectedly enables' \
        "$test_dir/aot-$forbidden_feature-graph.log"
    [[ ! -e "$aot_graph_output/wm2000-block-boot.aot" ]]
done

typeset -r checker_spoof_output=$test_dir/checker-spoof-output
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/checker-spoof-order.log \
    FN64_PAIR_TEST_AOT_GRAPH_FAIL=missing-production \
    FN64_PAIR_TEST_CHECKER_SPOOFS_REQUIRED=1 \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$checker_spoof_output" \
    >"$test_dir/checker-spoof.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: checker output spoofed the outer feature graph"
    exit 1
fi
rg -q 'pure-AOT feature graph lacks production-aot' "$test_dir/checker-spoof.log"
rg -q 'fn64-recomp-rs feature "production-aot"' "$checker_spoof_output/pure-aot-checker.log"
if rg -q 'fn64-recomp-rs feature "production-aot"' "$checker_spoof_output/aot-feature-check.log"; then
    print -u2 -- "test-build-wm2000-withheld-pair: checker evidence contaminated the outer graph log"
    exit 1
fi
[[ ! -e "$checker_spoof_output/receipt.json" ]]

typeset -r bool_guard_output=$test_dir/bool-guard-output
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/bool-guard-order.log FN64_PAIR_TEST_GUARD_BOOL=1 \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$bool_guard_output" >"$test_dir/bool-guard.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: boolean guard numeric evidence was accepted"
    exit 1
fi
rg -q 'memory guard JSONL tree_rss_mib is invalid' "$test_dir/bool-guard.log"
[[ ! -e "$bool_guard_output/receipt.json" ]]

typeset -r threshold_output=$test_dir/threshold-output
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$test_capture_c \
    FN64_PAIR_TEST_ORDER=$test_dir/threshold-order.log FN64_PAIR_TEST_THRESHOLD=1 \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$threshold_output" >"$test_dir/threshold.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: threshold guard evidence was accepted"
    exit 1
fi
rg -q 'threshold or unknown reason' "$test_dir/threshold.log"
[[ ! -e "$threshold_output/receipt.json" ]]

typeset -r drift_capture=$private_inputs/private-drift-capture.json
print -n -- stable > "$drift_capture"
typeset -r drift_output=$test_dir/drift-output
if PATH=$fake_bin:$PATH \
    ROM=$test_rom FN64_BOOT_CONTEXT=$test_boot \
    FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_FIXTURE \
    FN64_EXECUTABLE_IMAGE_FIXTURE=$test_capture_a:$test_capture_b:$drift_capture \
    FN64_PAIR_TEST_ORDER=$test_dir/drift-order.log FN64_PAIR_TEST_DRIFT_CAPTURE=$drift_capture \
    FN64_WM_PAIR_MEMORY_GUARD=$test_dir/fake-guard \
    FN64_WM_PAIR_AOT_CHECKER=$test_dir/fake-checker \
    "$test_root/scripts/build-wm2000-withheld-pair.zsh" "$drift_output" >"$test_dir/drift.log" 2>&1
then
    print -u2 -- "test-build-wm2000-withheld-pair: drifting capture was accepted"
    exit 1
fi
rg -q 'executable-image capture 3 changed while building' "$test_dir/drift.log"
[[ ! -e "$drift_output/receipt.json" ]]
print -- "test-build-wm2000-withheld-pair: pass"
