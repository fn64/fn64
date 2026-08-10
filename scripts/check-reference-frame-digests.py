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


def main() -> int:
    failures = []
    for rel, expected in sorted(FRAMES.items()):
        path = ROOT / rel
        if not path.is_file():
            failures.append(f"{rel}: cited by the docs but not in the repository")
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
