#!/usr/bin/env python3
"""
import_splat_syms.py — translate a splat-based decomp's config (splat.yaml +
symbol_addrs.txt) into the answer-key dump.toml shape fn64-discover's
reference adapter ingests (same [[section]] format as the OOTU dump).

WHY THIS SHAPE: splat-based decomps (Paper Mario, Kirby 64, Dinosaur
Planet, ...) declare their ROM/VRAM segment layout statically in splat.yaml
and their function symbols in symbol_addrs.txt — unlike zeldaret-style
builds (OoT, MM) where addresses only exist in the build-output .map (use
scripts/import_decomp_map.py for those). Both importers emit the same
dump.toml so the Rust side has exactly one answer-key format.

The output is decomp METADATA only (names, addresses, sizes) with cited
provenance — no ROM bytes, no source code, per the repo's no-ROM-bytes rule.

Honesty rules (mirrors fn64-discover's discipline):
- A symbol is only placed in a section when exactly one code segment can own
  it (by its `rom:` attribute when present, else by unambiguous VRAM
  containment). Overlay VRAM collisions without a `rom:` attribute are
  reported as ambiguous and SKIPPED, never guessed.
- A `rom:` attribute that contradicts the segment's affine ROM<->VRAM
  mapping is reported and skipped.
- Every skip category is counted in the banner and on stderr.

Usage:
    python3 scripts/import_splat_syms.py \
        --config <decomp>/ver/us/splat.yaml \
        --symbols <decomp>/ver/us/symbol_addrs.txt \
        --game "Paper Mario (US)" \
        --decomp-url https://github.com/pmret/papermario \
        --out games/PAPERMARIO/syms/dump.toml

Requires PyYAML. Self-check: python3 scripts/import_splat_syms.py --self-test
"""

import argparse
import re
import sys
from pathlib import Path

# Answer keys cover cartridge game code in cached KSEG0. RSP IMEM/boot
# symbols (0xA4000040-style KSEG1 device addresses) and KUSEG addresses are
# outside discovery's bank model and are counted, not guessed at.
KSEG0_START = 0x8000_0000
KSEG0_END = 0xA000_0000

SYMBOL_RE = re.compile(
    r"^\s*([A-Za-z_]\w*)\s*=\s*(0[xX][0-9A-Fa-f]+|\d+)\s*;\s*(?://(.*))?$"
)


def load_yaml(text):
    try:
        import yaml
    except ImportError:
        print(
            "ERROR: PyYAML not importable. Install it (pip install pyyaml) or "
            "run with a decomp venv's python3.",
            file=sys.stderr,
        )
        sys.exit(1)
    return yaml.safe_load(text)


def sanitize_name(name: str) -> str:
    # dump.toml [[section]] names feed identifier-shaped consumers (see the
    # OOTU importer's N64Recomp note): letters/digits/underscore only.
    clean = re.sub(r"[^0-9A-Za-z_]", "_", str(name))
    if not clean or clean[0].isdigit():
        clean = f"seg_{clean}"
    return clean


def code_segments(config):
    """Yield (name, rom_start, rom_end, vram, source_index) for every
    placeable top-level `type: code` segment. Segments without a numeric
    start or resolvable vram are counted, not guessed."""
    vram_classes = {
        c["name"]: c["vram"]
        for c in config.get("vram_classes", []) or []
        if isinstance(c, dict) and isinstance(c.get("vram"), int)
    }
    entries = config.get("segments", []) or []

    starts = []
    for seg in entries:
        if isinstance(seg, list) and seg and isinstance(seg[0], int):
            starts.append(seg[0])
        elif isinstance(seg, dict) and isinstance(seg.get("start"), int):
            starts.append(seg["start"])
    starts.sort()

    skipped = {"no_start": 0, "no_vram": 0}
    out = []
    for index, seg in enumerate(entries):
        if not (isinstance(seg, dict) and seg.get("type") == "code"):
            continue
        start = seg.get("start")
        if not isinstance(start, int):
            skipped["no_start"] += 1
            continue
        vram = seg.get("vram")
        if not isinstance(vram, int):
            vram = vram_classes.get(seg.get("vram_class"))
        if not isinstance(vram, int):
            skipped["no_vram"] += 1
            continue
        later = [s for s in starts if s > start]
        rom_end = min(later) if later else start
        name = seg.get("name") or f"seg_{start:06x}"
        out.append(
            {
                "name": sanitize_name(name),
                "rom": start,
                "rom_end": rom_end,
                "vram": vram,
                "index": index,
            }
        )
    return out, skipped


def parse_symbol_lines(lines):
    """Yield (name, vram, attrs) for every parseable symbol_addrs line."""
    for line in lines:
        m = SYMBOL_RE.match(line)
        if not m:
            continue
        name, addr, comment = m.group(1), int(m.group(2), 0), m.group(3) or ""
        attrs = {}
        for token in comment.split():
            if ":" in token:
                key, _, value = token.partition(":")
                attrs[key] = value
        yield name, addr, attrs


