#!/usr/bin/env python3
"""Fail if generated WM shards regain a codegen dependency at runtime."""

import copy
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SHARDS = ROOT / "examples" / "wm2000-block-shards"
ROOT_MANIFEST = ROOT / "examples" / "wm2000-block-boot" / "Cargo.toml"
INVENTORY = SHARDS / "shard_inventory.in"


def fail(message: str) -> None:
    raise SystemExit(f"WM shard dependency audit: {message}")


def read_source(relative: str) -> str:
    """Read a repository file, failing the audit instead of raising.

    This audit previously read `crates/fn64-boot-harness/src/
    generated_runner_build.rs`, which became a module directory. The
    unhandled FileNotFoundError meant every subsequent check silently stopped
    running -- which is why the 35 -> 32 inventory drift went uncaught. Any
    path that moves must now fail as an audit finding, loudly.
    """
    path = ROOT / relative
    if not path.is_file():
        fail(f"audit input {relative} does not exist; a source path moved")
    return path.read_text()


# The one hand-maintained inventory. Every Rust consumer `include!`s this file,
# so the audit measures it once and holds everything else against it.
if not INVENTORY.is_file():
    fail("shard inventory shard_inventory.in is missing")
inventory_source = INVENTORY.read_text()
inventory = re.findall(r'^\s*\("([^"]+)", "([^"]+)"\),$', inventory_source, re.MULTILINE)
if not inventory:
    fail("cannot parse the shard inventory pair list")
inventory_packages = [package for package, _ in inventory]
inventory_dirs = [directory for _, directory in inventory]
SHARD_COUNT = len(inventory)
if len(set(inventory_packages)) != SHARD_COUNT:
    fail("shard inventory contains duplicate package names")
if len(set(inventory_dirs)) != SHARD_COUNT:
    fail("shard inventory contains duplicate manifest directories")
if inventory_packages != sorted(inventory_packages):
    fail(
        "shard inventory is not sorted by package name; "
        "materialize_package binary-searches this order"
    )

manifests = sorted(
    path / "Cargo.toml"
    for path in SHARDS.iterdir()
    if path.is_dir()
    and path.name != "producer"
    and (path / "Cargo.toml").is_file()
)
if len(manifests) != SHARD_COUNT:
    fail(
        f"shard inventory declares {SHARD_COUNT} packages but "
        f"{len(manifests)} shard manifests exist on disk"
    )
observed_dirs = sorted(manifest.parent.name for manifest in manifests)
if observed_dirs != sorted(inventory_dirs):
    surplus = sorted(set(observed_dirs) - set(inventory_dirs))
    absent = sorted(set(inventory_dirs) - set(observed_dirs))
    fail(
        "shard directories on disk do not match the inventory: "
        f"surplus={surplus} absent={absent}"
    )

package_names = []
package_by_manifest = {}
for manifest in manifests:
    with manifest.open("rb") as source:
        document = tomllib.load(source)
    package = document.get("package", {}).get("name", manifest.parent.name)
    package_names.append(package)
    package_by_manifest[manifest.resolve()] = package
    if document.get("package", {}).get("build") != "../build.rs":
        fail(f"{package} unexpectedly activated the prepared materializer")
    dependencies = document.get("dependencies", {})
    build_dependencies = document.get("build-dependencies", {})
    if set(dependencies) != {"fn64-recomp-rs"}:
        fail(f"{package} runtime dependencies are {sorted(dependencies)}, expected fn64-recomp-rs")
    if "fn64-recomp-rs-codegen" not in build_dependencies:
        fail(f"{package} does not use fn64-recomp-rs-codegen at build time")
    if "fn64-recomp-rs" in build_dependencies:
        fail(f"{package} still depends on fn64-recomp-rs directly at build time")
if len(set(package_names)) != len(package_names):
    fail("shard leaf manifests contain duplicate package names")

with (ROOT / "crates/fn64-recomp-rs/Cargo.toml").open("rb") as source:
    runtime = tomllib.load(source)
