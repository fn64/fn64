#!/usr/bin/env python3
"""Validate and render fn64's base-renderer behavior denominator."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path


SCHEMA = "fn64.base-renderer-behavior-matrix.v1"
CATEGORIES = (
    "microcode",
    "command",
    "combiner",
    "blender",
    "depth",
    "coverage",
    "texture",
    "framebuffer",
    "vi",
    "integration",
)
EXPECTED_IDS = {
    "f3dex2-geometry-state",
    "legacy-fast3d-f3dex",
    "reject-geometry-variants",
    "line-microcodes",
    "s2dex-object-background",
    "f3dzex2-wave-other-microcodes",
    "raw-dpc-command-envelope",
    "rdp-command-state-order",
    "rsp-transform-lighting-texgen",
    "combiner-two-cycle-routing",
    "combiner-yuv-chroma-key",
    "blender-two-cycle-memory",
    "alpha-compare-and-dither",
    "depth-memory-encoding",
    "depth-raster-modes",
    "coverage-sample-mask",
    "coverage-alpha-memory",
    "tmem-load-layout-formats",
    "texture-address-filter-lod",
    "framebuffer-layout-hidden-bits",
    "vi-restoration-divot",
    "vi-gamma-dither",
    "vi-aa-resampling-analog",
    "full-rom-zero-unsupported",
}
EXACTNESS = {"exact_public", "bounded_reference", "missing"}
EVIDENCE_KINDS = {"test", "doc", "tool"}
BLOCKER_KINDS = {"hardware_trace", "full_rom", "allowed_spec", "implementation"}
ID_RE = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*\Z")


class MatrixError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise MatrixError(message)


def load_json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise MatrixError(f"cannot read {path}: {error}") from error
    require(isinstance(value, dict), "matrix root must be an object")
    return value


def validate_shape(matrix: dict, root: Path) -> list[dict]:
    require(matrix.get("schema") == SCHEMA, f"schema must be {SCHEMA!r}")
    require(
        set(matrix) == {"schema", "claim_id", "categories", "items"},
        "unknown or missing matrix root field",
    )
    require(matrix["claim_id"] == "base-rendering-accuracy", "unexpected claim_id")
    require(matrix["categories"] == list(CATEGORIES), "category denominator drifted")
    items = matrix["items"]
    require(isinstance(items, list), "items must be an array")
    require(items, "items must not be empty")

    seen: set[str] = set()
    category_counts: Counter[str] = Counter()
    blocker_counts: Counter[str] = Counter()
    for index, item in enumerate(items):
        where = f"items[{index}]"
        require(isinstance(item, dict), f"{where} must be an object")
        require(
            set(item)
            == {
                "id",
                "category",
                "title",
                "exactness",
                "behavior",
                "evidence",
                "blockers",
                "next_action",
            },
            f"{where}: unknown or missing field",
        )
        item_id = item["id"]
        require(isinstance(item_id, str) and ID_RE.fullmatch(item_id), f"{where}: invalid id")
        require(item_id not in seen, f"duplicate item id {item_id!r}")
        seen.add(item_id)
        category = item["category"]
        require(category in CATEGORIES, f"{item_id}: unknown category {category!r}")
        category_counts[category] += 1
        require(
            isinstance(item["title"], str) and item["title"].strip(),
            f"{item_id}: title must be nonempty",
        )
        exactness = item["exactness"]
        require(exactness in EXACTNESS, f"{item_id}: unknown exactness {exactness!r}")
        require(
            isinstance(item["behavior"], str) and item["behavior"].strip(),
            f"{item_id}: behavior must be nonempty",
        )
        require(
            isinstance(item["next_action"], str) and item["next_action"].strip(),
            f"{item_id}: next_action must be nonempty",
        )

        evidence = item["evidence"]
        require(isinstance(evidence, list) and evidence, f"{item_id}: evidence must be nonempty")
        evidence_keys: set[tuple[str, str, str]] = set()
        has_test = False
        for evidence_index, ref in enumerate(evidence):
            ref_where = f"{item_id}.evidence[{evidence_index}]"
            require(isinstance(ref, dict), f"{ref_where} must be an object")
            require(set(ref) == {"kind", "path", "needle"}, f"{ref_where}: invalid fields")
            kind = ref["kind"]
            require(kind in EVIDENCE_KINDS, f"{ref_where}: unknown kind {kind!r}")
            path_text = ref["path"]
            needle = ref["needle"]
            require(isinstance(path_text, str) and path_text, f"{ref_where}: invalid path")
            require(isinstance(needle, str) and needle, f"{ref_where}: invalid needle")
            path = Path(path_text)
            require(not path.is_absolute() and ".." not in path.parts, f"{ref_where}: path escapes repo")
            absolute = root / path
            require(absolute.is_file(), f"{ref_where}: missing {path_text}")
            try:
                source = absolute.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as error:
                raise MatrixError(f"{ref_where}: cannot read {path_text}: {error}") from error
            require(needle in source, f"{ref_where}: needle is stale in {path_text}")
            key = (kind, path_text, needle)
            require(key not in evidence_keys, f"{ref_where}: duplicate evidence")
            evidence_keys.add(key)
            has_test |= kind == "test"
        if exactness != "missing":
            require(has_test, f"{item_id}: implemented behavior requires test evidence")

        blockers = item["blockers"]
        require(isinstance(blockers, list), f"{item_id}: blockers must be an array")
        if exactness == "exact_public":
            require(not blockers, f"{item_id}: exact_public behavior cannot retain blockers")
        else:
            require(blockers, f"{item_id}: non-exact behavior requires an explicit blocker")
        item_blockers: set[str] = set()
        for blocker_index, blocker in enumerate(blockers):
            blocker_where = f"{item_id}.blockers[{blocker_index}]"
            require(isinstance(blocker, dict), f"{blocker_where} must be an object")
            require(set(blocker) == {"kind", "detail"}, f"{blocker_where}: invalid fields")
            kind = blocker["kind"]
            detail = blocker["detail"]
            require(kind in BLOCKER_KINDS, f"{blocker_where}: unknown kind {kind!r}")
            require(kind not in item_blockers, f"{item_id}: duplicate blocker kind {kind!r}")
            require(isinstance(detail, str) and detail.strip(), f"{blocker_where}: empty detail")
            item_blockers.add(kind)
            blocker_counts[kind] += 1

    require(seen == EXPECTED_IDS, f"item denominator drifted: missing={sorted(EXPECTED_IDS - seen)}, extra={sorted(seen - EXPECTED_IDS)}")
    require(set(category_counts) == set(CATEGORIES), "every category must have at least one item")
    require(blocker_counts["hardware_trace"] > 0, "matrix must name hardware-trace blockers")
    require(blocker_counts["full_rom"] > 0, "matrix must name full-ROM blockers")
    require(any(item["exactness"] != "exact_public" for item in items), "base accuracy cannot be closed while this matrix is the open denominator")
    return items


def validate_claim_guard(root: Path) -> None:
    inventory_path = root / "docs/rt64-public-feature-inventory.json"
    inventory = load_json(inventory_path)
    matching = [item for item in inventory.get("items", []) if item.get("id") == "base-rendering-accuracy"]
    require(len(matching) == 1, "public feature inventory must contain base-rendering-accuracy exactly once")
    claim = matching[0]
    require(claim.get("status") == "open", "base-rendering-accuracy must remain open")
    closure = claim.get("closure", "")
    require(
        "BASE-RENDERER-BEHAVIOR-MATRIX.md" in closure,
        "base claim no longer points at this closure denominator",
    )


def evidence_link(root: Path, ref: dict) -> str:
    path = root / ref["path"]
    source = path.read_text(encoding="utf-8").splitlines()
    line = next(index for index, text in enumerate(source, 1) if ref["needle"] in text)
    label = f"{Path(ref['path']).name}:{line}"
    return f"[{label}](../{ref['path']}#L{line})"


def render_doc(matrix: dict, items: list[dict], root: Path) -> str:
    exactness_counts = Counter(item["exactness"] for item in items)
    blocker_counts = Counter(
        blocker["kind"] for item in items for blocker in item["blockers"]
    )
    exactness_labels = {
        "exact_public": "exact public contract",
        "bounded_reference": "bounded reference",
        "missing": "missing",
    }
    blocker_labels = {
        "hardware_trace": "hardware trace",
        "full_rom": "full-ROM",
        "allowed_spec": "allowed specification",
        "implementation": "implementation",
    }
    lines = [
        "# Base-renderer behavior matrix",
        "",
        "Generated by `tools/check_base_renderer_matrix.py` from",
        "`docs/base-renderer-behavior-matrix.json`; do not edit this file by hand.",
        "",
        "This is the closure denominator for `base-rendering-accuracy`. `exact public",
        "contract` means the cited public behavior is implemented and directly tested; it",
        "does not promote unrelated silicon internals. `bounded reference` means a named,",
        "deterministic subset exists but at least one explicit boundary remains. `missing`",
        "means the behavior has no admissible implementation/evidence yet. The parent claim",
        "remains open until every row is exact and the full-ROM gate is clean.",
        "",
        "## Summary",
        "",
        "| Exactness | Count |",
        "|---|---:|",
    ]
    for exactness in ("exact_public", "bounded_reference", "missing"):
        lines.append(f"| {exactness_labels[exactness]} | {exactness_counts[exactness]} |")
    lines.extend(["", "| Blocker class | Rows |", "|---|---:|"])
    for blocker in ("hardware_trace", "full_rom", "allowed_spec", "implementation"):
        lines.append(f"| {blocker_labels[blocker]} | {blocker_counts[blocker]} |")
    lines.extend(
        [
            "",
            f"Exact denominator: **{exactness_counts['exact_public']}/{len(items)} rows**; base accuracy is **open**.",
            "",
            "## Behavior denominator",
            "",
            "| Category | ID | Behavior | Exactness | Evidence | Blockers | Next action |",
            "|---|---|---|---|---|---|---|",
        ]
    )
    for category in CATEGORIES:
        for item in (candidate for candidate in items if candidate["category"] == category):
            evidence = ", ".join(evidence_link(root, ref) for ref in item["evidence"])
            blockers = "none" if not item["blockers"] else "<br>".join(
                f"**{blocker_labels[blocker['kind']]}:** {blocker['detail']}"
                for blocker in item["blockers"]
            )
            lines.append(
                f"| {category} | `{item['id']}` | {item['behavior']} | "
                f"`{item['exactness']}` | {evidence} | {blockers} | {item['next_action']} |"
            )
    lines.extend(
        [
            "",
            "## Validation",
            "",
            "```sh",
            "python3 tools/check_base_renderer_matrix.py",
            "```",
            "",
            "The validator rejects denominator shrinkage, unknown categories or statuses,",
            "stale evidence paths/needles, implemented rows without test evidence, non-exact",
            "rows without blockers, loss of hardware/full-ROM blockers, a closed parent claim,",
            "and generated-doc drift. Use `--write-doc` only after changing the JSON source.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--matrix",
        type=Path,
        default=Path("docs/base-renderer-behavior-matrix.json"),
    )
    parser.add_argument(
        "--doc",
        type=Path,
        default=Path("docs/BASE-RENDERER-BEHAVIOR-MATRIX.md"),
    )
    parser.add_argument("--print-doc", action="store_true")
    parser.add_argument("--write-doc", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        require(not (args.print_doc and args.write_doc), "--print-doc and --write-doc are mutually exclusive")
        matrix = load_json(args.matrix)
        items = validate_shape(matrix, root)
        validate_claim_guard(root)
        rendered = render_doc(matrix, items, root)
        if args.print_doc:
            sys.stdout.write(rendered)
            return 0
        if args.write_doc:
            args.doc.write_text(rendered, encoding="utf-8")
            print(f"base-renderer-matrix: wrote {args.doc}")
            return 0
        try:
            checked = args.doc.read_text(encoding="utf-8")
        except OSError as error:
            raise MatrixError(f"cannot read generated doc {args.doc}: {error}") from error
        require(checked == rendered, f"generated doc is stale: {args.doc}; inspect with --print-doc")
    except (MatrixError, OSError) as error:
        print(f"base-renderer-matrix: {error}", file=sys.stderr)
        return 1
    counts = Counter(item["exactness"] for item in items)
    print(
        "base-renderer-matrix: clean "
        f"({counts['exact_public']} exact, {counts['bounded_reference']} bounded, "
        f"{counts['missing']} missing; {len(items)} total; claim open)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
