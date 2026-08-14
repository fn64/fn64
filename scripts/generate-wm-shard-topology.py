#!/usr/bin/env python3
"""Generate the ROM-dependent dense-AOT Cargo topology for an AKI title."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
# The shared generator module and the boot Cargo.toml template this script
# splices a dependency block into both live in the committed WM2000 tree
# regardless of which title is being generated -- these are read-only
# sources, not the per-title output paths (see write_topology / --title).
SOURCE_SHARDS = ROOT / "examples" / "wm2000-block-shards"
SOURCE_BOOT = ROOT / "examples" / "wm2000-block-boot"

# Matches FN64_WM_SHARD_TITLE's own default in crates/fn64-boot-harness/build.rs
# (DEFAULT_WM_SHARD_DIR) -- same convention, same string, so an unspecified
# --title reproduces today's WM2000 output byte-for-byte.
DEFAULT_TITLE = "wm2000-block-shards"


def fail(message: str) -> None:
    raise SystemExit(f"generate WM shard topology: {message}")


def validate_title(title: str) -> None:
    """Mirror crates/fn64-boot-harness/build.rs::validate_shard_title.

    This validates shape (bare path segment, safe characters), not identity --
    unlike the package-name shapes, drift here fails loudly as a bad directory
    name or a bad `include!` path, not a silent mismatch, so duplicating the
    check instead of shelling out to Rust for it is low-risk.
    """
    if not title:
        fail("--title must not be empty")
    if title in (".", ".."):
        fail(f"--title must not be a directory-traversal segment: {title!r}")
    if "/" in title or "\\" in title:
        fail(f"--title must be a single path segment, got {title!r}")
    if not re.fullmatch(r"[A-Za-z0-9_-]+", title):
        fail(
            "--title must contain only ASCII alphanumerics, '-' and '_', "
            f"got {title!r}"
        )


def package_prefix_for_title(title: str) -> str:
    """`<prefix>-shards` -> `<prefix>`, matching the existing directory <->
    package-prefix convention (`wm2000-block-shards` directory,
    `wm2000-block` package prefix, `wm2000-block-boot` sibling directory)."""
    return title.removesuffix("-shards")


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rom", required=True, type=Path)
    parser.add_argument(
        "--output-root",
        required=True,
        type=Path,
        help="repository-shaped output root (may be the fn64 checkout)",
    )
    parser.add_argument(
        "--title",
        default=DEFAULT_TITLE,
        help=(
            "bare directory name under examples/ to generate into, and the "
            "source of the Cargo package-name prefix (same convention as "
            f"FN64_WM_SHARD_TITLE; default {DEFAULT_TITLE!r} reproduces the "
            "committed WM2000 tree byte-for-byte)"
        ),
    )
    arguments = parser.parse_args()
    validate_title(arguments.title)
    return arguments


def rust_string(value: Path) -> str:
    return json.dumps(os.fspath(value))


def discover_inventory(rom: Path, package_prefix: str) -> list[tuple[str, str]]:
    if not rom.is_absolute() or not rom.is_file():
        fail("--rom must name an absolute regular file")
    with tempfile.TemporaryDirectory(prefix="fn64-shard-topology.") as temporary:
        package = Path(temporary)
        build_rs = SOURCE_SHARDS / "build.rs"
        # The prefix is a Python value passed to Rust as a plain argument, not
        # re-derived by either side: Python builds package_prefix once (from
        # --title) and uses that same string both here and in
        # execution_order's regexes, so there is nothing left to drift.
        main_source = f'''#[allow(dead_code)]
#[path = {rust_string(build_rs)}]
mod generator;

fn main() {{
    let mut args = std::env::args_os();
    let rom_path = args.nth(1).expect("ROM argument");
    let package_prefix = args.next().expect("package-prefix argument");
    let package_prefix = package_prefix.to_str().expect("package prefix is UTF-8");
    let source = std::fs::read(rom_path).expect("reading ROM");
    let mut generator =
        generator::WmShardGenerator::from_rom_bytes_for_topology(&source, package_prefix);
    for (package, directory) in generator.package_inventory() {{
        println!("{{package}}\\t{{directory}}");
    }}
}}
'''
        manifest = f'''[package]
name = "fn64-wm-shard-topology-probe"
version = "0.0.0"
edition = "2021"
publish = false

[[bin]]
name = "fn64-wm-shard-topology-probe"
path = "main.rs"

[dependencies]
fn64-discover = {{ path = {rust_string(ROOT / "crates/fn64-discover")} }}
fn64-recomp-rs-codegen = {{ path = {rust_string(ROOT / "crates/fn64-recomp-rs-codegen")} }}
sha2 = "0.10"

[workspace]
'''
        (package / "main.rs").write_text(main_source)
        (package / "Cargo.toml").write_text(manifest)
        environment = os.environ.copy()
        environment["CARGO_BUILD_JOBS"] = "1"
        environment["CARGO_TARGET_DIR"] = os.fspath(ROOT / "target" / "wm-shard-topology")
        result = subprocess.run(
            [
                "cargo",
                "run",
                "--quiet",
                "--release",
                "--manifest-path",
                os.fspath(package / "Cargo.toml"),
                "--",
                os.fspath(rom),
                package_prefix,
            ],
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            env=environment,
        )
    inventory = []
    for line in result.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 2:
            fail(f"topology probe emitted malformed line: {line!r}")
        inventory.append((parts[0], parts[1]))
    if not inventory or inventory != sorted(inventory):
        fail("topology probe did not emit a nonempty sorted inventory")
    if len({package for package, _ in inventory}) != len(inventory):
        fail("topology contains duplicate package names")
    if len({directory for _, directory in inventory}) != len(inventory):
        fail("topology contains duplicate manifest directories")
    return inventory


def execution_order(
    inventory: list[tuple[str, str]], package_prefix: str
) -> list[tuple[str, str]]:
    # Built from the same package_prefix string passed to the Rust probe in
    # discover_inventory, not a second hardcoded "wm2000-block" -- see the
    # emitter/parser note there. re.escape guards prefixes containing regex
    # metacharacters (title validation already forbids everything but
    # alphanumerics/-/_, so this is defence in depth, not load-bearing).
    escaped = re.escape(package_prefix)
    boot_pattern = re.compile(rf"{escaped}-shard-(\d+)")
    tail_pattern = re.compile(rf"{escaped}-resident-tail-shard-(\d+)")
    overlay_pattern = re.compile(rf"{escaped}-overlay-(\d+)-shard-(\d+)")

    def key(item: tuple[str, str]) -> tuple[int, int, int]:
        package = item[0]
        if match := boot_pattern.fullmatch(package):
            return (0, 0, int(match.group(1)))
        if match := tail_pattern.fullmatch(package):
            return (1, 0, int(match.group(1)))
        match = overlay_pattern.fullmatch(package)
        if not match:
            fail(f"unexpected generated package name {package}")
        return (2, int(match.group(1)), int(match.group(2)))

    return sorted(inventory, key=key)


def inventory_source(inventory: list[tuple[str, str]], title: str) -> str:
    historical = """//
