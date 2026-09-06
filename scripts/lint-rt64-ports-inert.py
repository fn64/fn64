#!/usr/bin/env python3
"""Assert the ported RT64 modules stay INERT until deliberately wired.

The conveyor's rule is parity first, wiring after: each `rt64_*` module is a
literal port with characterization tests and NO production admission.

This matters because the port inventory reads "192 of 276 ported", and it is
easy to hear that as working renderer code. It is not -- none of these modules
can affect a rendered frame. Real line coverage was measured at ~10-15%, not
the ~40% the digest-credited headline implied, and exactly one module is
production-wired (via an explicit call, not a re-export).

HOW INERTNESS IS ENFORCED, AFTER TASK 4.6
-----------------------------------------
The portfolio used to live in `fn64-render-wgpu/src` behind
`cfg(any(test, feature = "rt64-port-characterization"))`, and inertness was a
grep for `pub mod` / `pub use` over that crate's `lib.rs`. Task 4.6 of
`docs/plans/CLEANUP-2026-09.md` moved it to `fn64-rt64-characterization`,
which turns inertness into a structural property: that crate depends on
`fn64-render-wgpu`, and `fn64-render-wgpu` does not depend on it, so a port
there cannot reach a production draw path even if it is `pub`.

So this lint now checks the two things that can still break that:

1.  **The arrow does not reverse.** No crate in the workspace outside the
    characterization crate itself may depend on `fn64-rt64-characterization`.
    A dependency edge into it is exactly the "admitted without parity
    evidence" failure the old `pub mod` grep existed to catch, and it is the
    only way a port there can become reachable from a shipping build.

2.  **Ports do not sneak back into the backend.** `fn64-render-wgpu` may
    declare an `rt64_*` module only if it is in `DELIBERATELY_WIRED` below.
    A port module reintroduced there is reachable from a default build by
    construction, which is the same violation in the other direction.

Wiring one ON PURPOSE: move the module into `fn64-render-wgpu`, call it
explicitly (see `raw_dpc`'s use of `rt64_gbi_rdp_decode::decode_set_scissor`)
rather than re-exporting the portfolio, and record it in DELIBERATELY_WIRED
with the reason.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"
BACKEND_LIB = CRATES / "fn64-render-wgpu" / "src" / "lib.rs"
PORTFOLIO = "fn64-rt64-characterization"
PORTFOLIO_DIR = CRATES / PORTFOLIO

# Ports living in `fn64-render-wgpu` on purpose, each with the reason it
# earned production admission. Mirrored by `WIRED` in that crate's
# `characterization_gate_tests`.
DELIBERATELY_WIRED: dict[str, str] = {
    "rt64_gbi_rdp_decode": (
        "raw_dpc calls decode_set_scissor explicitly; the only production-"
        "wired port, and it was never inert"
    ),
    "rt64_blender_analysis": (
        "targets/texrect.rs's own tests compare the backend's cycle_count "
        "against this port's blend/combine_cycle_count; kept in place so the "
        "backend does not depend on the portfolio crate"
    ),
    "rt64_vi_registers": (
        "vi_scanout's own tests read the ported rt64_vi.h bitfield extents; "
        "kept in place for the same reason"
    ),
}

MOD_DECL = re.compile(r"(?m)^\s*(?:pub\s+)?mod\s+(rt64_[A-Za-z0-9_]+)\s*;")
# A dependency on the portfolio, in either the `foo = { path = ... }` or the
# `[dependencies.foo]` spelling.
DEP_INLINE = re.compile(rf"(?m)^\s*{re.escape(PORTFOLIO)}\s*=")
DEP_SECTION = re.compile(rf"(?m)^\s*\[[^\]]*dependencies\.{re.escape(PORTFOLIO)}\]")


def main() -> int:
    if not BACKEND_LIB.exists():
        print(f"rt64 inert lint: {BACKEND_LIB} not found", file=sys.stderr)
        return 1
    if not PORTFOLIO_DIR.is_dir():
        print(
            f"rt64 inert lint: {PORTFOLIO_DIR} not found -- the characterization "
            "portfolio is where the ports live; if it moved, update this lint",
            file=sys.stderr,
        )
        return 1

    violations: list[str] = []

    # (1) Nothing may depend on the portfolio crate.
    for manifest in sorted(CRATES.glob("*/Cargo.toml")):
        if manifest.parent.name == PORTFOLIO:
            continue
        text = manifest.read_text()
        if DEP_INLINE.search(text) or DEP_SECTION.search(text):
            violations.append(
                f"{manifest.relative_to(ROOT)}: depends on `{PORTFOLIO}` -- the "
                f"portfolio is inert precisely because nothing depends on it; a "
                f"dependency admits every port in it to that crate's build"
            )

    # (2) The backend declares only deliberately wired ports.
    for name in MOD_DECL.findall(BACKEND_LIB.read_text()):
        if name not in DELIBERATELY_WIRED:
            violations.append(
                f"{BACKEND_LIB.relative_to(ROOT)}: `mod {name};` -- a "
                f"characterization port declared in the renderer backend is "
                f"reachable from a default build; it belongs in {PORTFOLIO}"
            )

    if violations:
        print("RT64 ported modules must stay inert until wired:", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        print(
            "\nIf this is deliberate, add the module to DELIBERATELY_WIRED in\n"
            f"{pathlib.Path(__file__).name} with the reason it was wired, and\n"
            "mirror it in fn64-render-wgpu's `characterization_gate_tests::WIRED`.",
            file=sys.stderr,
        )
        return 1

    ports = sorted(p.stem for p in PORTFOLIO_DIR.glob("src/rt64_*.rs"))
    print(
        f"rt64 inert lint: {len(ports)} ported modules in {PORTFOLIO}, "
        f"inert (no crate depends on it); "
        f"{len(DELIBERATELY_WIRED)} deliberately wired in fn64-render-wgpu"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