if "fn64-recomp-rs-codegen" in runtime.get("dependencies", {}):
    fail("fn64-recomp-rs has a normal dependency on codegen, recreating the invalidation cycle")

with (ROOT / "crates/fn64-recomp-rs-codegen/Cargo.toml").open("rb") as source:
    codegen = tomllib.load(source)
if "fn64-recomp-rs" not in codegen.get("dependencies", {}):
    fail("fn64-recomp-rs-codegen does not depend one-way on fn64-recomp-rs")
codegen_lib_source = (ROOT / "crates/fn64-recomp-rs-codegen/src/lib.rs").read_text()
if "pub use fn64_recomp_rs::{BankId, BankWordKind};" not in codegen_lib_source:
    fail("codegen does not re-export the BankId required by its typed input API")

materializer = SHARDS / "materializer.rs"
prepared_build = SHARDS / "prepared_build.rs"
generator = SHARDS / "build.rs"
prepared_tree = SHARDS / "prepared_tree.rs"
producer = SHARDS / "producer.rs"
producer_manifest = ROOT / "examples/wm2000-prepared-shard-producer/Cargo.toml"
for path in (
    materializer,
    prepared_build,
    generator,
    prepared_tree,
    producer,
    producer_manifest,
):
    if not path.is_file():
        fail(f"missing Stage B foundation {path.relative_to(ROOT)}")

materializer_source = materializer.read_text()
prepared_build_source = prepared_build.read_text()
generator_source = generator.read_text()
prepared_tree_source = prepared_tree.read_text()
producer_source = producer.read_text()
verifier_mod_source = read_source(
    "crates/fn64-boot-harness/src/generated_runner_build/mod.rs"
)
verifier_source = verifier_mod_source + read_source(
    "crates/fn64-boot-harness/src/generated_runner_build/build.rs"
)
wm_root_build_source = read_source("examples/wm2000-block-boot/build.rs")
# Scan the whole WM root source tree, not one named file. Both prior audit
# breakages were a `src/*.rs` file being split into modules while this script
# kept naming the old path: `generated_runner_build.rs` became a directory
# (crashing the audit outright) and the build-identity emitter moved from
# `main.rs` to `runner_reports.rs`.
WM_ROOT_SRC = ROOT / "examples/wm2000-block-boot/src"
if not WM_ROOT_SRC.is_dir():
    fail("WM selected child source tree is missing")
wm_root_sources = sorted(WM_ROOT_SRC.rglob("*.rs"))
if not wm_root_sources:
    fail("WM selected child source tree contains no Rust sources")
wm_root_source = "\n".join(path.read_text() for path in wm_root_sources)

expected_packages = sorted(package_names)
if expected_packages != sorted(inventory_packages):
    fail(
        "leaf manifest package names do not match the shard inventory: "
        f"manifests={expected_packages} inventory={sorted(inventory_packages)}"
    )

# The drift this audit exists to prevent: six hand-maintained copies of one
# list. Each consumer must `include!` the single source, never restate it.
# A literal `[&str; N]` inventory anywhere below is a regression by
# construction, so reject the shape as well as the content.
INVENTORY_CONSUMERS = (
    ("prepared materializer", 'include!("shard_inventory.in")', materializer_source),
    ("shared generator", 'include!("shard_inventory.in")', generator_source),
    (
        "WM root pack build",
        'include!("../wm2000-block-shards/shard_inventory.in")',
        wm_root_build_source,
    ),
    (
        "generated-build verifier",
        'include!("../../../../examples/wm2000-block-shards/shard_inventory.in")',
        verifier_mod_source,
    ),
)
def require_single_inventory_source(
    label: str, expected_include: str, source: str
) -> None:
    if expected_include not in source:
        fail(
            f"{label} does not include! the single shard inventory "
            f"({expected_include}); restating the list is what let it drift"
        )
    restated = re.search(
        r"(?:const|static)\s+\w+\s*:\s*\[&(?:'static )?str;\s*\d+\]\s*=\s*\[\s*\n\s*\"wm2000-block-",
        source,
    )
    if restated is not None:
        fail(
            f"{label} restates a literal shard package list; it must derive "
            "every entry from the included inventory"
        )
    if re.search(r"\[&(?:'static )?str;\s*\d+\]", source):
        fail(
            f"{label} declares a shard array with a hardcoded length; the "
            "count must follow from SHARD_INVENTORY.len()"
        )


