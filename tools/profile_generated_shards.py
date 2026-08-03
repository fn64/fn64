#!/usr/bin/env python3
"""Read-only structural profile of generated WM dense-AOT shard sources."""

from __future__ import annotations

import re
import sys
from pathlib import Path


PACKAGE_DIR = re.compile(r"^(?P<package>.+)-[0-9a-f]{16}$")
SELECTED = (
    b"verify_precompiled_instruction_word",
    b"advance_cop0_random",
    b"post_straight_instruction_exit",
    b"take_executable_write_boundary",
    b"executed >= budget.get",
    b"BlockExit::ExecutableWrite",
    b"BlockExit::Checkpoint",
)


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"profile_generated_shards: {message}")


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: tools/profile_generated_shards.py TARGET_DIR")
    target = Path(sys.argv[1]).resolve()
    build = target / "debug" / "build"
    if not build.is_dir():
        fail(f"no debug/build directory under {target}")

    all_runners: list[tuple[str, Path]] = []
    latest: dict[str, Path] = {}
    for runner in build.glob("*/out/runner.rs"):
        match = PACKAGE_DIR.fullmatch(runner.parents[1].name)
        if match is None:
            continue
        package = match.group("package")
        all_runners.append((package, runner))
        prior = latest.get(package)
        if prior is None or runner.stat().st_mtime_ns > prior.stat().st_mtime_ns:
            latest[package] = runner
    if not latest:
        fail(f"no generated shard runner.rs files under {build}")

    total_bytes = 0
    total_lines = 0
    selected_bytes = 0
    selected_lines = 0
    finish_invocations = 0
    for runner in latest.values():
        total_bytes += runner.stat().st_size
        with runner.open("rb") as source:
            for line in source:
                total_lines += 1
                if any(pattern in line for pattern in SELECTED):
                    selected_bytes += len(line)
                    selected_lines += 1
                finish_invocations += line.count(b"finish!(")

    all_runner_bytes = sum(path.stat().st_size for _, path in all_runners)
    rlibs = list((target / "debug" / "deps").glob("libwm2000_block_*.rlib"))
    rlib_bytes = sum(path.stat().st_size for path in rlibs)
    print(
        "generated_shards "
        f"target={target} packages={len(latest)} source_mib={total_bytes / 1048576:.1f} "
        f"source_lines={total_lines} selected_boilerplate_mib={selected_bytes / 1048576:.1f} "
        f"selected_boilerplate_percent={100.0 * selected_bytes / total_bytes:.1f} "
        f"selected_boilerplate_lines={selected_lines} finish_macro_invocations={finish_invocations}"
    )
    print(
        "generated_shard_cache "
        f"runner_copies={len(all_runners)} runner_gib={all_runner_bytes / 1073741824:.2f} "
        f"shard_rlibs={len(rlibs)} shard_rlib_gib={rlib_bytes / 1073741824:.2f}"
    )


if __name__ == "__main__":
    main()
