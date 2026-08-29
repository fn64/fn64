#!/bin/zsh
# Play WM2000 in a WINDOW, with a gamepad, on fn64's all-Rust stack:
# `fn64-cpu-runtime` (FN64_RECOMP=rs) driving `fn64-render-wgpu`
# (FN64_RENDER=wgpu). No N64Recomp C bodies, no RT64 C++ adapter, and
# therefore NO `--features rt64`.
#
#   ./scripts/play-wm2000.sh
#
# That is the whole command. Everything below is defaults you can override:
#
#   FN64_RENDER=reference ./scripts/play-wm2000.sh   # the software oracle
#   FN64_SKIP_EMIT=1      ./scripts/play-wm2000.sh   # reuse the emitted crate
#   FN64_SKIP_SHELL_BUILD=1 FN64_EXPECT_SHELL_SHA256=... \
#                            ./scripts/play-wm2000.sh # reuse one exact shell
#   SCRATCH=/tmp/mine     ./scripts/play-wm2000.sh   # your own scratch root
#
# In the window: F1 settings (incl. gamepad rebinding) - F2 screenshot PNG -
# F3 stack/fps HUD - F11 fullscreen - Esc exit. A gamepad is picked up by
# HOTPLUG, so it can be connected before or after launch; the first pad you
# press a button on becomes the active one. Keyboard works with no pad at all.
#
# ---------------------------------------------------------------------------
# The two traps this script exists to encode
# ---------------------------------------------------------------------------
#
# 1. LEXICAL PATH COLLISION. `recompile_rom` writes an ABSOLUTE
#    fn64-cpu-runtime path into the crate it emits, while the shell's own rs
#    manifest names fn64 by a RELATIVE path. Cargo compares those two strings
#    LEXICALLY, not by realpath, so if they do not resolve to the same STRING
#    the build dies with:
#
#      error: package collision in the lockfile: packages fn64-cpu-runtime
#      v0.0.0 (...) and fn64-cpu-runtime v0.0.0 (...) are different
#
#    A symlink alias of the same directory does NOT fix this -- that was tried
#    and it fails identically. What fixes it is rewriting the emitted
#    manifest's path to the SAME REAL path this repo is checked out at, which
#    is what the bounded manifest rewrite below does.
#
# 2. FN64_ABSENT_N64DD=1 IS NOT OPTIONAL. It is part of osDriveRomInit's
#    disposition; without it the 64DD probe read is a loud trap BY DESIGN.
set -euo pipefail

INVOCATION_DIR=${PWD:A}

# This repo, resolved to a REAL path (see trap 1 -- a symlinked invocation
# would otherwise write a non-matching string into the emitted manifest).
FN64=${FN64:-$(cd -- "$(dirname -- "$0")/.." && pwd -P)}
AKI=${AKI:-$HOME/Code/aki-recomp}
SCRATCH=${SCRATCH:-/private/tmp/fn64-play-scratch}
EMIT=$SCRATCH/emit1
ROM=${ROM:-$AKI/games/NWXE/wm2000.z64}
RECOMP_CONFIG=$AKI/games/NWXE/wm2000.toml
# The rs lane binds host functions BY ADDRESS, so this table is per-title
# game-profile data. There is deliberately no default in build.rs: another
# title's table would resolve silently and produce wrong behaviour.
HOST_LOOKUP=${RECOMP_RS_HOST_LOOKUP:-$HOME/Code/recomps/wm2000/packages/wm2000-boot/src/host_lookup.rs}
RENDER=${FN64_RENDER:-wgpu}
APP_TITLE=${FN64_APP_TITLE:-WrestleMania 2000 [built with fn64]}