for label, expected_include, source in INVENTORY_CONSUMERS:
    require_single_inventory_source(label, expected_include, source)

# Every place that spells the count in prose or in a wire format must derive
# it too. `artifact_count` is a manifest field one side writes and three sides
# parse; a literal there desynchronizes the prepared-tree format silently.
for label, source in (
    ("prepared publisher", prepared_tree_source),
    ("generated-build verifier", verifier_source),
    ("WM root pack build", wm_root_build_source),
    ("harness prepared-tree fixture", read_source(
        "crates/fn64-boot-harness/src/generated_runner_build/tests/mod.rs"
    )),
):
    literal = re.search(r"artifact_count (\d+)", source)
    if literal is not None:
        fail(
            f"{label} hardcodes 'artifact_count {literal.group(1)}'; it must "
            "format the length of the shared inventory"
        )


def root_dependency_inventory(
    document: dict, packages_by_manifest: dict[Path, str]
) -> list[str]:
    dependencies = document.get("dependencies", {})
    if not isinstance(dependencies, dict):
        fail("WM root dependency table is malformed")
    inventory = []
    seen_manifests = set()
    shard_prefixes = (
        "wm2000-block-shard-",
        "wm2000-block-resident-tail-shard-",
        "wm2000-block-overlay-",
    )
    for dependency, specification in dependencies.items():
        if not isinstance(specification, dict) or "path" not in specification:
            continue
        path = specification["path"]
        if not isinstance(path, str):
            fail(f"WM root dependency {dependency} has a non-string path")
        manifest = (ROOT_MANIFEST.parent / path / "Cargo.toml").resolve()
        package = packages_by_manifest.get(manifest)
        if package is None:
            if dependency.startswith(shard_prefixes):
                fail(
                    f"WM root shard dependency {dependency} does not resolve "
                    f"to one of the {SHARD_COUNT} leaf manifests"
                )
            continue
        declared_package = specification.get("package", dependency)
        if declared_package != package:
            fail(
                f"WM root dependency {dependency} resolves to {package}, "
                f"not declared package {declared_package}"
            )
        if dependency != package:
            fail(
                f"WM root shard dependency key {dependency} aliases {package}; "
                "the package identity must remain explicit"
            )
        if manifest in seen_manifests:
            fail(f"WM root depends on shard package {package} more than once")
        seen_manifests.add(manifest)
        inventory.append(package)
    return sorted(inventory)


def require_exact_root_dependency_inventory(document: dict) -> None:
    inventory = root_dependency_inventory(document, package_by_manifest)
    missing = sorted(set(expected_packages) - set(inventory))
    unexpected = sorted(set(inventory) - set(expected_packages))
    if missing or unexpected or len(inventory) != len(expected_packages):
        fail(
            "WM root dependency graph does not exactly match the "
            f"{SHARD_COUNT} shard packages: "
            f"missing={missing} unexpected={unexpected}"
        )


with ROOT_MANIFEST.open("rb") as source:
    root_document = tomllib.load(source)
require_exact_root_dependency_inventory(root_document)


for forbidden in (
    "fn64_discover",
    "fn64_recomp",
    "serde",
    "sha2::",
    "extern crate sha2",
    "Command::new",
):
    if forbidden in materializer_source or forbidden in prepared_build_source:
        fail(f"prepared materializer foundation contains forbidden edge {forbidden!r}")

build_source = (SHARDS / "build.rs").read_text()
if "generate_package(&package)" not in build_source:
    fail("legacy shard build does not consume the shared one-package generator")
if "fn64_recomp_rs::BankId" in build_source:
    fail("legacy shard build bypasses codegen's BankId re-export and needs an undeclared edge")
if "fn64_recomp_rs_codegen::BankId::new(id)" not in build_source:
    fail("legacy shard build does not construct the typed codegen BankId through its owned edge")
