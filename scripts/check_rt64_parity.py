"""Assert the wgpu backend agrees with the RT64 C++ oracle.

Reads the parity runner's JSON on stdin. Exits nonzero on any NEW divergence,
on a known divergence disappearing, or on a corpus that shrank -- a gate that
cannot compare must fail rather than report success.
"""
import json
import os
import sys

d = json.load(sys.stdin)
p = d["parity"]
auth = p["rt64_authoritative"]
cov = p["rt64_not_authoritative_coverage"]

exp_differs = int(os.environ.get("EXPECTED_DIFFERS", "1"))
exp_refused = int(os.environ.get("EXPECTED_ONE_REFUSED", "1"))
min_cases = int(os.environ.get("MIN_AUTHORITATIVE_CASES", "18"))

# Known, measured divergences. Each is explained in the runner's `intent`.
KNOWN_DIVERGENCES = {"scissor-narrower-than-rect"}

failures = []

if auth["cases"] < min_cases:
    failures.append(
        f"authoritative corpus shrank to {auth['cases']} cases "
        f"(expected >= {min_cases}); a vanished corpus is not a pass"
    )

if auth["differs"] != exp_differs:
    failures.append(
        f"rt64-authoritative differs={auth['differs']}, expected {exp_differs}"
    )
    for r in d["rows"]:
        if r["authority"] == "rt64-authoritative" and r["verdict"] != "identical":
            failures.append(
                f"  -> {r['case']}: {r['verdict']} ({r['differing_pixels']} px)"
            )

if cov["one_refused"] != exp_refused:
    failures.append(
        f"coverage partition one_refused={cov['one_refused']}, expected {exp_refused}"
    )

for r in d["rows"]:
    if r["authority"] != "rt64-authoritative":
        continue
    case = r["case"]
    if case in KNOWN_DIVERGENCES:
        # The known divergence must STILL diverge; if it stopped, the corpus
        # stopped exercising the case and the gate has gone blind.
        if r["verdict"] == "identical":
            failures.append(
                f"{case}: known divergence disappeared -- corpus no longer exercises it"
            )
        continue
    if r["verdict"] != "identical":
        failures.append(f"{case}: diverged from the RT64 oracle ({r['verdict']})")
    # Matching RT64 while BOTH are wrong is not parity.
    if r.get("wgpu_matches_key") is False:
        failures.append(f"{case}: wgpu does not match the hand-derived key")

if failures:
    print("RT64 PARITY GATE: FAIL")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)

print(
    f"RT64 PARITY GATE: PASS -- {auth['byte_identical']}/{auth['cases']} "
    "rt64-authoritative cases byte-identical to the RT64 C++ oracle"
)
print(
    f"  {exp_differs} known divergence (scissor-narrower-than-rect: "
    "RT64 paints outside the scissor, wgpu matches the derived key)"
)
