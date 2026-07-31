#!/bin/zsh

# Build the two independently-featured WM2000 artifacts used by the withheld
# dynamic A/B. Generated code and user-supplied captures remain in one private,
# caller-created directory; the receipt deliberately contains identities only.

set -eu
setopt NOCLOBBER

usage() {
    print -u2 -- "usage: ROM=/absolute/rom.z64 FN64_BOOT_CONTEXT=/absolute/boot.json FN64_EXECUTABLE_IMAGE_GROUPS=GROUP[,GROUP...] GROUP=/absolute/capture1:/absolute/capture2:/absolute/capture3 [FN64_WM_PAIR_CARGO_CACHE_SEED=/absolute/prior-cargo-target] $0 NEW_ABSOLUTE_OUTPUT_DIR"
}

if (( $# != 1 )); then
    usage
    exit 2
fi

typeset -r pair_root=${0:A:h:h}
typeset -r pair_manifest=$pair_root/examples/wm2000-block-boot/Cargo.toml
typeset -r pair_lock=$pair_root/examples/wm2000-block-boot/Cargo.lock
typeset -r pair_package=wm2000-block-boot
typeset -r pair_output_requested=$1
typeset -r pair_guard=${FN64_WM_PAIR_MEMORY_GUARD:-$pair_root/scripts/memory-guard.zsh}
typeset -r pair_checker=${FN64_WM_PAIR_AOT_CHECKER:-$pair_root/scripts/check-wm2000-pure-aot.zsh}
typeset -r pair_max_rss_mib=${FN64_GUARD_MAX_RSS_MIB:-4096}
typeset -r pair_min_free_percent=${FN64_GUARD_MIN_FREE_PERCENT:-40}
typeset -r pair_max_seconds=${FN64_GUARD_MAX_SECONDS:-3600}
typeset -r pair_poll_seconds=${FN64_GUARD_POLL_SECONDS:-1}
typeset -r pair_cache_seed_requested=${FN64_WM_PAIR_CARGO_CACHE_SEED:-}
typeset -r pair_target_subdir=cargo-target
typeset -r pair_binary_relative=debug/$pair_package
typeset -a pair_redactions pair_group_names pair_capture_paths pair_capture_digests pair_group_capture_counts
typeset pair_redaction_file=
typeset -i pair_finished=0

fail() {
    print -u2 -- "wm2000 withheld pair build: $*"
    exit 1
}

require_absolute_readable_file() {
    local label=$1
    local value=$2
    [[ "$value" == /* && -f "$value" && -r "$value" && ! -L "$value" ]] || \
        fail "$label must name an absolute readable regular non-symlink file"
    [[ "$value" != *$'\n'* && "$value" != *$'\r'* ]] || fail "$label path contains a line break"
}

hash_file() {
    local digest_line
    digest_line=$(shasum -a 256 -- "$1") || fail "cannot hash required input"
    [[ "$digest_line" =~ '^[0-9a-f]{64} ' ]] || fail "hash tool returned a malformed SHA-256"
    print -r -- "${digest_line%% *}"
}

sanitize_log() {
    local log_path=$1
    [[ -e "$log_path" ]] || return
    python3 - "$log_path" "$pair_redaction_file" <<'PY'
import pathlib
import sys

log = pathlib.Path(sys.argv[1])
secrets = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8").splitlines()
text = log.read_text(encoding="utf-8", errors="replace")
for secret in sorted(set(filter(None, secrets)), key=len, reverse=True):
    text = text.replace(secret, "<PRIVATE_INPUT>")
log.write_text(text, encoding="utf-8")
PY
}

cleanup() {
    local result=$?
    trap - EXIT HUP INT TERM
    if (( ! pair_finished )) && [[ -n "$pair_redaction_file" && -d ${pair_output:-} ]]; then
        sanitize_log "$pair_output/aot-feature-check.log"
        sanitize_log "$pair_output/pure-aot-checker.log"
        sanitize_log "$pair_output/dynamic-feature-check.log"
        sanitize_log "$pair_output/dynamic-source-check.log"
        sanitize_log "$pair_output/aot-build.log"
        sanitize_log "$pair_output/dynamic-withheld-build.log"
    fi
    if [[ -n "$pair_redaction_file" ]]; then
        rm -f -- "$pair_redaction_file"
    fi
    return $result
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

[[ "$pair_output_requested" == /* ]] || fail "output directory must be a new absolute path"
[[ ! -e "$pair_output_requested" && ! -L "$pair_output_requested" ]] || fail "output directory must not already exist"
typeset -r pair_parent=${pair_output_requested:h}
[[ -d "$pair_parent" && -w "$pair_parent" && -r "$pair_parent" && ! -L "$pair_parent" ]] || \
    fail "output parent must be an existing writable non-symlink directory"
typeset -r pair_parent_canonical=${pair_parent:A}
typeset -r pair_root_canonical=${pair_root:A}
[[ "$pair_parent_canonical" != "$pair_root_canonical" && "$pair_parent_canonical" != "$pair_root_canonical"/* ]] || \
    fail "output directory must remain outside the repository"
typeset -r pair_output=$pair_parent_canonical/${pair_output_requested:t}

require_absolute_readable_file ROM "${ROM:-}"
require_absolute_readable_file FN64_BOOT_CONTEXT "${FN64_BOOT_CONTEXT:-}"
typeset -r pair_rom=${ROM:A}
typeset -r pair_boot_context=${FN64_BOOT_CONTEXT:A}
pair_redactions=("$ROM" "$pair_rom" "$FN64_BOOT_CONTEXT" "$pair_boot_context")

typeset pair_cache_seed=
typeset pair_cache_seed_mode=none
if [[ -n "$pair_cache_seed_requested" ]]; then
    [[ "$pair_cache_seed_requested" == /* && -d "$pair_cache_seed_requested" \
        && -r "$pair_cache_seed_requested" && ! -L "$pair_cache_seed_requested" ]] || \
        fail "FN64_WM_PAIR_CARGO_CACHE_SEED must name an absolute readable non-symlink directory"
    pair_cache_seed=${pair_cache_seed_requested:A}
    [[ "$pair_cache_seed" != "$pair_root_canonical" \
        && "$pair_cache_seed" != "$pair_root_canonical"/* ]] || \
        fail "FN64_WM_PAIR_CARGO_CACHE_SEED must remain outside the repository"
    [[ -z "$(find "$pair_cache_seed" -type l -print -quit)" ]] || \
        fail "FN64_WM_PAIR_CARGO_CACHE_SEED must not contain symlinks"
    pair_redactions+=("$pair_cache_seed_requested" "$pair_cache_seed")
    pair_cache_seed_mode=caller_provided_untrusted_acceleration
fi

[[ -n "${FN64_EXECUTABLE_IMAGE_GROUPS:-}" ]] || fail "FN64_EXECUTABLE_IMAGE_GROUPS must name current executable-image capture groups"
pair_group_names=("${(@s:,:)FN64_EXECUTABLE_IMAGE_GROUPS}")
(( ${#pair_group_names} > 0 )) || fail "FN64_EXECUTABLE_IMAGE_GROUPS names no capture groups"
typeset group_name capture_value capture_path
for group_name in "${pair_group_names[@]}"; do
    [[ "$group_name" == FN64_EXECUTABLE_IMAGES \
        || "$group_name" =~ '^FN64_EXECUTABLE_IMAGE_[A-Z0-9_]+$' ]] || \
        fail "executable-image group names must be FN64_EXECUTABLE_IMAGES or FN64_EXECUTABLE_IMAGE_* tokens"
    capture_value=${(P)group_name:-}
    [[ -n "$capture_value" ]] || fail "$group_name is not set"
    typeset -a group_paths
    group_paths=("${(@s/:/)capture_value}")
    (( ${#group_paths} >= 3 )) || fail "$group_name must contain at least three colon-separated captures"
    for capture_path in "${group_paths[@]}"; do
        require_absolute_readable_file "$group_name capture" "$capture_path"
        pair_redactions+=("$capture_path" "${capture_path:A}")
        pair_capture_paths+=("${capture_path:A}")
        pair_capture_digests+=("$(hash_file "${capture_path:A}")")
    done
    pair_group_capture_counts+=(${#group_paths})
done

[[ "$pair_guard" == /* && -x "$pair_guard" && -f "$pair_guard" && ! -L "$pair_guard" ]] || fail "memory guard must name an absolute executable non-symlink file"
[[ "$pair_checker" == /* && -x "$pair_checker" && -f "$pair_checker" && ! -L "$pair_checker" ]] || fail "pure-AOT checker must name an absolute executable non-symlink file"
typeset pair_cargo_found
pair_cargo_found=$(whence -p cargo) || fail "cargo is unavailable"
# Preserve the executable name used for rustup proxy dispatch. `:A` resolves
# `$HOME/.cargo/bin/cargo` to the `rustup` binary, which then interprets
# Cargo's `--locked` as a rustup argument instead of selecting the cargo proxy.
# `:a` makes the path absolute without dereferencing that load-bearing symlink.
typeset -r pair_cargo=${pair_cargo_found:a}
[[ -x "$pair_cargo" && -f "$pair_cargo" ]] || fail "resolved cargo tool is not an executable regular file"
[[ "$pair_max_rss_mib" =~ '^[1-9][0-9]*$' ]] || fail "FN64_GUARD_MAX_RSS_MIB must be positive"
[[ "$pair_min_free_percent" =~ '^[0-9]+$' ]] \
    && (( pair_min_free_percent >= 0 && pair_min_free_percent <= 100 )) \
    || fail "FN64_GUARD_MIN_FREE_PERCENT must be 0..100"
[[ "$pair_max_seconds" =~ '^[1-9][0-9]*$' ]] || fail "FN64_GUARD_MAX_SECONDS must be positive"
[[ "$pair_poll_seconds" == 0.05 || "$pair_poll_seconds" == 0.1 || "$pair_poll_seconds" == 0.25 || "$pair_poll_seconds" == 0.5 || "$pair_poll_seconds" == 1 || "$pair_poll_seconds" == 2 ]] || \
    fail "FN64_GUARD_POLL_SECONDS has an unsupported value"

# These are pre-use identities, not a filesystem snapshot. Each path is read
# again after both builds and any drift prevents publication of a receipt.
typeset -r pair_rom_pre_sha=$(hash_file "$pair_rom")
typeset -r pair_boot_context_pre_sha=$(hash_file "$pair_boot_context")
typeset -r pair_manifest_pre_sha=$(hash_file "$pair_manifest")
typeset -r pair_lock_pre_sha=$(hash_file "$pair_lock")
typeset -r pair_guard_pre_sha=$(hash_file "$pair_guard")
typeset -r pair_checker_pre_sha=$(hash_file "$pair_checker")
typeset -r pair_cargo_pre_sha=$(hash_file "$pair_cargo")

mkdir -m 700 -- "$pair_output" || fail "cannot create private output directory"
typeset -r pair_target=$pair_output/$pair_target_subdir
typeset -r pair_aot_log=$pair_output/aot-build.log
typeset -r pair_dynamic_log=$pair_output/dynamic-withheld-build.log
typeset -r pair_aot_check_log=$pair_output/aot-feature-check.log
typeset -r pair_checker_log=$pair_output/pure-aot-checker.log
typeset -r pair_dynamic_check_log=$pair_output/dynamic-feature-check.log
typeset -r pair_dynamic_source_check_log=$pair_output/dynamic-source-check.log
typeset -r pair_aot_check_guard_jsonl=$pair_output/aot-feature-check-memory-guard.jsonl
typeset -r pair_dynamic_check_guard_jsonl=$pair_output/dynamic-feature-check-memory-guard.jsonl
typeset -r pair_dynamic_source_check_guard_jsonl=$pair_output/dynamic-source-check-memory-guard.jsonl
typeset -r pair_aot_guard_jsonl=$pair_output/aot-memory-guard.jsonl
typeset -r pair_dynamic_guard_jsonl=$pair_output/dynamic-withheld-memory-guard.jsonl
typeset -r pair_aot_binary=$pair_output/wm2000-block-boot.aot
typeset -r pair_dynamic_binary=$pair_output/wm2000-block-boot.dynamic-withheld
typeset -r pair_receipt=$pair_output/receipt.json
pair_redaction_file=$(mktemp "${TMPDIR:-/private/tmp}/fn64-wm-pair-redactions.XXXXXX") || fail "cannot reserve private redaction list"
printf '%s\n' "${pair_redactions[@]}" >> "$pair_redaction_file"

if [[ -n "$pair_cache_seed" ]]; then
    mkdir -m 700 -- "$pair_target" || fail "cannot create private Cargo target"
    # The seed is only a local acceleration input. Both lanes still execute
    # their complete Cargo commands, and only their newly retained binaries
    # enter the build-local receipt.
    cp -cR -- "$pair_cache_seed/." "$pair_target" || \
        fail "cannot clone the caller-provided Cargo cache seed"
fi

copy_retained_binary() {
    local source=$1
    local destination=$2
    [[ -x "$source" && -f "$source" && ! -L "$source" ]] || fail "Cargo did not produce the expected executable"
    # Cargo may rewrite a target in place on a subsequent feature build; a
    # hard link would then mutate the supposedly retained AOT artifact too.
    cp -p -- "$source" "$destination" || fail "cannot retain built executable"
    [[ -x "$destination" && -f "$destination" && ! -L "$destination" ]] || fail "retained executable is invalid"
}

typeset -a pair_build_env
pair_build_env=(
    "ROM=$pair_rom"
    "FN64_BOOT_CONTEXT=$pair_boot_context"
    "FN64_EXECUTABLE_IMAGE_GROUPS=$FN64_EXECUTABLE_IMAGE_GROUPS"
    "CARGO_TARGET_DIR=$pair_target"
    CARGO_BUILD_JOBS=1
    "FN64_GUARD_MAX_RSS_MIB=$pair_max_rss_mib"
    "FN64_GUARD_MIN_FREE_PERCENT=$pair_min_free_percent"
    "FN64_GUARD_MAX_SECONDS=$pair_max_seconds"
    "FN64_GUARD_POLL_SECONDS=$pair_poll_seconds"
)
for group_name in "${pair_group_names[@]}"; do
    pair_build_env+=("$group_name=${(P)group_name}")
done

print -u2 -- "wm2000 withheld pair build: checking pure AOT feature graph"
if ! FN64_GUARD_MAX_RSS_MIB=$pair_max_rss_mib \
    FN64_GUARD_MIN_FREE_PERCENT=$pair_min_free_percent \
    FN64_GUARD_MAX_SECONDS=$pair_max_seconds \
    FN64_GUARD_POLL_SECONDS=$pair_poll_seconds \
    FN64_GUARD_JSONL=$pair_aot_check_guard_jsonl \
    CARGO_TARGET_DIR=$pair_target CARGO_BUILD_JOBS=1 \
    "$pair_guard" /bin/zsh -c '
        set -eu
        "$1" tree --locked --manifest-path "$3" -e features -p "$4" -i fn64-recomp-rs
        "$2" >"$5" 2>&1
    ' pair-aot-feature-gate "$pair_cargo" "$pair_checker" "$pair_manifest" "$pair_package" \
        "$pair_checker_log" \
        >"$pair_aot_check_log" 2>&1
then
    sanitize_log "$pair_aot_check_log"
    sanitize_log "$pair_checker_log"
    fail "pure-AOT feature gate failed; retained $pair_aot_check_log"
fi
if rg -Fq 'fn64-recomp-rs feature "dev-interpreter"' "$pair_aot_check_log"; then
    fail "pure-AOT feature graph unexpectedly enables dev-interpreter"
fi
if rg -Fq 'fn64-recomp-rs feature "dynamic-mapped-runtime"' "$pair_aot_check_log"; then
    fail "pure-AOT feature graph unexpectedly enables dynamic-mapped-runtime"
fi
if ! rg -Fq 'fn64-recomp-rs feature "aot-runtime"' "$pair_aot_check_log"; then
    fail "pure-AOT feature graph lacks aot-runtime"
fi
if ! rg -Fq 'fn64-recomp-rs feature "production-aot"' "$pair_aot_check_log"; then
    fail "pure-AOT feature graph lacks production-aot"
fi

print -u2 -- "wm2000 withheld pair build: checking dynamic-withheld feature graph"
if ! FN64_GUARD_MAX_RSS_MIB=$pair_max_rss_mib \
    FN64_GUARD_MIN_FREE_PERCENT=$pair_min_free_percent \
    FN64_GUARD_MAX_SECONDS=$pair_max_seconds \
    FN64_GUARD_POLL_SECONDS=$pair_poll_seconds \
    FN64_GUARD_JSONL=$pair_dynamic_check_guard_jsonl \
    CARGO_TARGET_DIR=$pair_target CARGO_BUILD_JOBS=1 \
    "$pair_guard" "$pair_cargo" tree --locked --manifest-path "$pair_manifest" \
        -e features -p "$pair_package" -i fn64-recomp-rs --features dynamic-withheld \
        >"$pair_dynamic_check_log" 2>&1
then
    fail "dynamic-withheld feature graph command failed; retained $pair_dynamic_check_log"
fi
if rg -Fq 'fn64-recomp-rs feature "dev-interpreter"' "$pair_dynamic_check_log"; then
    fail "dynamic-withheld feature graph unexpectedly enables dev-interpreter"
fi
if ! rg -Fq 'fn64-recomp-rs feature "aot-runtime"' "$pair_dynamic_check_log"; then
    fail "dynamic-withheld feature graph lacks aot-runtime"
fi
if ! rg -Fq 'fn64-recomp-rs feature "production-aot"' "$pair_dynamic_check_log"; then
    fail "dynamic-withheld feature graph lacks production-aot"
fi
if ! rg -Fq 'fn64-recomp-rs feature "dynamic-mapped-runtime"' "$pair_dynamic_check_log"; then
    fail "dynamic-withheld feature graph lacks dynamic-mapped-runtime"
fi

print -u2 -- "wm2000 withheld pair build: checking dynamic-withheld source"
if ! FN64_GUARD_MAX_RSS_MIB=$pair_max_rss_mib \
    FN64_GUARD_MIN_FREE_PERCENT=$pair_min_free_percent \
    FN64_GUARD_MAX_SECONDS=$pair_max_seconds \
    FN64_GUARD_POLL_SECONDS=$pair_poll_seconds \
    FN64_GUARD_JSONL=$pair_dynamic_source_check_guard_jsonl \
    "$pair_guard" /usr/bin/env "${pair_build_env[@]}" \
    "$pair_cargo" check -j1 --locked --manifest-path "$pair_manifest" -p "$pair_package" \
        --bin "$pair_package" --features dynamic-withheld >"$pair_dynamic_source_check_log" 2>&1
then
    sanitize_log "$pair_dynamic_source_check_log"
    fail "dynamic-withheld source check failed; retained $pair_dynamic_source_check_log"
fi

print -u2 -- "wm2000 withheld pair build: building AOT artifact"
if ! FN64_GUARD_MAX_RSS_MIB=$pair_max_rss_mib \
    FN64_GUARD_MIN_FREE_PERCENT=$pair_min_free_percent \
    FN64_GUARD_MAX_SECONDS=$pair_max_seconds \
    FN64_GUARD_POLL_SECONDS=$pair_poll_seconds \
    FN64_GUARD_JSONL=$pair_aot_guard_jsonl \
    "$pair_guard" /usr/bin/env "${pair_build_env[@]}" \
    "$pair_cargo" build -j1 --locked --manifest-path "$pair_manifest" -p "$pair_package" >"$pair_aot_log" 2>&1
then
    sanitize_log "$pair_aot_log"
    fail "AOT build failed; retained $pair_aot_log"
fi
copy_retained_binary "$pair_target/$pair_binary_relative" "$pair_aot_binary"

print -u2 -- "wm2000 withheld pair build: building dynamic-withheld artifact"
if ! FN64_GUARD_MAX_RSS_MIB=$pair_max_rss_mib \
    FN64_GUARD_MIN_FREE_PERCENT=$pair_min_free_percent \
    FN64_GUARD_MAX_SECONDS=$pair_max_seconds \
    FN64_GUARD_POLL_SECONDS=$pair_poll_seconds \
    FN64_GUARD_JSONL=$pair_dynamic_guard_jsonl \
    "$pair_guard" /usr/bin/env "${pair_build_env[@]}" \
    "$pair_cargo" build -j1 --locked --manifest-path "$pair_manifest" -p "$pair_package" --features dynamic-withheld >"$pair_dynamic_log" 2>&1
then
    sanitize_log "$pair_aot_log"
    sanitize_log "$pair_dynamic_log"
    fail "dynamic-withheld build failed; retained $pair_dynamic_log"
fi
copy_retained_binary "$pair_target/$pair_binary_relative" "$pair_dynamic_binary"
sanitize_log "$pair_aot_check_log"
sanitize_log "$pair_checker_log"
sanitize_log "$pair_dynamic_check_log"
sanitize_log "$pair_dynamic_source_check_log"
sanitize_log "$pair_aot_log"
sanitize_log "$pair_dynamic_log"

verify_unchanged() {
    local label=$1
    local input_path=$2
    local expected=$3
    [[ "$(hash_file "$input_path")" == "$expected" ]] || fail "$label changed while building; refusing receipt"
}
verify_unchanged ROM "$pair_rom" "$pair_rom_pre_sha"
verify_unchanged FN64_BOOT_CONTEXT "$pair_boot_context" "$pair_boot_context_pre_sha"
verify_unchanged manifest "$pair_manifest" "$pair_manifest_pre_sha"
verify_unchanged lockfile "$pair_lock" "$pair_lock_pre_sha"
verify_unchanged memory-guard "$pair_guard" "$pair_guard_pre_sha"
verify_unchanged pure-AOT-checker "$pair_checker" "$pair_checker_pre_sha"
typeset pair_cargo_after
pair_cargo_after=$(whence -p cargo) || fail "cargo disappeared while building; refusing receipt"
[[ "${pair_cargo_after:a}" == "$pair_cargo" ]] || fail "resolved cargo tool changed while building; refusing receipt"
verify_unchanged cargo "$pair_cargo" "$pair_cargo_pre_sha"
typeset -i pair_capture_index
for (( pair_capture_index = 1; pair_capture_index <= ${#pair_capture_paths}; pair_capture_index++ )); do
    verify_unchanged "executable-image capture $pair_capture_index" \
        "$pair_capture_paths[$pair_capture_index]" "$pair_capture_digests[$pair_capture_index]"
done

typeset -r pair_aot_sha=$(hash_file "$pair_aot_binary")
typeset -r pair_dynamic_sha=$(hash_file "$pair_dynamic_binary")
typeset -r pair_aot_feature_tree_sha=$(hash_file "$pair_aot_check_log")
typeset -r pair_checker_log_sha=$(hash_file "$pair_checker_log")
typeset -r pair_dynamic_feature_tree_sha=$(hash_file "$pair_dynamic_check_log")
typeset -r pair_dynamic_source_check_sha=$(hash_file "$pair_dynamic_source_check_log")
[[ "$pair_aot_sha" != "$pair_dynamic_sha" ]] || fail "feature-separated retained binaries unexpectedly have identical hashes"

python3 - "$pair_receipt" "$pair_rom_pre_sha" "$pair_boot_context_pre_sha" "$pair_aot_sha" "$pair_dynamic_sha" "$pair_manifest_pre_sha" "$pair_lock_pre_sha" "$pair_guard_pre_sha" "$pair_checker_pre_sha" "$pair_cargo_pre_sha" \
    "$pair_max_rss_mib" "$pair_min_free_percent" "$pair_max_seconds" "$pair_poll_seconds" \
    "$pair_target_subdir" "$pair_cache_seed_mode" "${(j:,:)pair_group_names}" "${(j:,:)pair_group_capture_counts}" "${(j:,:)pair_capture_digests}" \
    "$pair_aot_check_guard_jsonl" "$pair_dynamic_check_guard_jsonl" "$pair_dynamic_source_check_guard_jsonl" "$pair_aot_guard_jsonl" "$pair_dynamic_guard_jsonl" \
    "$pair_aot_feature_tree_sha" "$pair_checker_log_sha" "$pair_dynamic_feature_tree_sha" "$pair_dynamic_source_check_sha" <<'PY'
import json
import sys

(
    receipt_path, rom_sha, boot_context_sha, aot_sha, dynamic_sha, manifest_sha, lock_sha, guard_sha, checker_sha, cargo_sha,
    max_rss_mib, min_free_percent, max_seconds, poll_seconds, target_subdir, cache_seed_mode,
    group_names, group_capture_counts, capture_digests,
    aot_check_jsonl_path, dynamic_check_jsonl_path, dynamic_source_check_jsonl_path, aot_jsonl_path, dynamic_jsonl_path,
    aot_feature_tree_sha, checker_log_sha, dynamic_feature_tree_sha, dynamic_source_check_sha,
) = sys.argv[1:]
groups = [name for name in group_names.split(",") if name]
counts = [int(count) for count in group_capture_counts.split(",") if count]
captures = [digest for digest in capture_digests.split(",") if digest]
if len(groups) != len(counts) or sum(counts) != len(captures):
    raise SystemExit("internal executable-image receipt grouping failure")
offset = 0
capture_groups = []
for name, count in zip(groups, counts):
    capture_groups.append({"name": name, "ordered_capture_sha256": captures[offset:offset + count]})
    offset += count

def guard_summary(path):
    records = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
    if not records:
        raise SystemExit("memory guard emitted no JSONL evidence")
    for record in records:
        if record.get("schema") != "fn64.memory-guard.sample.v1":
            raise SystemExit("memory guard JSONL schema is invalid")
        for key in ("elapsed_seconds", "tree_rss_mib", "peak_tree_rss_mib"):
            if type(record.get(key)) is not int or record[key] < 0:
                raise SystemExit(f"memory guard JSONL {key} is invalid")
        if record.get("reason") not in ("sample", "complete"):
            raise SystemExit("memory guard JSONL contains a threshold or unknown reason")
    if records[-1].get("reason") != "complete":
        raise SystemExit("memory guard JSONL lacks a complete terminal record")
    return {
        "samples": len(records),
        "elapsed_seconds": max(record["elapsed_seconds"] for record in records),
        "peak_tree_rss_mib": max(record["peak_tree_rss_mib"] for record in records),
        "last_reason": records[-1].get("reason"),
    }

receipt = {
    "schema": "fn64.wm2000.withheld-pair-build-receipt.v3",
    "authority": "build_local_non_authoritative",
    "evidence_scope": "operational_build_local_artifact_identity_only",
    "privacy": "path-free-private-input-identities-only",
    "identity_relation": "pre_use_hashes_verified_unchanged_after_both_builds",
    "inputs": {
        "raw_rom_sha256": rom_sha,
        "boot_context_sha256": boot_context_sha,
        "executable_image_capture_groups": capture_groups,
        "manifest_sha256": manifest_sha,
        "lock_sha256": lock_sha,
    },
    "artifacts": {
        "aot_sha256": aot_sha,
        "dynamic_withheld_sha256": dynamic_sha,
        "aot_feature_tree_log_sha256": aot_feature_tree_sha,
        "pure_aot_checker_log_sha256": checker_log_sha,
        "dynamic_feature_tree_log_sha256": dynamic_feature_tree_sha,
        "dynamic_source_check_log_sha256": dynamic_source_check_sha,
        "target_subdir": target_subdir,
        "cargo_cache_seed": cache_seed_mode,
    },
    "guard": {
        "max_rss_mib": int(max_rss_mib),
        "min_free_percent": int(min_free_percent),
        "max_seconds": int(max_seconds),
        "poll_seconds": poll_seconds,
        "cargo_build_jobs": 1,
        "memory_guard_sha256": guard_sha,
        "pure_aot_checker_sha256": checker_sha,
        "cargo_sha256": cargo_sha,
        "aot_feature_check_jsonl": "aot-feature-check-memory-guard.jsonl",
        "dynamic_feature_check_jsonl": "dynamic-feature-check-memory-guard.jsonl",
        "dynamic_source_check_jsonl": "dynamic-source-check-memory-guard.jsonl",
        "aot_jsonl": "aot-memory-guard.jsonl",
        "dynamic_withheld_jsonl": "dynamic-withheld-memory-guard.jsonl",
        "aot_feature_check_measurement": guard_summary(aot_check_jsonl_path),
        "dynamic_feature_check_measurement": guard_summary(dynamic_check_jsonl_path),
        "dynamic_source_check_measurement": guard_summary(dynamic_source_check_jsonl_path),
        "aot_measurement": guard_summary(aot_jsonl_path),
        "dynamic_withheld_measurement": guard_summary(dynamic_jsonl_path),
    },
    "commands": {
        "pure_aot_feature_gate": [
            "<MEMORY_GUARD_BY_RECORDED_SHA256>", "<CARGO_BY_RECORDED_SHA256>",
            "tree", "--locked", "--manifest-path",
            "examples/wm2000-block-boot/Cargo.toml", "-e", "features", "-p",
            "wm2000-block-boot", "-i", "fn64-recomp-rs",
        ],
        "pure_aot_checker_gate": [
            "<SAME_MEMORY_GUARD_INVOCATION_AS_PURE_AOT_FEATURE_GATE>",
            "<PURE_AOT_CHECKER_BY_RECORDED_SHA256>",
            "stdout=<BOUND_PURE_AOT_CHECKER_LOG>",
        ],
        "dynamic_feature_gate": [
            "<MEMORY_GUARD_BY_RECORDED_SHA256>", "<CARGO_BY_RECORDED_SHA256>",
            "tree", "--locked", "--manifest-path",
            "examples/wm2000-block-boot/Cargo.toml", "-e", "features", "-p",
            "wm2000-block-boot", "-i", "fn64-recomp-rs", "--features", "dynamic-withheld",
        ],
        "dynamic_source_check": [
            "<MEMORY_GUARD_BY_RECORDED_SHA256>", "env", "CARGO_BUILD_JOBS=1",
            "CARGO_TARGET_DIR=<PRIVATE_OUTPUT>/cargo-target", "ROM=<PRIVATE_ROM>",
            "FN64_BOOT_CONTEXT=<PRIVATE_CAPTURE>",
            "FN64_EXECUTABLE_IMAGE_GROUPS=<CAPTURE_GROUP_NAMES_ONLY>",
            "<CARGO_BY_RECORDED_SHA256>", "check", "-j1", "--locked", "--manifest-path",
            "examples/wm2000-block-boot/Cargo.toml", "-p", "wm2000-block-boot",
            "--bin", "wm2000-block-boot", "--features", "dynamic-withheld",
        ],
        "aot": [
            "<MEMORY_GUARD_BY_RECORDED_SHA256>", "env", "CARGO_BUILD_JOBS=1",
            "CARGO_TARGET_DIR=<PRIVATE_OUTPUT>/cargo-target", "ROM=<PRIVATE_ROM>",
            "FN64_BOOT_CONTEXT=<PRIVATE_CAPTURE>",
            "FN64_EXECUTABLE_IMAGE_GROUPS=<CAPTURE_GROUP_NAMES_ONLY>",
            "<CARGO_BY_RECORDED_SHA256>", "build", "-j1", "--locked", "--manifest-path",
            "examples/wm2000-block-boot/Cargo.toml", "-p", "wm2000-block-boot",
        ],
        "dynamic_withheld": [
            "<MEMORY_GUARD_BY_RECORDED_SHA256>", "env", "CARGO_BUILD_JOBS=1",
            "CARGO_TARGET_DIR=<PRIVATE_OUTPUT>/cargo-target", "ROM=<PRIVATE_ROM>",
            "FN64_BOOT_CONTEXT=<PRIVATE_CAPTURE>",
            "FN64_EXECUTABLE_IMAGE_GROUPS=<CAPTURE_GROUP_NAMES_ONLY>",
            "<CARGO_BY_RECORDED_SHA256>", "build", "-j1", "--locked", "--manifest-path",
            "examples/wm2000-block-boot/Cargo.toml", "-p", "wm2000-block-boot",
            "--features", "dynamic-withheld",
        ],
    },
}
with open(receipt_path, "x", encoding="utf-8") as out:
    json.dump(receipt, out, indent=2, sort_keys=True)
    out.write("\n")
PY

pair_finished=1
print -- "wm2000 withheld pair build: AOT binary ${(q)pair_aot_binary}"
print -- "wm2000 withheld pair build: dynamic binary ${(q)pair_dynamic_binary}"
print -- "wm2000 withheld pair build: receipt ${(q)pair_receipt}"
