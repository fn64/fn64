#!/usr/bin/env python3
"""Assert the rs-lane shell manifest mirrors the workspace shell manifest.

`crates/fn64-shell/rs/Cargo.toml` is a standalone workspace (game-derived
crates must never enter the main graph -- docs/DESIGN.md section 1) that
compiles the SAME `crates/fn64-shell/src/*.rs` sources as
`crates/fn64-shell/Cargo.toml`. So a dependency added to the workspace
manifest alone is invisible to `cargo test`, `cargo clippy` and CI: nothing in
the main workspace builds the rs manifest. It surfaces only when someone next
runs `scripts/play-wm2000.sh`, as a wall of `E0432`/`E0433`.

That is not hypothetical. The thiserror conversion (PR #174) added `thiserror`
and `serde_json` to the workspace manifest only; the WM2000 play lane could not
build at all -- 63 errors -- until they were mirrored across.

THE MIRROR RULE (docs/DESIGN.md, "standalone manifests carrying their own
`[workspace]`"):

  1. Every `[dependencies]` key of the workspace manifest MUST appear in the
     rs manifest. The workspace set is a SUBSET; the rs manifest is a strict
     superset, adding rs-lane-only entries (`fn64-cpu-runtime`, `libc`,
     `game-recompiled`). Extra rs entries are ALLOWED and not reported.
     This holds for EVERY dependency table, not only `[dependencies]`: each
     `[target.'cfg(...)'.dependencies]` table is compared against the rs
     manifest's table for the SAME cfg key. A crate added only to a
     target-specific table breaks the rs lane identically, and pooling the
     cfgs would let a macOS-only entry be "satisfied" by a Windows-only one.
     (`[build-dependencies]` and `[dev-dependencies]` are deliberately out of
     scope: the rs manifest builds no tests, and both `cc = "1"` build tables
     already match.)
  2. For a shared entry, the VERSION REQUIREMENT must be equal. A workspace
     manifest entry written `foo.workspace = true` is resolved through the
     root `[workspace.dependencies]` first, because the rs manifest has no
     `[workspace.dependencies]` to inherit from and must spell the pin out.
  3. `path` dependencies are compared by their basename only: the two
     manifests sit at different depths, so `../fn64-abi` and `../../fn64-abi`
     are the SAME crate. Differing `features`/`default-features` on a shared
     path entry are allowed -- the rs lane legitimately adds `recomp-rs`.

Run: python3 scripts/lint-rs-lane-manifest.py
Self-test: python3 scripts/lint-rs-lane-manifest.py --self-test
Exit 0 clean, 1 on a mirror violation, 2 on unusable input.
"""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKSPACE_MANIFEST = ROOT / "crates/fn64-shell/Cargo.toml"
RS_MANIFEST = ROOT / "crates/fn64-shell/rs/Cargo.toml"
ROOT_MANIFEST = ROOT / "Cargo.toml"


def version_req(spec, workspace_deps: dict):
    """The comparable version requirement of one dependency spec.

    Returns ("path", basename) for a path dep, ("version", req) for a
    registry dep, or ("unknown", None) when neither is expressible.
    """
    if isinstance(spec, str):
        return ("version", spec)
    if not isinstance(spec, dict):
        return ("unknown", None)
    if spec.get("workspace") is True:
        # Only `resolve()` knows the dependency's NAME, which is what the root
        # [workspace.dependencies] table is keyed by, so inheritance is
        # resolved there and never reaches this function.
        return ("unknown", None)
    if "path" in spec:
        return ("path", Path(spec["path"]).name)
    if "version" in spec:
        return ("version", spec["version"])
    return ("unknown", None)


def resolve(name: str, spec, workspace_deps: dict):
    """Resolve a dependency spec, following `workspace = true` to the root."""
    if isinstance(spec, dict) and spec.get("workspace") is True:
        inherited = workspace_deps.get(name)
        if inherited is None:
            return ("unknown", None)
        return version_req(inherited, {})
    return version_req(spec, {})


def show(p: Path) -> str:
    """Repo-relative path for a message, falling back to the full path.

    The self-test drives `check()` on synthetic manifests in a temp dir, which
    are not under ROOT, so this must not assume containment.
    """
    try:
        return str(p.relative_to(ROOT))
    except ValueError:
        return str(p)


