#!/usr/bin/env python3
"""Gate the committed reference frames against the digests the docs cite.

A frame digest in a doc claims a reproducible fact -- "task #654 renders
byte-identically" -- and lint-docs.py refuses a hash no test owns, because a
hash verified once by hand and never again cannot fail and so cannot warn.

The frames are in the repository, so the claim is checkable here and now: hash
the file, compare against what the docs assert. If a frame is ever recompressed
or re-rendered, this fails and names the file rather than letting the docs go
quietly stale.

# Fail closed on a missing frame (task 5.5)

Before task 5.5 this script exited 0 whenever a frame was absent AND
`FN64_SHARD_ROOT` was unset -- the exact shape `gates-must-fail-on-unusable-
input` (project memory) exists to forbid: a gate that cannot compare must not
report success. A plain clone of fn64 has no game content at all (the frames
were extracted to their own repository), so this was the ORDINARY state for
every contributor without a game checkout beside fn64 -- the digest gate was
silently a no-op for nearly everyone who ever ran it.

Now: the script exits 0 on a missing frame ONLY when called with
`--allow-missing`, which CI's docs job passes explicitly (with a comment
saying the digest is not verified in a plain clone -- an honest, visible
carve-out rather than a silent one). Anyone running this locally without the
flag and without `FN64_SHARD_ROOT`/the fixture gets a nonzero exit and a
message naming the env var to set, because THAT invocation has no excuse: a
developer who cares enough to run this script by hand should be told how to
make it verify something, not told it already did.

ponytail: a dict literal, not a manifest format. Add a row when a frame earns a
cited digest; reach for a parsed manifest only if these outgrow one screen.
"""
import argparse
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


def run(root: pathlib.Path, frames: dict, allow_missing: bool) -> tuple[list[str], int]:
    """(failures, unverified_count) -- split out of main() so the self-test
    can drive it against a synthetic ROOT/FRAMES without touching the real
    repository or environment."""
    failures = []
    unverified = 0
    for rel, expected in sorted(frames.items()):
        path = root / rel
        if not path.is_file():
            # The frame moved to the game repository. Verify it THERE when a
            # checkout is present; when it is not, this is unverifiable --
            # fn64 must build and lint standalone, so failing outright would
            # make the extracted repo a hard build dependency, which is the
            # coupling the extraction removed. But "unverifiable" is not
            # "clean": callers must say so explicitly with --allow-missing,
            # or this reports failure per gates-must-fail-on-unusable-input.
            moved = game_repo_path(rel)
            if moved is None:
                if allow_missing:
                    unverified += 1
                    print(
                        f"reference-frame-digests: {rel}: not in this repository "
                        "(extracted to the game repo); set FN64_SHARD_ROOT to verify "
                        "its digest -- unverified, not gated (--allow-missing)",
                        file=sys.stderr,
                    )
                    continue
                failures.append(
                    f"{rel}: not in this repository and FN64_SHARD_ROOT is unset "
                    "or the game checkout is absent -- this gate cannot compare "
                    "anything and must not report success; set FN64_SHARD_ROOT to "
                    "the game package root to verify, or pass --allow-missing if "
                    "you are deliberately running in a plain clone"
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
    return failures, unverified


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="exit 0 when a frame is absent and no game checkout is present to "
        "verify it against (a plain clone has no game content); CI's docs job "
        "passes this explicitly because the digest is not verified there. "
        "Without this flag, a missing frame with no FN64_SHARD_ROOT checkout "
        "is a hard failure, not a silent skip.",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="check the checker against synthetic frame sets, including both "
        "the --allow-missing and the fail-closed-without-it paths, and exit",
    )
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    failures, unverified = run(ROOT, FRAMES, args.allow_missing)

    for failure in failures:
        print(f"reference-frame-digests: {failure}", file=sys.stderr)
    if failures:
        return 1
    verified = len(FRAMES) - unverified
    print(
        f"reference-frame-digests: clean ({verified} frame(s) match the cited "
        f"digests, {unverified} unverified under --allow-missing)"
        if unverified
        else f"reference-frame-digests: clean ({len(FRAMES)} frame(s) match the cited digests)"
    )
    return 0


def self_test() -> int:
    """Exercise both paths task 5.5 added: `--allow-missing` tolerates an
    absent frame with no game checkout, and its absence (the ordinary local
    invocation) fails loudly and names FN64_SHARD_ROOT."""
    import tempfile

    cases_passed = 0
    cases_failed: list[str] = []

    def case(label: str, fn) -> None:
        nonlocal cases_passed
        try:
            fn()
            cases_passed += 1
        except AssertionError as error:
            cases_failed.append(f"{label}: {error}")

    with tempfile.TemporaryDirectory(prefix="check-reference-frame-digests-selftest-") as tmp:
        empty_root = pathlib.Path(tmp)
        frames = {"reference/some-frames/does-not-exist.png": "deadbeef" * 8}
        saved_env = os.environ.pop("FN64_SHARD_ROOT", None)
        try:

            def missing_frame_without_allow_missing_fails_naming_the_env_var():
                failures, unverified = run(empty_root, frames, allow_missing=False)
                assert len(failures) == 1, failures
                assert "FN64_SHARD_ROOT" in failures[0], failures[0]
                assert unverified == 0

            case(
                "a missing frame with no FN64_SHARD_ROOT and no --allow-missing fails, "
                "naming FN64_SHARD_ROOT",
                missing_frame_without_allow_missing_fails_naming_the_env_var,
            )

            def missing_frame_with_allow_missing_passes_but_is_counted_unverified():
                failures, unverified = run(empty_root, frames, allow_missing=True)
                assert failures == [], failures
                assert unverified == 1

            case(
                "a missing frame WITH --allow-missing exits clean (0 failures) but "
                "counts as unverified, not silently as verified",
                missing_frame_with_allow_missing_passes_but_is_counted_unverified,
            )
        finally:
            if saved_env is not None:
                os.environ["FN64_SHARD_ROOT"] = saved_env

        # A frame that IS present and matches passes regardless of the flag,
        # and is never counted unverified.
        present_root = pathlib.Path(tmp) / "present"
        frame_rel = "reference/some-frames/present.png"
        frame_path = present_root / frame_rel
        frame_path.parent.mkdir(parents=True)
        frame_path.write_bytes(b"pixels")
        digest = hashlib.sha256(b"pixels").hexdigest()

        def present_matching_frame_passes_and_is_not_unverified():
            failures, unverified = run(present_root, {frame_rel: digest}, allow_missing=False)
            assert failures == [], failures
            assert unverified == 0

        case(
            "a present, matching frame passes and is not counted unverified",
            present_matching_frame_passes_and_is_not_unverified,
        )

        def present_mismatched_frame_fails_regardless_of_allow_missing():
            failures, _unverified = run(present_root, {frame_rel: "0" * 64}, allow_missing=True)
            assert len(failures) == 1, failures
            assert "docs cite" in failures[0]

        case(
            "a present frame whose hash MISMATCHES fails even with --allow-missing "
            "(the flag tolerates absence, never a wrong digest)",
            present_mismatched_frame_fails_regardless_of_allow_missing,
        )

    if cases_failed:
        for failure in cases_failed:
            print(f"SELF-TEST FAIL: {failure}", file=sys.stderr)
        print(f"{len(cases_failed)} self-test case(s) failed", file=sys.stderr)
        return 1
    print(f"check-reference-frame-digests self-test: {cases_passed} cases passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
