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
SOURCE_SHARDS = ROOT / "examples" / "wm2000-block-shards"
SOURCE_BOOT = ROOT / "examples" / "wm2000-block-boot"


def fail(message: str) -> None:
    raise SystemExit(f"generate WM shard topology: {message}")


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rom", required=True, type=Path)
    parser.add_argument(
        "--output-root",
        required=True,
        type=Path,
        help="repository-shaped output root (may be the fn64 checkout)",
    )
    return parser.parse_args()


def rust_string(value: Path) -> str:
    return json.dumps(os.fspath(value))


def discover_inventory(rom: Path) -> list[tuple[str, str]]:
    if not rom.is_absolute() or not rom.is_file():
        fail("--rom must name an absolute regular file")
    with tempfile.TemporaryDirectory(prefix="fn64-shard-topology.") as temporary:
        package = Path(temporary)
        build_rs = SOURCE_SHARDS / "build.rs"
        main_source = f'''#[allow(dead_code)]
#[path = {rust_string(build_rs)}]
mod generator;

fn main() {{
    let source = std::fs::read(std::env::args_os().nth(1).expect("ROM argument"))
        .expect("reading ROM");
    let mut generator = generator::WmShardGenerator::from_rom_bytes_for_topology(&source);
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


def execution_order(inventory: list[tuple[str, str]]) -> list[tuple[str, str]]:
    def key(item: tuple[str, str]) -> tuple[int, int, int]:
        package = item[0]
        if match := re.fullmatch(r"wm2000-block-shard-(\d+)", package):
            return (0, 0, int(match.group(1)))
        if match := re.fullmatch(r"wm2000-block-resident-tail-shard-(\d+)", package):
            return (1, 0, int(match.group(1)))
        match = re.fullmatch(r"wm2000-block-overlay-(\d+)-shard-(\d+)", package)
        if not match:
            fail(f"unexpected generated package name {package}")
        return (2, int(match.group(1)), int(match.group(2)))

    return sorted(inventory, key=key)


def inventory_source(inventory: list[tuple[str, str]]) -> str:
    historical = """//
// The inventory is 32, not 35. Commit 6ae673e (\"bound overlay generations to
// their text extent\") shrank the text-bounded overlays from [3,1,6,8] shards
// to [2,1,5,7], retiring overlay-0-shard-02, overlay-2-shard-05 and
// overlay-3-shard-07.
""" if len(inventory) == 32 else """//
// This inventory was derived mechanically from the selected ROM. Re-running
// `scripts/generate-wm-shard-topology.py` is the only supported way to change
// its package count or manifest-directory mapping.
"""
    entries = "".join(f'    ("{package}", "{directory}"),\n' for package, directory in inventory)
    return f'''// Single source of truth for the WM2000 dense-AOT shard inventory.
//
// This file is `include!`d verbatim by every consumer that must agree on the
// shard catalog, so the list cannot drift between them again:
//
//   examples/wm2000-block-shards/build.rs        (legacy shard generator)
//   examples/wm2000-block-shards/materializer.rs (prepared-tree consumer)
//   examples/wm2000-block-boot/build.rs          (WM root pack build)
//   crates/fn64-boot-harness/src/generated_runner_build/mod.rs (verifier)
//
// Each entry pairs the Cargo package name with the directory under
// `examples/wm2000-block-shards/` that holds its leaf manifest. The two are
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


def replace_boot_dependencies(source: str, ordered: list[tuple[str, str]]) -> str:
    dependencies = "".join(
        f'{package} = {{ path = "../wm2000-block-shards/{directory}" }}\n'
        for package, directory in ordered
    )
    pattern = re.compile(
        r'^wm2000-block-(?:shard|resident-tail|overlay).*?\n\n(?=\[build-dependencies\])',
        re.MULTILINE | re.DOTALL,
    )
    replaced, count = pattern.subn(dependencies + "\n", source)
    if count != 1:
        fail("cannot locate the generated dependency block in boot Cargo.toml")
    return replaced


def write_topology(output_root: Path, inventory: list[tuple[str, str]]) -> None:
    shards = output_root / "examples" / "wm2000-block-shards"
    boot = output_root / "examples" / "wm2000-block-boot"
    shards.mkdir(parents=True, exist_ok=True)
    (boot / "src").mkdir(parents=True, exist_ok=True)
    ordered = execution_order(inventory)

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
    (shards / "shard_inventory.in").write_text(inventory_source(inventory))
    for package, directory in inventory:
        target = shards / directory
        target.mkdir(parents=True, exist_ok=True)
        (target / "Cargo.toml").write_text(leaf_manifest(package, directory))

    boot_source = (SOURCE_BOOT / "Cargo.toml").read_text()
    (boot / "Cargo.toml").write_text(replace_boot_dependencies(boot_source, ordered))
    (boot / "src" / "dense_aot.rs").write_text(dense_aot_source(ordered))


def main() -> None:
    arguments = parse_arguments()
    output_root = arguments.output_root.resolve()
    inventory = discover_inventory(arguments.rom.resolve())
    write_topology(output_root, inventory)
    print(f"generated_packages={len(inventory)} output_root={output_root}")


if __name__ == "__main__":
    main()