// The inventory is 32, not 35. Commit 6ae673e (\"bound overlay generations to
// their text extent\") shrank the text-bounded overlays from [3,1,6,8] shards
// to [2,1,5,7], retiring overlay-0-shard-02, overlay-2-shard-05 and
// overlay-3-shard-07.
""" if title == DEFAULT_TITLE and len(inventory) == 32 else """//
// This inventory was derived mechanically from the selected ROM. Re-running
// `scripts/generate-wm-shard-topology.py` is the only supported way to change
// its package count or manifest-directory mapping.
"""
    entries = "".join(f'    ("{package}", "{directory}"),\n' for package, directory in inventory)
    # WM2000's committed header wording predates --title and names the title
    # by its product name, not its directory; preserved verbatim so
    # regenerating with the default title reproduces the committed file
    # byte-for-byte. A real second title gets generic wording instead of a
    # guessed product name.
    inventory_label = "WM2000" if title == DEFAULT_TITLE else title
    boot_label = "WM root pack build" if title == DEFAULT_TITLE else "root pack build"
    return f'''// Single source of truth for the {inventory_label} dense-AOT shard inventory.
//
// This file is `include!`d verbatim by every consumer that must agree on the
// shard catalog, so the list cannot drift between them again:
//
//   examples/{title}/build.rs        (legacy shard generator)
//   examples/{title}/materializer.rs (prepared-tree consumer)
//   examples/{package_prefix_for_title(title)}-boot/build.rs          ({boot_label})
//   crates/fn64-boot-harness/src/generated_runner_build/mod.rs (verifier)
//
// Each entry pairs the Cargo package name with the directory under
// `examples/{title}/` that holds its leaf manifest. The two are
// not mechanically derivable from one another: the resident-tail packages
// live in the historically numbered `shard15`/`shard16` directories.
//
// Entries are sorted by package name. `materialize_package` binary-searches
// this order, and the verifier zips the directory column against the package
// column, so both the sort and the pairing are load-bearing.
{historical}[
{entries}]
'''


def workspace_manifest(inventory: list[tuple[str, str]]) -> str:
    resident_dirs = sorted(
        directory for package, directory in inventory if "overlay" not in package
    )
    rows = []
    offsets = (
        [0, 4, 8, 12]
        if len(resident_dirs) == 17
        else range(0, len(resident_dirs), 4)
    )
    for row_index, offset in enumerate(offsets):
        width = 5 if len(resident_dirs) == 17 and row_index == 3 else 4
        row = ", ".join(f'"{name}"' for name in resident_dirs[offset : offset + width])
        rows.append(f"    {row},")
    members = "\n".join(rows)
    return f'''[workspace]
resolver = "2"
members = [
{members}
    "overlay*-shard*",
]

[profile.dev.build-override]
# Mechanical ROM discovery is CPU-bound and is shared build-time machinery,
# not generated guest code. Leaving host build dependencies at opt-level 0
# makes each overlay shard repeat the same recovery roughly forty times more
# slowly; keep target crates in the ordinary dev profile while optimizing only
# build scripts and their dependencies.
opt-level = 1
'''


def leaf_manifest(package: str, directory: str) -> str:
    compact = directory.startswith("shard") and directory not in {"shard00", "shard16"}
    if compact:
        return f'''[package]
name = "{package}"
version = "0.0.0"
edition = "2021"
publish = false
build = "../build.rs"
[lib]
path = "../lib.rs"
[dependencies]
fn64-recomp-rs = {{ path = "../../../crates/fn64-recomp-rs", default-features = false, features = ["aot-runtime"] }}
[build-dependencies]
fn64-discover = {{ path = "../../../crates/fn64-discover" }}
fn64-recomp-rs-codegen = {{ path = "../../../crates/fn64-recomp-rs-codegen" }}
sha2 = "0.10"
'''
    return f'''[package]
name = "{package}"
version = "0.0.0"
edition = "2021"
publish = false
build = "../build.rs"

[lib]
path = "../lib.rs"

[dependencies]
fn64-recomp-rs = {{ path = "../../../crates/fn64-recomp-rs", default-features = false, features = ["aot-runtime"] }}

[build-dependencies]
fn64-discover = {{ path = "../../../crates/fn64-discover" }}
fn64-recomp-rs-codegen = {{ path = "../../../crates/fn64-recomp-rs-codegen" }}
sha2 = "0.10"
'''


def dense_aot_source(ordered: list[tuple[str, str]]) -> str:
    output = "use crate::*;\n\npub(crate) const DENSE_AOT_ARTIFACTS: &[DenseAotArtifact] = &[\n"
    for package, _ in ordered:
        symbol = package.replace("-", "_")
        output += f'''    DenseAotArtifact {{
        bank_id: {symbol}::BANK_ID,
        code_bank: {symbol}::code_bank,
        runner: {symbol}::run,
    }},
'''
    output += "];\n\npub(crate) const DENSE_AOT_IDENTITIES: &[LinkedDenseIdentity] = &[\n"
    for package, _ in ordered:
        symbol = package.replace("-", "_")
        output += f'''    LinkedDenseIdentity {{
        source_sha256: {symbol}::SOURCE_SHA256,
        runner_source_sha256: {symbol}::RUNNER_SOURCE_SHA256,
    }},
'''
    return output + "];\n"


def replace_boot_dependencies(
    source: str, ordered: list[tuple[str, str]], title: str
) -> str:
    dependencies = "".join(
        f'{package} = {{ path = "../{title}/{directory}" }}\n'
        for package, directory in ordered
    )
    # `source` is always read from SOURCE_BOOT, the committed WM2000
    # Cargo.toml template (see write_topology) -- regardless of which title
    # is being generated. So the pattern that LOCATES the block to replace
    # must match WM2000's own prefix, not this run's target `package_prefix`;
    # only the replacement text (`dependencies`, built from `ordered` and
    # `title` above) uses the target.
    source_prefix = package_prefix_for_title(DEFAULT_TITLE)
    pattern = re.compile(
        rf'^{re.escape(source_prefix)}-(?:shard|resident-tail|overlay).*?\n\n(?=\[build-dependencies\])',
        re.MULTILINE | re.DOTALL,
    )
    replaced, count = pattern.subn(dependencies + "\n", source)
    if count != 1:
        fail("cannot locate the generated dependency block in boot Cargo.toml")
    return replaced


def write_topology(
    output_root: Path, inventory: list[tuple[str, str]], title: str
) -> None:
    package_prefix = package_prefix_for_title(title)
    boot_title = f"{package_prefix}-boot"
    shards = output_root / "examples" / title
    boot = output_root / "examples" / boot_title
    shards.mkdir(parents=True, exist_ok=True)
    (boot / "src").mkdir(parents=True, exist_ok=True)
    ordered = execution_order(inventory, package_prefix)

    old_dirs: set[str] = set()
    old_inventory = shards / "shard_inventory.in"
    if old_inventory.is_file():
        old_dirs = set(re.findall(r'^\s*\("[^"]+", "([^"]+)"\),$', old_inventory.read_text(), re.MULTILINE))
    new_dirs = {directory for _, directory in inventory}
    for directory in sorted(old_dirs - new_dirs):
        target = shards / directory
        if target.is_dir() and (target / "Cargo.toml").is_file():
            shutil.rmtree(target)

    (shards / "Cargo.toml").write_text(workspace_manifest(inventory))
    (shards / "shard_inventory.in").write_text(inventory_source(inventory, title))
    for package, directory in inventory:
        target = shards / directory
        target.mkdir(parents=True, exist_ok=True)
        (target / "Cargo.toml").write_text(leaf_manifest(package, directory))

    # The boot Cargo.toml template always comes from the committed WM2000
    # tree (SOURCE_BOOT) -- it is the shared dependency-block shape, not
    # per-title content. replace_boot_dependencies rewrites it for this
    # title's own prefix and shard directory.
    boot_source = (SOURCE_BOOT / "Cargo.toml").read_text()
    boot_source = boot_source.replace(
        f'name = "{package_prefix_for_title(DEFAULT_TITLE)}-boot"',
        f'name = "{boot_title}"',
    )
    (boot / "Cargo.toml").write_text(
        replace_boot_dependencies(boot_source, ordered, title)
    )
    (boot / "src" / "dense_aot.rs").write_text(dense_aot_source(ordered))


def main() -> None:
    arguments = parse_arguments()
    output_root = arguments.output_root.resolve()
    package_prefix = package_prefix_for_title(arguments.title)
    inventory = discover_inventory(arguments.rom.resolve(), package_prefix)
    write_topology(output_root, inventory, arguments.title)
    print(
        f"generated_packages={len(inventory)} output_root={output_root} "
        f"title={arguments.title}"
    )


if __name__ == "__main__":
    main()
