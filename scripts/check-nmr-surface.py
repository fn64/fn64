#!/usr/bin/env python3
"""Check fn64's live ABI against the clean-room NMR compatibility inventory.

The canonical 116-name surface lives in crates/fn64-abi/nmr-surface.json.
This checker scans every production Rust module for exported `_recomp`
definitions, classifies direct `unimplemented!` bodies as traps and mixed
bodies containing `unimplemented!` as partial, and owns the generated block in
docs/COMPLETENESS.md.

Usage:
  scripts/check-nmr-surface.py                 print the live report
  scripts/check-nmr-surface.py --check-doc     fail if COMPLETENESS.md drifted
  scripts/check-nmr-surface.py --write-doc     refresh its generated block
  scripts/check-nmr-surface.py --require-complete
                                               fail unless all 116 are real
  scripts/check-nmr-surface.py --require-all-exports
                                               also fail on adjacent trap/partial bodies
  scripts/check-nmr-surface.py --selftest      prove the parser catches gaps
"""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "crates/fn64-abi/nmr-surface.json"
SOURCE_DIR = ROOT / "crates/fn64-abi/src"
DOC = ROOT / "docs/COMPLETENESS.md"
BEGIN = "<!-- BEGIN GENERATED NMR SURFACE -->"
END = "<!-- END GENERATED NMR SURFACE -->"
EXPORT = re.compile(
    r"#\[no_mangle\]\s*pub\s+(?:unsafe\s+)?extern\s+\"C\"\s+fn\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)_recomp\b",
    re.MULTILINE,
)
RAW_STRING_START = re.compile(r'r(#+)?"')


@dataclass(frozen=True)
class ExportedShim:
    symbol: str
    relative_path: str
    line: int
    body: str

    @property
    def status(self) -> str:
        scrubbed = scrub_rust(self.body).strip()
        if not re.search(r"\bunimplemented!\s*\(", scrubbed):
            return "implemented"
        if re.match(r"unimplemented!\s*\(", scrubbed):
            return "trap"
        return "partial"


def scrub_rust(text: str) -> str:
    """Blank comments and string contents while preserving code positions.

    Brace matching cannot count braces inside panic strings or doc comments.
    This small lexer covers Rust line/block comments, escaped strings, and raw
    strings; every removed byte becomes whitespace so diagnostics retain line
    numbers. Character literals in this ABI do not contain braces and need no
    special handling.
    """

    out = list(text)
    i = 0
    block_depth = 0
    while i < len(text):
        if block_depth:
            if text.startswith("/*", i):
                out[i : i + 2] = "  "
                block_depth += 1
                i += 2
            elif text.startswith("*/", i):
                out[i : i + 2] = "  "
                block_depth -= 1
                i += 2
            else:
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue

        if text.startswith("//", i):
            end = text.find("\n", i)
            if end == -1:
                end = len(text)
            out[i:end] = " " * (end - i)
            i = end
            continue
        if text.startswith("/*", i):
            out[i : i + 2] = "  "
            block_depth = 1
            i += 2
            continue

        # Match against the original source at an offset. Slicing `text[i:]`
        # here copies the remaining source on every lexer step and makes the
        # scan quadratic on large generated ABI modules.
        raw = RAW_STRING_START.match(text, i)
        if raw:
            hashes = raw.group(1) or ""
            start_len = len(raw.group(0))
            close = '"' + hashes
            end = text.find(close, i + start_len)
            end = len(text) if end == -1 else end + len(close)
            for j in range(i, end):
                if text[j] != "\n":
                    out[j] = " "
            i = end
            continue

        if text[i] == '"':
            j = i + 1
            escaped = False
            while j < len(text):
                if not escaped and text[j] == '"':
                    j += 1
                    break
                escaped = not escaped and text[j] == "\\"
                if text[j] != "\\":
                    escaped = False
                j += 1
            for k in range(i, j):
                if text[k] != "\n":
                    out[k] = " "
            i = j
            continue
        i += 1
    return "".join(out)


def function_body(source: str, signature_end: int) -> str:
    scrubbed = scrub_rust(source)
    opening = scrubbed.find("{", signature_end)
    if opening == -1:
        raise ValueError("extern function has no body")
    depth = 0
    for i in range(opening, len(scrubbed)):
        if scrubbed[i] == "{":
            depth += 1
        elif scrubbed[i] == "}":
            depth -= 1
            if depth == 0:
                return source[opening + 1 : i]
    raise ValueError("extern function body has unmatched braces")