def attr_int(attrs, key):
    raw = attrs.get(key)
    if raw is None:
        return None
    try:
        return int(raw, 0)
    except ValueError:
        return None


def place_functions(segments, symbols):
    """Assign type:func symbols to segments. Returns (per-segment function
    lists, skip counters)."""
    skipped = {
        "not_func": 0,
        "outside_kseg0": 0,
        "rom_unplaced": 0,
        "rom_vram_inconsistent": 0,
        "suffix_vram_inconsistent": 0,
        "vram_unplaced": 0,
        "vram_ambiguous": 0,
    }
    placed = {seg["index"]: [] for seg in segments}
    by_name = {seg["name"]: seg for seg in segments}

    for name, vram, attrs in symbols:
        if attrs.get("type") != "func":
            skipped["not_func"] += 1
            continue
        if not KSEG0_START <= vram < KSEG0_END:
            skipped["outside_kseg0"] += 1
            continue
        size = attr_int(attrs, "size")
        rom = attr_int(attrs, "rom")
        # splat's per-segment symbol_name_format suffixes overlay symbols
        # with their segment name (func_80198880_ovl7) -- the only static
        # disambiguator for bank-slot overlays that share a VRAM window.
        # endswith (not rsplit) because segment names may contain
        # underscores; longest match wins when one name suffixes another.
        suffix_matches = [n for n in by_name if name.endswith(f"_{n}")]
        suffix_seg = (
            by_name[max(suffix_matches, key=len)] if suffix_matches else None
        )

        if rom is not None:
            owners = [s for s in segments if s["rom"] <= rom < s["rom_end"]]
            if len(owners) != 1:
                skipped["rom_unplaced"] += 1
                continue
            seg = owners[0]
            if vram - seg["vram"] != rom - seg["rom"]:
                skipped["rom_vram_inconsistent"] += 1
                continue
        elif suffix_seg is not None:
            seg = suffix_seg
            if not seg["vram"] <= vram < seg["vram"] + (seg["rom_end"] - seg["rom"]):
                skipped["suffix_vram_inconsistent"] += 1
                continue
        else:
            owners = [
                s
                for s in segments
                if s["vram"] <= vram < s["vram"] + (s["rom_end"] - s["rom"])
            ]
            if not owners:
                skipped["vram_unplaced"] += 1
                continue
            if len(owners) > 1:
                skipped["vram_ambiguous"] += 1
                continue
            seg = owners[0]

        placed[seg["index"]].append({"name": name, "vram": vram, "size": size})

    for funcs in placed.values():
        funcs.sort(key=lambda f: f["vram"])
        for current, following in zip(funcs, funcs[1:]):
            if current["size"] is None:
                current["size"] = following["vram"] - current["vram"]
        if funcs and funcs[-1]["size"] is None:
            funcs[-1]["size"] = 0
    return placed, skipped


def write_dump(out_path, tool, game, decomp_url, sha1, sections, counters):
    total = sum(len(s["functions"]) for s in sections)
    named = sum(
        1
        for s in sections
        for f in s["functions"]
        if not f["name"].startswith("func_")
    )
    skip_note = ", ".join(f"{k}={v}" for k, v in counters.items() if v)
    with open(out_path, "w") as f:
        f.write(
            f"# Autogenerated by {tool} from the {game} splat decomp's\n"
            f"# OWN CONFIG (splat.yaml + symbol_addrs.txt, {decomp_url}),\n"
            f"# targeting ROM sha1 {sha1}. Function names come from that\n"
            "# decomp's symbol file; func_XXXXXXXX entries are functions the\n"
            "# decomp itself has not yet named (honest upstream gap, not\n"
            "# ours). DO NOT hand-edit — rerun the importer instead.\n"
            "#\n"
            f"# Sections: {len(sections)}   Functions: {total}   "
            f"Real-named: {named}   Unnamed (func_*): {total - named}\n"
            f"# Skipped symbols: {skip_note or 'none'}\n\n"
        )
        for sec in sections:
            f.write("[[section]]\n")
            f.write(f'name = "{sec["name"]}"\n')
            f.write(f'rom = 0x{sec["rom"]:08x}\n')
            f.write(f'vram = 0x{sec["vram"]:08x}\n')
            f.write(f'size = 0x{sec["size"]:x}\n\n')
            f.write("functions = [\n")
            for fn in sec["functions"]:
                f.write(
                    f'    {{ name = "{fn["name"]}", vram = 0x{fn["vram"]:08x}, '
                    f'size = 0x{fn["size"]:x} }},\n'
                )
            f.write("]\n\n")
    return total, named


def run(config_text, symbol_texts):
    config = load_yaml(config_text)
    segments, seg_skips = code_segments(config)
    symbols = [
        sym for text in symbol_texts for sym in parse_symbol_lines(text.splitlines())
    ]
    placed, sym_skips = place_functions(segments, symbols)

    sections = []
    for seg in segments:
        funcs = placed[seg["index"]]
        if not funcs:
            continue
        sections.append(
            {
                "name": seg["name"],
                "rom": seg["rom"],
                "vram": seg["vram"],
                "size": seg["rom_end"] - seg["rom"],
                "functions": funcs,
            }
        )
    counters = {f"segment_{k}": v for k, v in seg_skips.items()}
    counters.update(sym_skips)
    return config, sections, counters