if (
    '#[path = "build.rs"]' not in producer_source
    or "for package in generator::PACKAGES" not in producer_source
    or "generate_package(package)" not in producer_source
):
    fail("one-shot producer does not stream the measured shared generator")
if "rename_noreplace(staging, &self.output)" not in prepared_tree_source:
    fail("prepared publisher does not no-replace rename its complete staging tree")
if "prepared output must be outside the repository" not in prepared_tree_source:
    fail("prepared publisher does not reject repository-contained output")
for required in (
    'ROOT_SCHEMA_V2: &str = "fn64.wm-prepared-shard-tree.v2"',
    'ARTIFACT_SCHEMA_V1: &str = "fn64.wm-prepared-shard-artifact.v1"',
    'const IDENTITY_NAME: &str = "identity.v1"',
    'const UPDATE_MARKER_NAME: &str = ".update.v2"',
    '"artifact {} {} {} {}\\n"',
):
    if required not in prepared_tree_source:
        fail(f"prepared publisher is missing v2 cross-binding shape {required!r}")
if "IDENTITY_NAME" not in materializer_source:
    fail("prepared materializer does not consume the selected package sidecar")
if "UPDATE_MARKER_NAME" not in materializer_source or "require_stable_projection" not in materializer_source:
    fail("prepared materializer does not fail closed during stable-root updates")
for forbidden_root_edge in ("MANIFEST_NAME", "manifest.v2", "ROOT_SCHEMA_V2"):
    if forbidden_root_edge in materializer_source:
        fail(
            "prepared materializer watches the root authority manifest, recreating "
            f"global invalidation via {forbidden_root_edge!r}"
        )

with producer_manifest.open("rb") as source:
    producer_document = tomllib.load(source)
producer_dependencies = set(producer_document.get("dependencies", {}))
if producer_dependencies != {
    "fn64-discover",
    "fn64-recomp-rs",
    "fn64-recomp-rs-codegen",
    "sha2",
}:
    fail(f"one-shot producer dependencies are unexpected: {sorted(producer_dependencies)}")

with (ROOT / "crates/fn64-boot-harness/Cargo.toml").open("rb") as source:
    harness_document = tomllib.load(source)
harness_dependencies = harness_document.get("dependencies", {})
for forbidden in ("fn64-discover", "fn64-recomp-rs-codegen"):
    if forbidden in harness_dependencies:
        fail(f"verifier runtime graph directly depends on {forbidden}")
for required in (
    '"fn64.generated-runner-build-identity.v3"',
    '"fn64.verified-generated-runner-build.v5"',
    '"legacy_with_prepared_candidate"',
    '"prepared_consumed"',
    "measure_prepared_tree_v3(",
    "build_prepared_producer_v3(",
    "revalidate_prepared_producer_v3(",
):
    if required not in verifier_source:
        fail(f"generated-build verifier lacks v3 authority edge {required!r}")
if "GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3" not in wm_root_source:
    fail("WM selected child does not emit generated-build identity v3")


def require_external_generation_registration(source: str) -> None:
    """Every admission loop must reserve its generation before installing code.

    The WM root has more than one `for image in pack::EXTERNAL_EXECUTABLE_IMAGES`
    loop -- the admitting one in `block_program.rs` and a diagnostic printing one
    in `main.rs`. The previous check used `rsplit(marker, 1)`, so it inspected
    only the LAST loop; once the sources were split into modules that became the
    printing loop, and the real admission site went unaudited. Identify
    admission loops by the `let code =` install that follows, and require every
    one of them.
    """
    loop_marker = "    for image in pack::EXTERNAL_EXECUTABLE_IMAGES {"
    segments = source.split(loop_marker)[1:]
    if not segments:
        fail("WM selected child lacks the external executable-image admission loop")
    admitting = [segment for segment in segments if "        let code =" in segment]
    if not admitting:
        fail("WM selected child lacks the external executable-image admission loop")
    for segment in admitting:
        loop = segment.split("        let code =", 1)[0]
        if "register_external_executable_generation(" not in loop:
            fail(
                "WM selected child admits an external executable image without "
                "reserving its precompiled generation"
            )