def live_exports(source_dir: Path = SOURCE_DIR) -> dict[str, ExportedShim]:
    exports: dict[str, ExportedShim] = {}
    # rglob, not glob: shims live in subdirectory modules too (e.g. pi/mmio.rs,
    # task_dispatch/lifecycle.rs) since the large files were split. Skip test
    # modules -- exports are #[no_mangle] production definitions, never in tests.
    for path in sorted(source_dir.rglob("*.rs")):
        if path.name == "tests.rs" or "tests" in path.parts:
            continue
        source = path.read_text()
        for match in EXPORT.finditer(source):
            symbol = match.group("name")
            if symbol in exports:
                prior = exports[symbol]
                raise ValueError(
                    f"duplicate export {symbol}_recomp in {prior.relative_path} and "
                    f"{path.relative_to(ROOT)}"
                )
            try:
                relative_path = str(path.relative_to(ROOT))
            except ValueError:
                relative_path = path.name
            exports[symbol] = ExportedShim(
                symbol=symbol,
                relative_path=relative_path,
                line=source.count("\n", 0, match.start("name")) + 1,
                body=function_body(source, match.end()),
            )
    return exports


def load_manifest(path: Path = MANIFEST) -> tuple[dict, list[str]]:
    data = json.loads(path.read_text())
    symbols = [symbol for group in data["subsystems"] for symbol in group["symbols"]]
    if data.get("schema") != 1:
        raise ValueError(f"unsupported NMR surface schema {data.get('schema')!r}")
    if len(symbols) != data["denominator"]:
        raise ValueError(
            f"manifest denominator is {data['denominator']}, but it contains {len(symbols)} symbols"
        )
    duplicates = sorted({symbol for symbol in symbols if symbols.count(symbol) > 1})
    if duplicates:
        raise ValueError(f"duplicate canonical symbols: {', '.join(duplicates)}")
    return data, symbols


def statuses(canonical: list[str], exports: dict[str, ExportedShim]) -> dict[str, str]:
    return {
        symbol: exports[symbol].status if symbol in exports else "absent"
        for symbol in canonical
    }


def render_report(data: dict, canonical: list[str], exports: dict[str, ExportedShim]) -> str:
    status = statuses(canonical, exports)
    canonical_set = set(canonical)
    extras = sorted(set(exports) - canonical_set)
    headline = {
        name: list(status.values()).count(name)
        for name in ("implemented", "partial", "trap", "absent")
    }
    present = len(canonical) - headline["absent"]
    lines = [
        BEGIN,
        "_Generated by `scripts/check-nmr-surface.py` from "
        "`crates/fn64-abi/nmr-surface.json` and the live ABI source. Do not edit this block by hand._",
        "",
        "Status meanings: **implemented** has no `unimplemented!` path in the shim; "
        "**partial** has a real path plus a loud unimplemented branch; **trap** immediately "
        "traps; **absent** has no exported `_recomp` definition. These are source-shape "
        "classifications, not claims of hardware-exact behavior.",
        "",
        f"Live headline: **{present}/{len(canonical)} canonical shims are exported** — "
        f"{headline['implemented']} implemented, {headline['partial']} partial, "
        f"{headline['trap']} immediate traps, and {headline['absent']} absent.",
        "",
        "| Subsystem | Total | Implemented | Partial | Trap | Absent |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    totals = {name: 0 for name in ("implemented", "partial", "trap", "absent")}
    for group in data["subsystems"]:
        counts = {name: 0 for name in totals}
        for symbol in group["symbols"]:
            counts[status[symbol]] += 1
            totals[status[symbol]] += 1
        lines.append(
            f"| {group['name']} | {len(group['symbols'])} | {counts['implemented']} | "
            f"{counts['partial']} | {counts['trap']} | {counts['absent']} |"
        )
    lines.append(
        f"| **Total** | **{len(canonical)}** | **{totals['implemented']}** | "
        f"**{totals['partial']}** | **{totals['trap']}** | **{totals['absent']}** |"
    )
    lines.extend(["", "### Per-shim matrix", "", "| Subsystem | Shim | Status | Evidence |", "|---|---|---|---|"])
    for group in data["subsystems"]:
        for symbol in group["symbols"]:
            shim = exports.get(symbol)
            evidence = (
                f"`{shim.relative_path}:{shim.line}`" if shim else "no live export"
            )
            lines.append(
                f"| {group['name']} | `{symbol}_recomp` | **{status[symbol]}** | {evidence} |"
            )
    lines.extend(
        [
            "",
            "### Adjacent exports outside the 116",
            "",
            "These low-level or title-specific helpers are real ABI exports but are not part "
            "of N64Recomp's canonical 116-name `reimplemented_funcs` denominator:",
            "",
            "| Shim | Status | Evidence |",
            "|---|---|---|",
        ]
    )
    if extras:
        for symbol in extras:
            shim = exports[symbol]
            lines.append(
                f"| `{symbol}_recomp` | **{shim.status}** | "
                f"`{shim.relative_path}:{shim.line}` |"
            )
    else:
        lines.append("| _None._ | — | — |")
    lines.append(END)
    return "\n".join(lines)


def replace_generated_block(doc: str, report: str) -> str:
    if BEGIN not in doc or END not in doc:
        raise ValueError(f"{DOC.relative_to(ROOT)} is missing generated block markers")
    before, rest = doc.split(BEGIN, 1)
    _, after = rest.split(END, 1)
    return before.rstrip() + "\n\n" + report + "\n\n" + after.lstrip("\n")


def check_doc(report: str) -> bool:
    current = DOC.read_text()
    expected = replace_generated_block(current, report)
    if current == expected:
        return True
    print(
        "NMR surface doc drift: docs/COMPLETENESS.md does not match the live ABI.\n"
        "Run scripts/check-nmr-surface.py --write-doc and review the diff.",
        file=sys.stderr,
    )
    return False


def selftest() -> int:
    fixture = """
#[no_mangle]
pub unsafe extern "C" fn real_recomp(_r: *mut u8) { let s = "}"; consume(s); }
#[no_mangle]
pub extern "C" fn trap_recomp() { unimplemented!("named { trap }") }
#[no_mangle]
pub extern "C" fn partial_recomp() { if condition() { unimplemented!("branch") } done(); }
"""
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp)
        (path / "fixture.rs").write_text(fixture)
        found = live_exports(path)
    expected = {
        "real": "implemented",
        "trap": "trap",
        "partial": "partial",
        "missing": "absent",
    }
    actual = statuses(list(expected), found)
    if actual != expected:
        print(f"NMR surface selftest failed: expected {expected}, got {actual}", file=sys.stderr)
        return 1
    print("NMR surface checker selftest: implemented/partial/trap/absent classification 4/4")
    return 0