SELF_TEST_YAML = """
name: Fixture Game
sha1: 0000000000000000000000000000000000000000
vram_classes:
  - { name: main, vram: 0x80100000 }
segments:
  - name: header
    type: header
    start: 0x0
  - name: boot
    type: code
    start: 0x40
    vram: 0xA4000040
  - name: main
    type: code
    start: 0x1000
    vram_class: main
  - name: ovl_a
    type: code
    start: 0x3000
    vram: 0x80200000
  - name: ovl_b
    type: code
    start: 0x4000
    vram: 0x80200000
  - [0x5000]
"""

SELF_TEST_SYMS = """
func_A4000040 = 0xA4000040; // type:func rom:0x40
boot_main = 0x80100000; // type:func size:0x20
helper = 0x80100020; // type:func
tail = 0x80100400; // type:func
some_data = 0x80100800; // type:data
ovl_a_entry = 0x80200010; // type:func rom:0x3010
ovl_ambiguous = 0x80200100; // type:func
ovl_bad_rom = 0x80200200; // type:func rom:0x4300
func_80200300_ovl_b = 0x80200300; // type:func
func_80200400_ovl_a = 0x80200400; // type:func
func_90000000_ovl_b = 0x90000000; // type:func
"""


def self_test():
    config, sections, counters = run(SELF_TEST_YAML, [SELF_TEST_SYMS])
    by_name = {s["name"]: s for s in sections}

    assert config["name"] == "Fixture Game"
    # boot segment lives in RSP IMEM space (KSEG1 device addresses, above
    # 0x80000000 numerically!): excluded by the KSEG0 window, not a floor.
    assert "boot" not in by_name and counters["outside_kseg0"] == 1
    main = by_name["main"]
    assert main["rom"] == 0x1000 and main["vram"] == 0x80100000
    assert main["size"] == 0x2000  # bounded by ovl_a's start
    names = [f["name"] for f in main["functions"]]
    assert names == ["boot_main", "helper", "tail"]
    sizes = {f["name"]: f["size"] for f in main["functions"]}
    # explicit size attr wins; gap-derived size for helper; open tail = 0.
    assert sizes == {"boot_main": 0x20, "helper": 0x3E0, "tail": 0}
    # rom: attr disambiguates the ovl_a/ovl_b vram collision, and a
    # segment-name suffix does the same for symbols without a rom attr.
    assert [f["name"] for f in by_name["ovl_a"]["functions"]] == [
        "ovl_a_entry",
        "func_80200400_ovl_a",
    ]
    assert [f["name"] for f in by_name["ovl_b"]["functions"]] == ["func_80200300_ovl_b"]
    # ...vram-only placement in the collision is ambiguous and skipped...
    assert counters["vram_ambiguous"] == 1
    # ...a rom attr contradicting the affine mapping is skipped...
    assert counters["rom_vram_inconsistent"] == 1
    # ...and a suffix pointing at a segment that can't contain the vram is
    # skipped, not trusted.
    assert counters["suffix_vram_inconsistent"] == 1
    assert counters["not_func"] == 1
    print("self-test OK")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--config", type=Path, help="path to splat.yaml")
    ap.add_argument(
        "--symbols",
        type=Path,
        action="append",
        default=[],
        help="path to symbol_addrs.txt (repeatable)",
    )
    ap.add_argument("--out", type=Path, help="output dump.toml path")
    ap.add_argument("--game", help='display name, e.g. "Paper Mario (US)"')
    ap.add_argument("--decomp-url", help="decomp repo URL for the provenance banner")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        self_test()
        return 0
    if not (args.config and args.symbols and args.out and args.game and args.decomp_url):
        ap.error("--config, --symbols, --out, --game, --decomp-url are required")

    config, sections, counters = run(
        args.config.read_text(),
        [p.read_text() for p in args.symbols],
    )
    if not sections:
        print("ERROR: no function-bearing code sections produced", file=sys.stderr)
        return 1

    sha1 = config.get("sha1", "UNKNOWN")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    total, named = write_dump(
        args.out,
        "scripts/import_splat_syms.py",
        args.game,
        args.decomp_url,
        sha1,
        sections,
        counters,
    )
    print(f"Wrote {args.out}", file=sys.stderr)
    print("\n=== Import summary ===")
    print(f"Target ROM sha1 (from splat.yaml): {sha1}")
    print(f"Sections: {len(sections)}")
    print(f"Total functions: {total}")
    print(f"Real-named functions: {named}")
    print(f"Unnamed func_XXXXXXXX: {total - named}")
    print(f"Skipped: {({k: v for k, v in counters.items() if v}) or 'none'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
