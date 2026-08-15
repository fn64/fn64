#!/usr/bin/env python3
"""Gate the committed reference frames against the digests the docs cite.

A frame digest in a doc claims a reproducible fact -- "task #654 renders
byte-identically" -- and lint-docs.py refuses a hash no test owns, because a
hash verified once by hand and never again cannot fail and so cannot warn.

The frames are in the repository, so the claim is checkable here and now: hash
the file, compare against what the docs assert. If a frame is ever recompressed
or re-rendered, this fails and names the file rather than letting the docs go
quietly stale.

ponytail: a dict literal, not a manifest format. Add a row when a frame earns a
cited digest; reach for a parsed manifest only if these outgrow one screen.
"""
import hashlib
import os
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# path -> the digest the docs cite for it.
# reference/revenge-frames/README.md:38 and docs/plans/HANDOFF-2026-08-09.md:148
# both cite this one for Revenge task #654.
FRAMES = {
    "reference/revenge-frames/first-boot-arena-1800collect.png":
        "9794211091c53fb7dd73e52501f959843cf943566e13eac8cc637893f1731ec1",
}


def game_repo_path(rel: str) -> pathlib.Path | None:
    """Where a moved reference frame lives now, if a game checkout is present.

    The frames were extracted to their own repository with the rest of the game
    content, so `reference/revenge-frames/x.png` is now
    `<game-repo>/reference/revenge-frames/x.png`. FN64_SHARD_ROOT points at that
    repo's `packages/` directory, so its parent is the repo root.
    """
    shard_root = os.environ.get("FN64_SHARD_ROOT")
    if not shard_root:
        return None
    candidate = pathlib.Path(shard_root).parent / rel
    return candidate if candidate.is_file() else None


def main() -> int:
    failures = []
    for rel, expected in sorted(FRAMES.items()):
        path = ROOT / rel
        if not path.is_file():
            # The frame moved to the game repository. Verify it THERE when a
            # checkout is present; skip when it is not, because fn64 must build
            # and lint standalone -- a plain clone has no game content at all.
            # Failing here would make the extracted repo a build dependency of
            # fn64, which is the coupling the extraction removed.
            moved = game_repo_path(rel)
            if moved is None:
                print(
                    f"reference-frame-digests: {rel}: not in this repository "
                    "(extracted to the game repo); set FN64_SHARD_ROOT to verify "
                    "its digest",
                    file=sys.stderr,
                )
                continue
            actual = hashlib.sha256(moved.read_bytes()).hexdigest()
            if actual != expected:
                failures.append(
                    f"{rel}: docs cite {expected}, game-repo file hashes {actual}"
                )
            continue
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual != expected:
            failures.append(f"{rel}: docs cite {expected}, file hashes {actual}")

    for failure in failures:
        print(f"reference-frame-digests: {failure}", file=sys.stderr)
    if failures:
        return 1
    print(f"reference-frame-digests: clean ({len(FRAMES)} frame(s) match the cited digests)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