# Cargo accepts an ambient target override, so the binary selected for launch
# must be derived from the same resolved directory passed to the build. A
# fixed repo-local path can otherwise survive as an executable and be launched
# immediately after Cargo successfully built the requested revision elsewhere.
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  if [[ "$CARGO_TARGET_DIR" == /* ]]; then
    CARGO_TARGET_ROOT=${CARGO_TARGET_DIR:A}
  else
    CARGO_TARGET_ROOT=${INVOCATION_DIR}/${CARGO_TARGET_DIR}
    CARGO_TARGET_ROOT=${CARGO_TARGET_ROOT:A}
  fi
  RECOMPILE_TARGET_DIR=$CARGO_TARGET_ROOT
  SHELL_TARGET_DIR=$CARGO_TARGET_ROOT
else
  RECOMPILE_TARGET_DIR=$FN64/target
  SHELL_TARGET_DIR=$FN64/crates/fn64-shell/rs/target
fi
BIN=$RECOMPILE_TARGET_DIR/release/recompile_rom
SHELL_BIN=$SHELL_TARGET_DIR/release/fn64
EMIT_RECEIPT=$EMIT/.fn64-private-emit-receipt.v1.json

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

verify_reused_shell() {
  [[ -f "$SHELL_BIN" && ! -L "$SHELL_BIN" && -x "$SHELL_BIN" ]] || {
    echo "[play-wm2000] FATAL: reusable shell is not a regular, non-symlink executable: $SHELL_BIN" >&2
    return 1
  }
  [[ -n "${FN64_EXPECT_SHELL_SHA256:-}" ]] || {
    echo "[play-wm2000] FATAL: FN64_SKIP_SHELL_BUILD=1 requires FN64_EXPECT_SHELL_SHA256" >&2
    return 1
  }
  [[ "$FN64_EXPECT_SHELL_SHA256" != *[^0-9a-f]* && ${#FN64_EXPECT_SHELL_SHA256} -eq 64 ]] || {
    echo "[play-wm2000] FATAL: FN64_EXPECT_SHELL_SHA256 must be exactly 64 lowercase hexadecimal characters" >&2
    return 1
  }
  local actual
  actual=$(sha256_file "$SHELL_BIN")
  [[ "$actual" == "$FN64_EXPECT_SHELL_SHA256" ]] || {
    echo "[play-wm2000] FATAL: reused shell digest mismatch: expected $FN64_EXPECT_SHELL_SHA256, measured $actual" >&2
    return 1
  }
  REUSED_SHELL_SHA256=$actual
}

rewrite_emit_manifest() {
  python3 - "$EMIT/Cargo.toml" "$FN64/crates/fn64-cpu-runtime" <<'PY'
import json
import os
from pathlib import Path
import stat
import sys

manifest = Path(sys.argv[1])
runtime = sys.argv[2]
try:
    metadata = os.lstat(manifest)
except OSError as error:
    raise SystemExit(f"[play-wm2000] FATAL: inspect emitted manifest {manifest}: {error}")
if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
    raise SystemExit(
        f"[play-wm2000] FATAL: emitted manifest must be a regular, non-symlink file: {manifest}"
    )
try:
    lines = manifest.read_text(encoding="utf-8").splitlines(keepends=True)
except OSError as error:
    raise SystemExit(f"[play-wm2000] FATAL: read emitted manifest {manifest}: {error}")
matches = [index for index, line in enumerate(lines) if line.startswith("fn64-cpu-runtime = ")]
if len(matches) != 1:
    raise SystemExit(
        f"[play-wm2000] FATAL: emitted manifest must contain exactly one "
        f"fn64-cpu-runtime dependency, found {len(matches)}"
    )
index = matches[0]
newline = "\r\n" if lines[index].endswith("\r\n") else "\n"
lines[index] = f"fn64-cpu-runtime = {{ path = {json.dumps(runtime)} }}{newline}"
try:
    manifest.write_text("".join(lines), encoding="utf-8")
except OSError as error:
    raise SystemExit(f"[play-wm2000] FATAL: rewrite emitted manifest {manifest}: {error}")
PY
}

measure_emit_receipt() {
  local mode=$1
  python3 - "$mode" "$FN64" "$RECOMP_CONFIG" "$ROM" "$BIN" "$EMIT" "$EMIT_RECEIPT" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile

SCHEMA = "fn64.private-recompile-emit-receipt.v1"
DOMAIN = b"fn64.private-recompile-emit-receipt.v1\0"
TREE_DOMAIN = b"fn64.generated-tree.v1\0"
WORKTREE_DOMAIN = b"fn64.source-worktree.v1\0"

mode, root_arg, config_arg, rom_arg, driver_arg, emit_arg, receipt_arg = sys.argv[1:]
root = Path(root_arg).resolve()
config = Path(config_arg)
rom = Path(rom_arg)
driver = Path(driver_arg)
emit = Path(emit_arg)
receipt = Path(receipt_arg)


def fatal(message: str) -> "None":
    raise SystemExit(f"[play-wm2000] FATAL: {message}")


def push(hasher: "hashlib._Hash", value: bytes) -> None:
    hasher.update(len(value).to_bytes(8, "big"))
    hasher.update(value)


def stable_file(path: Path | bytes, label: str) -> tuple[int, str]:
    try:
        before = os.lstat(path)
    except OSError as error:
        fatal(f"measure {label}: {error}")
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        fatal(f"{label} must be a regular, non-symlink file")
    digest = hashlib.sha256()
    try:
        with open(path, "rb", buffering=1024 * 1024) as source:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
        after = os.lstat(path)
    except OSError as error:
        fatal(f"measure {label}: {error}")
    identity = lambda value: (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
    )
    if identity(before) != identity(after):
        fatal(f"{label} changed while it was being measured")
    return before.st_size, digest.hexdigest()


def file_identity(path: Path, label: str) -> dict[str, object]:
    size, digest = stable_file(path, label)
    return {"bytes": size, "sha256": digest}


def git_bytes(*args: str) -> bytes:
    try:
        result = subprocess.run(
            ["git", "-C", os.fspath(root), *args],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        fatal(f"measure fn64 worktree with git {' '.join(args)}: {error}")
    return result.stdout


def source_identity() -> dict[str, object]:
    head = git_bytes("rev-parse", "HEAD").decode("ascii").strip()
    if len(head) != 40 or any(character not in "0123456789abcdef" for character in head):
        fatal("fn64 HEAD is not one canonical lowercase 40-hex commit")
    listing = git_bytes("ls-files", "-z", "--cached", "--others", "--exclude-standard")
    paths = sorted(path for path in listing.split(b"\0") if path)
    if len(paths) != len(set(paths)):
        fatal("fn64 tracked/untracked source listing contains duplicate paths")
    digest = hashlib.sha256(WORKTREE_DOMAIN)
    push(digest, head.encode("ascii"))
    root_bytes = os.fsencode(root)
    for relative in paths:
        if relative.startswith(b"/") or b"\0" in relative or b".." in relative.split(b"/"):
            fatal("fn64 source listing contains a non-canonical path")
        path = os.path.join(root_bytes, relative)
        push(digest, relative)
        try:
            metadata = os.lstat(path)
        except FileNotFoundError:
            push(digest, b"missing")
            continue
        except OSError as error:
            fatal(f"measure fn64 source path: {error}")
        if stat.S_ISREG(metadata.st_mode):
            size, content = stable_file(path, "fn64 source file")
            push(digest, b"file")
            push(digest, b"1" if metadata.st_mode & stat.S_IXUSR else b"0")
            push(digest, size.to_bytes(8, "big"))
            push(digest, content.encode("ascii"))
        elif stat.S_ISLNK(metadata.st_mode):
            push(digest, b"symlink")
            try:
                push(digest, os.readlink(path))
            except OSError as error:
                fatal(f"measure fn64 source symlink: {error}")
        else:
            fatal("fn64 source identity encountered an unsupported file type")
    if listing != git_bytes("ls-files", "-z", "--cached", "--others", "--exclude-standard"):
        fatal("fn64 source listing changed while it was being measured")
    if head != git_bytes("rev-parse", "HEAD").decode("ascii").strip():
        fatal("fn64 HEAD changed while its source identity was being measured")
    status_bytes = git_bytes("status", "--porcelain=v1", "-z", "--untracked-files=all")
    return {
        "head": head,
        "root": os.fspath(root),
        "state": "dirty" if status_bytes else "clean",
        "status_sha256": hashlib.sha256(status_bytes).hexdigest(),
        "tree_sha256": digest.hexdigest(),
    }


def generated_tree_identity() -> dict[str, object]:
    try:
        root_metadata = os.lstat(emit)
    except OSError as error:
        fatal(f"measure emitted tree: {error}")
    if stat.S_ISLNK(root_metadata.st_mode) or not stat.S_ISDIR(root_metadata.st_mode):
        fatal("emitted tree root must be a regular, non-symlink directory")
    digest = hashlib.sha256(TREE_DOMAIN)
    files = 0
    total_bytes = 0
    emit_bytes = os.fsencode(emit)
    receipt_bytes = os.fsencode(receipt)
    for directory, names, filenames in os.walk(emit_bytes, topdown=True, followlinks=False):
        names.sort()
        filenames.sort()
        for name in names:
            path = os.path.join(directory, name)
            metadata = os.lstat(path)
            if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
                fatal("emitted tree contains a symlink or unsupported directory entry")
            relative = os.path.relpath(path, emit_bytes)
            push(digest, b"dir")
            push(digest, relative)
        for name in filenames:
            path = os.path.join(directory, name)
            if path == receipt_bytes:
                continue
            relative = os.path.relpath(path, emit_bytes)
            size, content = stable_file(path, "generated file")
            push(digest, b"file")
            push(digest, relative)
            push(digest, size.to_bytes(8, "big"))
            push(digest, content.encode("ascii"))
            files += 1
            total_bytes += size
    return {"bytes": total_bytes, "files": files, "sha256": digest.hexdigest()}


def measured() -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "config": file_identity(config, "recompile config"),
        "rom": file_identity(rom, "ROM input"),
        "recompile_rom": file_identity(driver, "recompile_rom"),
        "source": source_identity(),
        "generated_tree": generated_tree_identity(),
    }


def canonical(value: dict[str, object]) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode("utf-8")


def validate_digest(value: object, label: str) -> None:
    if not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
        fatal(f"emit receipt {label} is not one lowercase SHA-256")


if mode == "write":
    body = measured()
    body["receipt_sha256"] = hashlib.sha256(DOMAIN + canonical(body)).hexdigest()
    encoded = json.dumps(body, ensure_ascii=False, indent=2, sort_keys=True).encode("utf-8") + b"\n"
    emit.mkdir(parents=True, exist_ok=True)
    try:
        descriptor, temporary = tempfile.mkstemp(prefix=receipt.name + ".tmp-", dir=emit)
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(encoded)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, receipt)
    except OSError as error:
        fatal(f"publish private emit receipt: {error}")
    print(f"[play-wm2000] private emit receipt: {body['receipt_sha256']}")
elif mode == "verify":
    try:
        receipt_metadata = os.lstat(receipt)
    except OSError as error:
        fatal(f"measure private emit receipt: {error}")
    if receipt_metadata.st_size > 64 * 1024:
        fatal("private emit receipt exceeds the 64-KiB bound")
    stable_file(receipt, "private emit receipt")
    try:
        retained = json.loads(receipt.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fatal(f"parse private emit receipt: {error}")
    if not isinstance(retained, dict) or set(retained) != {
        "schema", "config", "rom", "recompile_rom", "source", "generated_tree", "receipt_sha256"
    }:
        fatal("private emit receipt has missing or unknown fields")
    if retained.get("schema") != SCHEMA:
        fatal("private emit receipt schema is invalid")
    validate_digest(retained.get("receipt_sha256"), "receipt_sha256")
    for field in ("config", "rom", "recompile_rom", "generated_tree"):
        value = retained.get(field)
        if not isinstance(value, dict):
            fatal(f"private emit receipt {field} is malformed")
        validate_digest(value.get("sha256"), f"{field}.sha256")
    source = retained.get("source")
    if not isinstance(source, dict):
        fatal("private emit receipt source is malformed")
    for field in ("status_sha256", "tree_sha256"):
        validate_digest(source.get(field), f"source.{field}")
    body = dict(retained)
    claimed_receipt = body.pop("receipt_sha256")
    recomputed_receipt = hashlib.sha256(DOMAIN + canonical(body)).hexdigest()
    if claimed_receipt != recomputed_receipt:
        fatal("private emit receipt self-digest mismatch")
    current = measured()
    if retained["config"] != current["config"]:
        fatal("private emit receipt recompile config mismatch")
    if retained["rom"] != current["rom"]:
        fatal("private emit receipt ROM input mismatch")
    if retained["recompile_rom"] != current["recompile_rom"]:
        fatal("private emit receipt recompile_rom mismatch")
    if retained["source"] != current["source"]:
        fatal("private emit receipt fn64 source/worktree mismatch")
    if retained["generated_tree"] != current["generated_tree"]:
        fatal("private emit receipt generated-tree mismatch")
    print(f"[play-wm2000] private emit receipt verified: {claimed_receipt}")
else:
    fatal(f"unknown emit-receipt mode {mode!r}")
PY
}

if [[ "${1:-}" == --print-artifact-paths ]]; then
  (( $# == 1 )) || { echo "[play-wm2000] FATAL: --print-artifact-paths accepts no other arguments" >&2; exit 2; }
  printf 'recompile_rom=%s\nshell=%s\n' "$BIN" "$SHELL_BIN"
  exit 0
fi

# Content-free preflight for the regression suite and for operators deciding
# whether an existing shell is safe to reuse. It exercises the production
# verifier but deliberately stops before ROM or host-profile intake.
if [[ "${1:-}" == --check-shell-reuse ]]; then
  (( $# == 1 )) || { echo "[play-wm2000] FATAL: --check-shell-reuse accepts no other arguments" >&2; exit 2; }
  [[ -n "${FN64_SKIP_SHELL_BUILD:-}" ]] || {
    echo "[play-wm2000] FATAL: --check-shell-reuse requires FN64_SKIP_SHELL_BUILD=1" >&2
    exit 2
  }
  verify_reused_shell
  echo "[play-wm2000] reusable shell verified: $SHELL_BIN (sha256=$REUSED_SHELL_SHA256)"
  exit 0
fi

if [[ "${1:-}" == --record-emit-receipt || "${1:-}" == --check-emit-reuse ]]; then
  (( $# == 1 )) || { echo "[play-wm2000] FATAL: ${1:-} accepts no other arguments" >&2; exit 2; }
  rewrite_emit_manifest
  if [[ "$1" == --record-emit-receipt ]]; then
    measure_emit_receipt write
  else
    [[ -n "${FN64_SKIP_EMIT:-}" ]] || {
      echo "[play-wm2000] FATAL: --check-emit-reuse requires FN64_SKIP_EMIT=1" >&2
      exit 2
    }
    measure_emit_receipt verify
  fi
  exit 0
fi

for f in "$ROM" "$HOST_LOOKUP"; do
  [[ -f "$f" ]] || { echo "[play-wm2000] FATAL: missing $f" >&2; exit 1; }
done

# 1. Emit the whole-ROM Rust crate. Guarded by the same staleness rule
#    run-rs-lane.sh uses: a `recompile_rom` older than the codegen sources it
#    was built from silently emits a crate WITHOUT the fix under test, and the
#    run then "reproduces" a blocker that is already fixed. That cost two wrong
#    conclusions in one day.
if [[ -z "${FN64_SKIP_EMIT:-}" ]]; then
  echo "[play-wm2000] building recompile_rom (FN64_SKIP_EMIT=1 to reuse an existing emit)"
  ( cd "$FN64" && CARGO_TARGET_DIR="$RECOMPILE_TARGET_DIR" \
      cargo build --release --bin recompile_rom --offline )
  NEWER=$(find "$FN64/crates/fn64-cpu-runtime-codegen/src" "$FN64/crates/fn64-cpu-runtime/src" \
            -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null || true)
  if [[ -n "$NEWER" ]]; then
    echo "[play-wm2000] FATAL: recompile_rom is STALE -- $NEWER is newer than the binary." >&2
    exit 1
  fi
  mkdir -p "$EMIT"
  "$BIN" --config "$RECOMP_CONFIG" --rom "$ROM" --out "$EMIT"
else
  [[ -d "$EMIT" ]] || { echo "[play-wm2000] FATAL: FN64_SKIP_EMIT=1 but $EMIT does not exist" >&2; exit 1; }
fi

# 2. Defuse trap 1: point the emitted crate at THIS checkout's real path, and
#    bridge the emitted crate in via the symlink Cargo `path` cannot express
#    with an env var.
rewrite_emit_manifest
if [[ -z "${FN64_SKIP_EMIT:-}" ]]; then
  measure_emit_receipt write
else
  measure_emit_receipt verify
  echo "[play-wm2000] reusing receipt-bound emitted crate at $EMIT"
fi
ln -sfn "$EMIT" "$FN64/crates/fn64-shell/rs/recompiled"

# 3. Build the windowed shell on the rs lane. No `--features rt64`.
#    FN64_RENDER=wgpu needs no Cargo feature: WgpuBackend::try_new is
#    unconditionally available.
cd "$FN64/crates/fn64-shell/rs"
if [[ -z "${FN64_SKIP_SHELL_BUILD:-}" ]]; then
  echo "[play-wm2000] building the shell (rs lane, renderer=$RENDER)"
  CARGO_TARGET_DIR="$SHELL_TARGET_DIR" \
  FN64_RECOMP=rs \
  FN64_APP_TITLE="$APP_TITLE" \
  ROM="$ROM" \
  RECOMP_RS_HOST_LOOKUP="$HOST_LOOKUP" \
    cargo build --release --offline
else
  verify_reused_shell
  echo "[play-wm2000] reusing linked shell at $SHELL_BIN (sha256=$REUSED_SHELL_SHA256)"
fi

[[ -f "$SHELL_BIN" && ! -L "$SHELL_BIN" && -x "$SHELL_BIN" ]] || {
  echo "[play-wm2000] FATAL: selected shell is not a regular, non-symlink executable: $SHELL_BIN" >&2
  exit 1
}
SHELL_SHA256=$(sha256_file "$SHELL_BIN")
echo "[play-wm2000] selected shell: $SHELL_BIN (sha256=$SHELL_SHA256)"

# 4. Play. The startup banner names the lane and the RESOLVED renderer -- if
#    it says `reference-fallback`, wgpu failed to construct and the reason is
#    on the line above it. Paste that [fn64-stack] block into any report.
FINAL_SHELL_SHA256=$(sha256_file "$SHELL_BIN")
[[ "$FINAL_SHELL_SHA256" == "$SHELL_SHA256" ]] || {
  echo "[play-wm2000] FATAL: selected shell changed before launch: expected $SHELL_SHA256, measured $FINAL_SHELL_SHA256" >&2
  exit 1
}
exec env \
  ROM="$ROM" \
  FN64_ABSENT_N64DD=1 \
  FN64_RENDER="$RENDER" \
  "$SHELL_BIN" "$@"
