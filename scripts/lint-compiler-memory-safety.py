#!/usr/bin/env python3
"""Fail closed when production compiler entrypoints lose the memory guard."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SHELL_ENTRYPOINTS = (
    "scripts/guarded-cargo-test.zsh",
    "scripts/guarded-nextest.zsh",
    "scripts/guarded-cargo-build.zsh",
    "scripts/profile-wm2000-shard.zsh",
    "scripts/benchmark-wm-prepared-invalidation.zsh",
    "scripts/verify-wm-prepared-parity.zsh",
    "scripts/lane-parity.sh",
    "scripts/native-emit.sh",
    "scripts/capture-wm-executable-image-group.zsh",
    "scripts/current-static-scorecard.zsh",
)
DEFAULT_ENTRYPOINTS = ("scripts/memory-guard.zsh", *SHELL_ENTRYPOINTS)
LOCAL_TEST_ENTRYPOINTS = (
    "scripts/guarded-cargo-test.zsh",
    "scripts/guarded-nextest.zsh",
)
GENERATED_BUILD = "crates/fn64-boot-harness/src/generated_runner_build.rs"
COMPILER_COMMAND = re.compile(r"\bcargo\s+(?:build|test|check|metadata|nextest|run)\b")


def shell_commands(source: str) -> list[str]:
    commands: list[str] = []
    current: list[str] = []
    for line in source.splitlines():
        stripped = line.strip()
        if not current and (not stripped or stripped.startswith("#")):
            continue
        current.append(line)
        if line.rstrip().endswith("\\"):
            continue
        commands.append("\n".join(current))
        current = []
    if current:
        commands.append("\n".join(current))
    return commands


def check_sources(sources: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for path in DEFAULT_ENTRYPOINTS:
        source = sources[path]
        expected_rss = 4096 if path in LOCAL_TEST_ENTRYPOINTS else 2048
        if f"FN64_GUARD_MAX_RSS_MIB:-{expected_rss}" not in source:
            errors.append(f"{path}: missing {expected_rss} MiB default")
        if "FN64_GUARD_MIN_FREE_PERCENT:-40" not in source:
            errors.append(f"{path}: missing 40% free-memory default")
        for unsafe in (
            "FN64_GUARD_MAX_RSS_MIB:-10240",
            "FN64_GUARD_MIN_FREE_PERCENT:-25",
        ):
            if unsafe in source:
                errors.append(f"{path}: unsafe guard default {unsafe}")

    for path in SHELL_ENTRYPOINTS:
        source = sources[path]
        for command in shell_commands(source):
            executable_text = re.sub(r'"(?:\\.|[^"\\])*"|\'[^\']*\'', "", command)
            compiler = COMPILER_COMMAND.search(executable_text)
            if not compiler:
                continue
            if "memory-guard.zsh" not in command and '"$guard"' not in command:
                headline = command.strip().splitlines()[0]
                errors.append(f"{path}: unguarded Cargo compiler command: {headline}")
            if compiler.group(0).split()[-1] != "metadata" and "-j1" not in executable_text:
                headline = command.strip().splitlines()[0]
                errors.append(f"{path}: Cargo compiler command is not serialized: {headline}")

    guarded_build = sources["scripts/guarded-cargo-build.zsh"]
    if 'cargo "$cargo_operation" -j1' not in guarded_build:
        errors.append("scripts/guarded-cargo-build.zsh: dynamic Cargo operation is not serial")
    if 'exec "$repo_root/scripts/memory-guard.zsh"' not in guarded_build:
        errors.append("scripts/guarded-cargo-build.zsh: dynamic Cargo operation is unguarded")

    for path in LOCAL_TEST_ENTRYPOINTS:
        if "export CARGO_BUILD_JOBS=1" not in sources[path]:
            errors.append(f"{path}: Cargo compile jobs are not bound to one")
    guarded_nextest = sources["scripts/guarded-nextest.zsh"]
    if 'cargo nextest run -j1 "$@"' not in guarded_nextest:
        errors.append("scripts/guarded-nextest.zsh: nextest processes are not serialized")

    native_emit = sources["scripts/native-emit.sh"]
    if '"$guard" "$DRIVER" --config' not in native_emit:
        errors.append("scripts/native-emit.sh: ROM emitter execution is unguarded")
    lane_parity = sources["scripts/lane-parity.sh"]
    if 'OOT_MAX_SWAPS="$SWAPS" "$guard" "$bin"' not in lane_parity:
        errors.append("scripts/lane-parity.sh: boot lane execution is unguarded")
    capture_group = sources["scripts/capture-wm-executable-image-group.zsh"]
    if 'FN64_GUARD_MAX_SECONDS=$timeout_text "$guard" env -i' not in capture_group:
        errors.append(
            "scripts/capture-wm-executable-image-group.zsh: producer execution is unguarded"
        )
    if (
        '"$guard" cargo run -q -j1 -p fn64-discover '
        '--bin validate_executable_image_group --' not in capture_group
    ):
        errors.append(
            "scripts/capture-wm-executable-image-group.zsh: canonical validator is unguarded"
        )
    current_scorecard = sources["scripts/current-static-scorecard.zsh"]
    if "typeset -r selected_build_guard_max_rss_mib=4096" not in current_scorecard or (
        '"FN64_GUARD_MAX_RSS_MIB=$selected_build_guard_max_rss_mib"'
        not in current_scorecard
    ):
        errors.append(
            "scripts/current-static-scorecard.zsh: selected-build outer guard is not fixed at 4096 MiB"
        )

    generated = sources[GENERATED_BUILD]
    if "const BUILD_MAX_RSS_MIB: u32 = 4096;" not in generated:
        errors.append(f"{GENERATED_BUILD}: generated-build authority is not fixed at 4096 MiB")
    if "const BUILD_MIN_FREE_PERCENT: u8 = 40;" not in generated:
        errors.append(f"{GENERATED_BUILD}: generated-build authority is not fixed at 40% free")
    if "const SELECTED_BUILD_CARGO_JOBS_V5: u16 = 2;" not in generated:
        errors.append(f"{GENERATED_BUILD}: selected build is not fixed at two Cargo jobs")
    if 'Some("2048")' in generated:
        errors.append(f"{GENERATED_BUILD}: stale 2048 MiB authority expectation")
    for binding in (
        '.arg(format!("-j{SELECTED_BUILD_CARGO_JOBS_V5}"))',
        '.env("CARGO_BUILD_JOBS", SELECTED_BUILD_CARGO_JOBS_V5.to_string())',
        '.env("FN64_GUARD_MAX_RSS_MIB", BUILD_MAX_RSS_MIB.to_string())',
        '"FN64_GUARD_MIN_FREE_PERCENT",\n            BUILD_MIN_FREE_PERCENT.to_string(),',
    ):
        if binding not in generated:
            errors.append(f"{GENERATED_BUILD}: missing exact guard environment binding")
    if "fn apply(&self, command: &mut Command) {\n        command\n            .env_clear()" not in generated:
        errors.append(f"{GENERATED_BUILD}: selected build environment is not cleared")
    if generated.count("FN64_GUARD_MAX_RSS_MIB") < 3:
        errors.append(f"{GENERATED_BUILD}: not every owned build phase binds the RSS guard")
    if generated.count("FN64_GUARD_MIN_FREE_PERCENT") < 3:
        errors.append(f"{GENERATED_BUILD}: not every owned build phase binds the free-memory guard")
    return errors


def repository_sources() -> dict[str, str]:
    paths = (*DEFAULT_ENTRYPOINTS, GENERATED_BUILD)
    return {path: (ROOT / path).read_text(encoding="utf-8") for path in paths}


def selftest(sources: dict[str, str]) -> None:
    if check_sources(sources):
        raise AssertionError("live sources must pass before negative fixtures")

    unsafe = dict(sources)
    unsafe["scripts/memory-guard.zsh"] = unsafe["scripts/memory-guard.zsh"].replace(
        "FN64_GUARD_MAX_RSS_MIB:-2048", "FN64_GUARD_MAX_RSS_MIB:-4096", 1
    )
    if not check_sources(unsafe):
        raise AssertionError("unsafe-default fixture was accepted")

    stale_authority = dict(sources)
    stale_authority[GENERATED_BUILD] = stale_authority[GENERATED_BUILD].replace(
        'Some("4096")', 'Some("2048")', 1
    )
    if not any("stale 2048 MiB" in error for error in check_sources(stale_authority)):
        raise AssertionError("stale generated-build authority fixture was accepted")

    unguarded = dict(sources)
    unguarded["scripts/verify-wm-prepared-parity.zsh"] = unguarded[
        "scripts/verify-wm-prepared-parity.zsh"
    ].replace('"$guard" cargo build', "cargo build", 1)
    if not any("unguarded Cargo compiler command" in error for error in check_sources(unguarded)):
        raise AssertionError("unguarded-Cargo fixture was accepted")

    parallel = dict(sources)
    parallel["scripts/lane-parity.sh"] = parallel["scripts/lane-parity.sh"].replace(
        "cargo test -j1", "cargo test -j2", 1
    )
    if not any("not serialized" in error for error in check_sources(parallel)):
        raise AssertionError("parallel-Cargo fixture was accepted")

    parallel_nextest = dict(sources)
    parallel_nextest["scripts/guarded-nextest.zsh"] = parallel_nextest[
        "scripts/guarded-nextest.zsh"
    ].replace("export CARGO_BUILD_JOBS=1", "export CARGO_BUILD_JOBS=2", 1)
    if not any(
        "compile jobs are not bound to one" in error
        for error in check_sources(parallel_nextest)
    ):
        raise AssertionError("parallel-nextest compile fixture was accepted")

    unguarded_capture = dict(sources)
    unguarded_capture["scripts/capture-wm-executable-image-group.zsh"] = unguarded_capture[
        "scripts/capture-wm-executable-image-group.zsh"
    ].replace('FN64_GUARD_MAX_SECONDS=$timeout_text "$guard" env -i', "env -i", 1)
    if not any("producer execution is unguarded" in error for error in check_sources(unguarded_capture)):
        raise AssertionError("unguarded capture-producer fixture was accepted")

    wrapper_unguarded = dict(sources)
    wrapper_unguarded["scripts/current-static-scorecard.zsh"] = wrapper_unguarded[
        "scripts/current-static-scorecard.zsh"
    ].replace('"$repo_root/scripts/memory-guard.zsh" cargo run', "cargo run", 1)
    if not any(
        "unguarded Cargo compiler command" in error
        for error in check_sources(wrapper_unguarded)
    ):
        raise AssertionError("unguarded scorecard-writer fixture was accepted")

    unbound_selected_jobs = dict(sources)
    unbound_selected_jobs[GENERATED_BUILD] = unbound_selected_jobs[GENERATED_BUILD].replace(
        "const SELECTED_BUILD_CARGO_JOBS_V5: u16 = 2;",
        "const SELECTED_BUILD_CARGO_JOBS_V5: u16 = 3;",
        1,
    )
    if not any(
        "selected build is not fixed at two Cargo jobs" in error
        for error in check_sources(unbound_selected_jobs)
    ):
        raise AssertionError("unbound selected-build job fixture was accepted")

    stale_outer_cap = dict(sources)
    stale_outer_cap["scripts/current-static-scorecard.zsh"] = stale_outer_cap[
        "scripts/current-static-scorecard.zsh"
    ].replace("selected_build_guard_max_rss_mib=4096", "selected_build_guard_max_rss_mib=2048", 1)
    if not any(
        "selected-build outer guard is not fixed at 4096 MiB" in error
        for error in check_sources(stale_outer_cap)
    ):
        raise AssertionError("stale selected-build outer-cap fixture was accepted")


def main() -> int:
    sources = repository_sources()
    errors = check_sources(sources)
    if errors:
        for error in errors:
            print(f"compiler memory safety lint: {error}", file=sys.stderr)
        return 1
    if sys.argv[1:] == ["--selftest"]:
        selftest(sources)
    elif sys.argv[1:]:
        print("usage: scripts/lint-compiler-memory-safety.py [--selftest]", file=sys.stderr)
        return 2
    print(
        "compiler memory safety lint: PASS "
        "(2048 MiB ordinary production, 4096 MiB selected build/local tests, 40% free; "
        "scoped Cargo commands guarded; selected build authority-bound to two jobs)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