def main(argv: list[str]) -> int:
    allowed = {
        "--check-doc",
        "--write-doc",
        "--require-complete",
        "--require-all-exports",
        "--selftest",
    }
    unknown = set(argv) - allowed
    if unknown or ("--check-doc" in argv and "--write-doc" in argv):
        print(__doc__, file=sys.stderr)
        return 2
    if "--selftest" in argv:
        return selftest()

    try:
        data, canonical = load_manifest()
        exports = live_exports()
        report = render_report(data, canonical, exports)
    except (KeyError, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"NMR surface check failed: {error}", file=sys.stderr)
        return 1

    if "--write-doc" in argv:
        DOC.write_text(replace_generated_block(DOC.read_text(), report))
        print("Updated docs/COMPLETENESS.md from the live ABI surface")
    elif "--check-doc" in argv:
        if not check_doc(report):
            return 1
        print("NMR surface doc gate: live ABI matches docs/COMPLETENESS.md")
    else:
        print(report)

    if "--require-complete" in argv:
        status = statuses(canonical, exports)
        incomplete = [symbol for symbol in canonical if status[symbol] != "implemented"]
        if incomplete:
            counts = {name: list(status.values()).count(name) for name in set(status.values())}
            print(
                "NMR full-parity gate: NOT MET "
                f"({counts.get('implemented', 0)}/116 implemented; "
                f"{counts.get('partial', 0)} partial, {counts.get('trap', 0)} trap, "
                f"{counts.get('absent', 0)} absent)",
                file=sys.stderr,
            )
            return 1
        print("NMR full-parity gate: 116/116 implemented, zero partial/trap/absent")
    if "--require-all-exports" in argv:
        canonical_set = set(canonical)
        incomplete = [
            symbol
            for symbol, shim in sorted(exports.items())
            if symbol not in canonical_set and shim.status != "implemented"
        ]
        if incomplete:
            detail = ", ".join(
                f"{symbol}_recomp={exports[symbol].status}" for symbol in incomplete
            )
            print(
                f"All-export gate: NOT MET ({detail})",
                file=sys.stderr,
            )
            return 1
        extras = sorted(set(exports) - canonical_set)
        print(
            f"All-export gate: {len(canonical) + len(extras)}/{len(canonical) + len(extras)} "
            "live shims implemented, zero partial/trap bodies"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
