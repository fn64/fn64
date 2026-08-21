#!/usr/bin/env python3
"""Assert the ported RT64 modules stay INERT until deliberately wired.

The conveyor's rule is parity first, wiring after: each `rt64_*` module is a
literal port with characterization tests and NO production admission. That is
enforced by declaring them `mod`, never `pub mod`, and never re-exporting
them with `pub use`.

This matters because the port inventory reads "192 of 276 ported", and it is
easy to hear that as working renderer code. It is not -- none of these modules
can affect a rendered frame. Real line coverage was measured at ~10-15%, not
the ~40% the digest-credited headline implied, and exactly one module is
production-wired (via an explicit call, not a re-export).

The invariant is one grep, so it is cheap to check and cheap to forget. A
module that silently becomes `pub` is admitted to production without the
parity evidence the conveyor exists to require -- so this fails loudly, and
names the allowlist as the way to say "yes, deliberately".

Wiring one ON PURPOSE: call into it explicitly (see `raw_dpc`'s use of
`rt64_gbi_rdp_decode::decode_set_scissor`) rather than re-exporting it, or
add it to DELIBERATELY_PUBLIC below with the reason.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
LIB = ROOT / "crates" / "fn64-render-wgpu" / "src" / "lib.rs"

# Modules intentionally made public, each with the reason it earned it.
DELIBERATELY_PUBLIC: dict[str, str] = {}

PUB_MOD = re.compile(r"(?m)^\s*pub\s+mod\s+(rt64_[A-Za-z0-9_]+)\s*;")
PUB_USE = re.compile(r"(?m)^\s*pub\s+use\s+(rt64_[A-Za-z0-9_]+)\b")
PLAIN_MOD = re.compile(r"(?m)^\s*mod\s+(rt64_[A-Za-z0-9_]+)\s*;")


def main() -> int:
    if not LIB.exists():
        print(f"rt64 inert lint: {LIB} not found", file=sys.stderr)
        return 1

    source = LIB.read_text()
    inert = PLAIN_MOD.findall(source)
    violations: list[str] = []

    for name in PUB_MOD.findall(source):
        if name not in DELIBERATELY_PUBLIC:
            violations.append(
                f"`pub mod {name};` -- ported modules are `mod`, not `pub mod`"
            )
    for name in PUB_USE.findall(source):
        if name not in DELIBERATELY_PUBLIC:
            violations.append(
                f"`pub use {name}` -- re-exporting admits a port to production "
                f"without parity evidence"
            )

    if violations:
        print("RT64 ported modules must stay inert until wired:", file=sys.stderr)
        for v in violations:
            print(f"  {LIB.relative_to(ROOT)}: {v}", file=sys.stderr)
        print(
            "\nIf this is deliberate, add the module to DELIBERATELY_PUBLIC in\n"
            f"{pathlib.Path(__file__).name} with the reason it was wired.",
            file=sys.stderr,
        )
        return 1

    allowed = f", {len(DELIBERATELY_PUBLIC)} deliberately public" if DELIBERATELY_PUBLIC else ""
    print(f"rt64 inert lint: {len(inert)} ported modules, all inert{allowed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