require_external_generation_registration(wm_root_source)


def expect_root_inventory_failure(document: dict, expected_fragment: str) -> None:
    try:
        require_exact_root_dependency_inventory(document)
    except SystemExit as error:
        if expected_fragment not in str(error):
            fail(
                "root dependency negative fixture failed for an unexpected reason: "
                f"{error}"
            )
    else:
        fail("root dependency negative fixture unexpectedly passed")


def selftest() -> None:
    missing_tail = copy.deepcopy(root_document)
    del missing_tail["dependencies"]["wm2000-block-resident-tail-shard-00"]
    expect_root_inventory_failure(
        missing_tail, "wm2000-block-resident-tail-shard-00"
    )

    obsolete_shard = copy.deepcopy(root_document)
    obsolete_shard["dependencies"]["wm2000-block-shard-15"] = {
        "path": "../wm2000-block-shards/shard15",
        "package": "wm2000-block-resident-tail-shard-00",
    }
    expect_root_inventory_failure(obsolete_shard, "aliases")

    # Strip the reservation from EVERY admission site. Replacing only the
    # first occurrence is what made this fixture vacuous once the WM root was
    # split into modules: the first match became an unrelated definition.
    unreserved_external = wm_root_source.replace(
        "register_external_executable_generation(",
        "omit_external_executable_generation(",
    )
    try:
        require_external_generation_registration(unreserved_external)
    except SystemExit as error:
        if "without reserving its precompiled generation" not in str(error):
            fail("external-generation negative fixture failed unexpectedly")
    else:
        fail("unreserved external-generation fixture unexpectedly passed")

    # The drift that motivated this audit's rewrite: an inventory copy that
    # restates the package list, and a stale hardcoded array length. Both must
    # fail, or the six-copy bug can come back exactly as it did before.
    restated = (
        'const PREPARED_PACKAGES: [&str; 35] = [\n'
        '    "wm2000-block-shard-00",\n'
        "];\n"
    )
    try:
        require_single_inventory_source(
            "fixture", 'include!("x")', 'include!("x")\n' + restated
        )
    except SystemExit as error:
        if "restates a literal shard package list" not in str(error):
            fail(f"restated-inventory fixture failed unexpectedly: {error}")
    else:
        fail("restated-inventory fixture unexpectedly passed")

    try:
        require_single_inventory_source(
            "fixture", 'include!("x")', 'include!("x")\nconst A: [&str; 35] = derive();\n'
        )
    except SystemExit as error:
        if "hardcoded length" not in str(error):
            fail(f"hardcoded-length fixture failed unexpectedly: {error}")
    else:
        fail("hardcoded-length fixture unexpectedly passed")

    # A consumer that silently stopped including the shared inventory.
    try:
        require_single_inventory_source("fixture", 'include!("x")', "// nothing\n")
    except SystemExit as error:
        if "does not include! the single shard inventory" not in str(error):
            fail(f"missing-include fixture failed unexpectedly: {error}")
    else:
        fail("missing-include fixture unexpectedly passed")

    # A source path that moved, which is how this audit silently died before.
    try:
        read_source("crates/fn64-boot-harness/src/generated_runner_build.rs")
    except SystemExit as error:
        if "a source path moved" not in str(error):
            fail(f"moved-source fixture failed unexpectedly: {error}")
    else:
        fail("moved-source fixture unexpectedly passed")

    print("WM shard dependency audit selftest: 7/7")


if sys.argv[1:] == ["--selftest"]:
    selftest()
elif sys.argv[1:]:
    print(
        "usage: scripts/lint-wm-shard-dependencies.py [--selftest]",
        file=sys.stderr,
    )
    raise SystemExit(2)
else:
    print(
        "WM shard dependency audit: PASS "
        f"({SHARD_COUNT} exact root/runtime shard edges derived from one "
        "shard_inventory.in; shared producer/materializer foundation present "
        "and inactive)"
    )