def check(workspace_manifest: Path, rs_manifest: Path, root_manifest: Path):
    """Return a list of human-readable mirror violations."""
    for p in (workspace_manifest, rs_manifest, root_manifest):
        if not p.is_file():
            raise FileNotFoundError(p)

    ws = tomllib.loads(workspace_manifest.read_text())
    rs = tomllib.loads(rs_manifest.read_text())
    root = tomllib.loads(root_manifest.read_text())
    workspace_deps = root.get("workspace", {}).get("dependencies", {})

    problems = []

    # Every dependency table, not just [dependencies]. A crate added only to a
    # `[target.'cfg(...)'.dependencies]` table breaks the rs lane in exactly the
    # same way -- the shell's src/*.rs is platform-conditional whichever table
    # its crates come from -- and comparing per-target keeps a macOS-only entry
    # from being "satisfied" by a Windows-only one.
    tables = [("dependencies", ws.get("dependencies", {}), rs.get("dependencies", {}))]
    ws_targets = ws.get("target", {})
    rs_targets = rs.get("target", {})
    for cfg in sorted(ws_targets):
        tables.append(
            (
                f"target.'{cfg}'.dependencies",
                ws_targets[cfg].get("dependencies", {}),
                rs_targets.get(cfg, {}).get("dependencies", {}),
            )
        )

    for table, ws_deps, rs_deps in tables:
        for name in sorted(ws_deps):
            if name not in rs_deps:
                problems.append(
                    f"{show(rs_manifest)}: missing [{table}] entry "
                    f"`{name}`, which {show(workspace_manifest)} has. The rs "
                    f"lane compiles the same sources, so this is a build failure in "
                    f"scripts/play-wm2000.sh and nowhere else."
                )
                continue
            want = resolve(name, ws_deps[name], workspace_deps)
            got = resolve(name, rs_deps[name], workspace_deps)
            if want[0] == "unknown" or got[0] == "unknown":
                continue
            if want != got:
                problems.append(
                    f"{show(rs_manifest)}: [{table}] `{name}` is {got[1]!r} but "
                    f"{show(workspace_manifest)} pins {want[1]!r}. The two "
                    f"manifests compile one source tree and must resolve one version."
                )
    return problems


