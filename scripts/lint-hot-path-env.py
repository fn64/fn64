#!/usr/bin/env python3
"""Reject direct process-environment reads from registered runtime hot paths."""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGETS = {
    "crates/fn64-audio/src/rsp/interpreter.rs": ("run_imem",),
    "crates/fn64-audio/src/rsp/recomp/runtime/mod.rs": (
        "write_cp0",
        "dma_read",
        "dma_write",
    ),
    "crates/fn64-abi/src/dispatch.rs": ("note_dma_overlay_load",),
    "crates/fn64-abi/src/recompiled/runners.rs": ("call_c",),
    "crates/fn64-abi/src/si/mod.rs": ("osContGetReadData_recomp",),
    "crates/fn64-abi/src/task_dispatch/setup.rs": (
        "deliver_ai_buffer",
        "dump_audio_pcm_stream",
    ),
    "crates/fn64-abi/src/task_dispatch/rsp_commit.rs": ("dispatch_captured_raw_rdp",),
}

FUNCTION = re.compile(
    r"(?m)^(?P<indent>[ \t]*)(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"C\"\s+)?fn\s+(?P<name>[A-Za-z0-9_]+)\b"
)
ENV_READ = re.compile(r"(?:std::)?env::(?:var|var_os)\s*\(")
ITEM = re.compile(
    r"(?m)^(?P<indent>[ \t]*)(?:#\[|(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:(?:unsafe\s+)?(?:extern\s+\"C\"\s+)?fn\s+|"
    r"(?:struct|enum|impl|mod|const|static|type)\b))"
)


def function_body(source: str, name: str, path: pathlib.Path) -> tuple[str, int]:
    matches = list(FUNCTION.finditer(source))
    for index, match in enumerate(matches):
        if match.group("name") != name:
            continue
        indent = len(match.group("indent"))
        end = len(source)
        for following in ITEM.finditer(source, match.end()):
            if len(following.group("indent")) <= indent:
                end = following.start()
                break
        return source[match.start() : end], source.count("\n", 0, match.start()) + 1
    raise ValueError(f"{path.relative_to(ROOT)}: registered hot function {name!r} is missing")


def main() -> int:
    failures: list[str] = []
    checked = 0
    for relative, functions in TARGETS.items():
        path = ROOT / relative
        source = path.read_text(encoding="utf-8")
        for name in functions:
            body, first_line = function_body(source, name, path)
            checked += 1
            for match in ENV_READ.finditer(body):
                line = first_line + body.count("\n", 0, match.start())
                failures.append(
                    f"{relative}:{line}: {name} reads the process environment directly; "
                    "parse launch-time diagnostics through a typed OnceLock-backed helper"
                )
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"hot-path env lint: {checked} registered functions are free of direct env reads")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
