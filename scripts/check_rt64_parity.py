"""Assert the wgpu backend agrees with the RT64 C++ oracle.

Reads the parity runner's JSON on stdin. Exits nonzero on any new divergence,
on an accounted case changing its exact outcome, or on a corpus that shrank.
A gate that cannot compare must fail rather than report success.
"""
from dataclasses import dataclass
from enum import Enum
import json
import os
import sys


class Kind(Enum):
    RT64_DEFECT = "RT64_DEFECT"
    FN64_CAPABILITY_GAP = "FN64_CAPABILITY_GAP"
    BROKEN_FIXTURE = "BROKEN_FIXTURE"


@dataclass(frozen=True)
class ExpectedOutcome:
    kind: Kind
    verdict: str
    citation: str
    reason: str
    wgpu_outcome: str = "completed"
    rt64_outcome: str = "completed"
    wgpu_matches_key: bool | None = None
    rt64_matches_key: bool | None = None
    differing_pixels: int | None = None


# Exact assertions, not permissions to skip comparison. A changed result is
# deliberately RED until this entry and its cited evidence are reviewed.
EXPECTED_OUTCOMES = {
    "scissor-narrower-than-rect": ExpectedOutcome(
        Kind.RT64_DEFECT,
        "differs",
        "fn64-render-conformance-parity-runner.rs:149-180 (public gDPSetScissor S10.2 derivation)",
        "the scissor excludes 38,400 pixels; fn64 must match the key and RT64 must not",
        wgpu_matches_key=True,
        rt64_matches_key=False,
        differing_pixels=38_400,
    ),
    "textured-rect-flip-point-sampled": ExpectedOutcome(
        Kind.FN64_CAPABILITY_GAP,
        "one-refused",
        "crates/fn64-render-wgpu/src/raw_dpc/mod.rs:1938-1940",
        "TEXRECTFLIP must refuse loudly until implemented; completion requires byte identity",
        wgpu_outcome="refused",
    ),
    "textured-rect-yuv16": ExpectedOutcome(
        Kind.FN64_CAPABILITY_GAP,
        "one-refused",
        "crates/fn64-render-wgpu/src/tmem/wire.rs:649-652",
        "YUV must refuse loudly until implemented; completion requires byte identity",
        wgpu_outcome="refused",
    ),
    "perspective-textured-triangle-negative-w": ExpectedOutcome(
        Kind.BROKEN_FIXTURE,
        "differs",
        "crates/fn64-render-wgpu/src/rdp_harness/tests.rs:851-870",
        "RT64 does not sample this geometry; replace it while the signed-W unit test pins the fix",
        wgpu_matches_key=True,
        rt64_matches_key=False,
        differing_pixels=12,
    ),
}


def outcome(value):
    return "refused" if isinstance(value, dict) and "refused" in value else "completed"


d = json.load(sys.stdin)
p = d["parity"]
auth = p["rt64_authoritative"]
cov = p["rt64_not_authoritative_coverage"]
min_cases = int(os.environ.get("MIN_AUTHORITATIVE_CASES", "33"))
exp_coverage_refused = int(os.environ.get("EXPECTED_COVERAGE_ONE_REFUSED", "1"))
failures = []

if auth["cases"] < min_cases:
    failures.append(
        f"authoritative corpus shrank to {auth['cases']} cases "
        f"(expected >= {min_cases}); a vanished corpus is not a pass"
    )

if cov["one_refused"] != exp_coverage_refused:
    failures.append(
        f"coverage partition one_refused={cov['one_refused']}, "
        f"expected {exp_coverage_refused}"
    )

rows = {row["case"]: row for row in d["rows"]}
for case, expected in EXPECTED_OUTCOMES.items():
    row = rows.get(case)
    prefix = f"{case}: {expected.kind.value}"
    suffix = f" -- {expected.reason}; citation: {expected.citation}"
    if row is None:
        failures.append(f"{prefix} stale entry: case is absent{suffix}")
        continue
    if row["authority"] != "rt64-authoritative":
        failures.append(
            f"{prefix} expected rt64-authoritative, got {row['authority']}{suffix}"
        )
    if row["verdict"] != expected.verdict:
        failures.append(
            f"{prefix} expected verdict {expected.verdict}, got {row['verdict']}{suffix}"
        )
    for backend, wanted in (
        ("wgpu", expected.wgpu_outcome),
        ("rt64", expected.rt64_outcome),
    ):
        got = outcome(row[backend])
        if got != wanted:
            failures.append(
                f"{prefix} expected {backend} outcome {wanted}, got {got}{suffix}"
            )
    for field, wanted in (
        ("wgpu_matches_key", expected.wgpu_matches_key),
        ("rt64_matches_key", expected.rt64_matches_key),
    ):
        if wanted is not None and row.get(field) is not wanted:
            failures.append(
                f"{prefix} expected {field}={wanted}, got {row.get(field)}{suffix}"
            )
    if (
        expected.differing_pixels is not None
        and row.get("differing_pixels") != expected.differing_pixels
    ):
        failures.append(
            f"{prefix} expected differing_pixels={expected.differing_pixels}, "
            f"got {row.get('differing_pixels')}{suffix}"
        )

for row in d["rows"]:
    if row["authority"] != "rt64-authoritative":
        continue
    case = row["case"]
    if case in EXPECTED_OUTCOMES:
        continue
    if row["verdict"] != "identical":
        failures.append(f"{case}: unaccounted RT64 divergence ({row['verdict']})")
    # Matching RT64 while both are wrong is not parity.
    if row.get("wgpu_matches_key") is False:
        failures.append(f"{case}: wgpu does not match the hand-derived key")

if failures:
    print("RT64 PARITY GATE: FAIL")
    for failure in failures:
        print(f"  {failure}")
    sys.exit(1)

print(
    f"RT64 PARITY GATE: PASS -- {auth['byte_identical']}/{auth['cases']} "
    "rt64-authoritative cases byte-identical to the RT64 C++ oracle"
)
for case, expected in EXPECTED_OUTCOMES.items():
    print(
        f"  {case}: {expected.kind.value} asserted {expected.verdict}; "
        f"{expected.citation}"
    )