def self_test() -> int:
    """Prove the lint FAILS on a manifest that violates the rule.

    A lint that cannot fail is theatre, so each case is checked to produce the
    expected verdict on a synthetic pair written to a temp dir.
    """
    global WORKSPACE_MANIFEST, RS_MANIFEST, ROOT_MANIFEST
    import tempfile

    root_toml = '[workspace.dependencies]\nthiserror = "2"\n'
    cases = [
        (
            "mirrored (superset rs) -> clean",
            '[dependencies]\nserde = "1"\nthiserror.workspace = true\n',
            '[dependencies]\nserde = "1"\nthiserror = "2"\nlibc = "0.2"\n',
            0,
        ),
        (
            "rs missing a workspace dep -> 1 problem",
            '[dependencies]\nserde = "1"\nserde_json = "1"\n',
            '[dependencies]\nserde = "1"\n',
            1,
        ),
        (
            "rs missing the real-world pair -> 2 problems",
            '[dependencies]\nthiserror.workspace = true\nserde_json = "1"\n',
            '[dependencies]\nserde = "1"\n',
            2,
        ),
        (
            "version drift on a shared pin -> 1 problem",
            '[dependencies]\nserde = "1"\n',
            '[dependencies]\nserde = "2"\n',
            1,
        ),
        (
            "workspace-inherited pin drift -> 1 problem",
            '[dependencies]\nthiserror.workspace = true\n',
            '[dependencies]\nthiserror = "1"\n',
            1,
        ),
        (
            "path deps at different depths -> clean",
            '[dependencies]\nfn64-abi = { path = "../fn64-abi" }\n',
            '[dependencies]\nfn64-abi = { path = "../../fn64-abi", features = ["recomp-rs"] }\n',
            0,
        ),
        (
            "mirrored target table -> clean",
            '[target.\'cfg(target_os = "macos")\'.dependencies]\nobjc2 = "0.5.2"\n',
            '[target.\'cfg(target_os = "macos")\'.dependencies]\nobjc2 = "0.5.2"\n',
            0,
        ),
        (
            "dep only in a main TARGET table -> 1 problem",
            '[target.\'cfg(target_os = "macos")\'.dependencies]\n'
            'objc2 = "0.5.2"\nobjc2-app-kit = "0.2.2"\n',
            '[target.\'cfg(target_os = "macos")\'.dependencies]\nobjc2 = "0.5.2"\n',
            1,
        ),
        (
            "whole target table missing from rs -> 1 problem",
            '[target.\'cfg(windows)\'.dependencies]\nwindows-sys = "0.61"\n',
            '[dependencies]\nserde = "1"\n',
            1,
        ),
        (
            "target tables are compared PER cfg, not pooled -> 1 problem",
            '[target.\'cfg(windows)\'.dependencies]\nwindows-sys = "0.61"\n',
            '[target.\'cfg(target_os = "macos")\'.dependencies]\nwindows-sys = "0.61"\n',
            1,
        ),
        (
            "version drift inside a target table -> 1 problem",
            '[target.\'cfg(target_os = "macos")\'.dependencies]\nobjc2 = "0.5.2"\n',
            '[target.\'cfg(target_os = "macos")\'.dependencies]\nobjc2 = "0.6"\n',
            1,
        ),
    ]

    failures = 0
    with tempfile.TemporaryDirectory() as td:
        d = Path(td)
        (d / "root.toml").write_text(root_toml)
        for label, ws_body, rs_body, expected in cases:
            (d / "ws.toml").write_text(ws_body)
            (d / "rs.toml").write_text(rs_body)
            got = check(d / "ws.toml", d / "rs.toml", d / "root.toml")
            ok = len(got) == expected
            print(f"  {'ok  ' if ok else 'FAIL'} {label}: {len(got)} problem(s), expected {expected}")
            if not ok:
                failures += 1
                for g in got:
                    print(f"         {g}")

    # The lint must refuse unusable input rather than passing vacuously.
    try:
        check(Path("/nonexistent/ws.toml"), RS_MANIFEST, ROOT_MANIFEST)
        print("  FAIL missing manifest did not raise")
        failures += 1
    except FileNotFoundError:
        print("  ok   missing manifest raises rather than passing vacuously")

    # Malformed TOML must also raise (main() turns it into a clean FATAL/2
    # rather than a traceback).
    with tempfile.TemporaryDirectory() as td:
        d = Path(td)
        (d / "root.toml").write_text(root_toml)
        (d / "rs.toml").write_text('[dependencies]\nserde = "1"\n')
        (d / "ws.toml").write_text("not valid toml [[[\n")
        try:
            check(d / "ws.toml", d / "rs.toml", d / "root.toml")
            print("  FAIL malformed TOML did not raise")
            failures += 1
        except tomllib.TOMLDecodeError:
            print("  ok   malformed TOML raises rather than passing vacuously")

        # ...and main()'s handler turns that into exit 2, not a traceback.
        saved = (WORKSPACE_MANIFEST, RS_MANIFEST, ROOT_MANIFEST)
        try:
            WORKSPACE_MANIFEST = d / "ws.toml"
            RS_MANIFEST = d / "rs.toml"
            ROOT_MANIFEST = d / "root.toml"
            code = main([])
        finally:
            WORKSPACE_MANIFEST, RS_MANIFEST, ROOT_MANIFEST = saved
        if code == 2:
            print("  ok   main() reports malformed TOML as a clean FATAL (exit 2)")
        else:
            print(f"  FAIL main() returned {code} for malformed TOML, expected 2")
            failures += 1

    print(f"self-test: {failures} failure(s)")
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    try:
        problems = check(WORKSPACE_MANIFEST, RS_MANIFEST, ROOT_MANIFEST)
    except FileNotFoundError as e:
        print(f"lint-rs-lane-manifest: FATAL: missing manifest {e}", file=sys.stderr)
        return 2
    except tomllib.TOMLDecodeError as e:
        # Unusable input must fail the same clean way a missing file does,
        # rather than as a raw traceback.
        print(f"lint-rs-lane-manifest: FATAL: unparseable manifest: {e}", file=sys.stderr)
        return 2
    for p in problems:
        print(f"  {p}")
    if problems:
        print(f"\nlint-rs-lane-manifest: {len(problems)} error(s)", file=sys.stderr)
        return 1
    print("lint-rs-lane-manifest: rs-lane manifest mirrors the shell manifest")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
