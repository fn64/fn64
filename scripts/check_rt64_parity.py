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

exp_differs = int(os.environ.get("EXPECTED_DIFFERS", "3"))
exp_refused = int(os.environ.get("EXPECTED_ONE_REFUSED", "1"))
min_cases = int(os.environ.get("MIN_AUTHORITATIVE_CASES", "32"))

# Known, measured divergences. Each is explained in the runner's `intent`.
KNOWN_DIVERGENCES = {
    "scissor-narrower-than-rect",
    # TEXRECTFLIP (0x25) is UNIMPLEMENTED in the wgpu raw-DPC slice and
    # refuses LOUDLY rather than dropping silently: `plan_texture_rectangle`
    # returns early for `flip()` because "declaring a write no executor fills
    # would promise content that never arrives" (`raw_dpc/mod.rs:1938`). RT64
    # renders it. A known FEATURE GAP, not an accepted behavioural
    # difference -- delete this entry when the flip executor lands.
    "textured-rect-flip-point-sampled",
    # INEFFECTIVE AS WRITTEN, kept visible rather than deleted or forced
    # green. This case was added to pin the signed-W perspective fix, but
    # RT64 never reaches its sampler for this geometry, so the 12 differing
    # pixels are NOT evidence about the fix -- the hand-derived key does not
    # describe what RT64 computes here. The fix itself IS pinned, by
    # `a_negative_w_flips_the_raw_s10_5_coordinate`
    # (`rdp_harness/tests.rs`), which asserts the raw S10.5 coordinate
    # directly and is mutation-proven: reverting the denominator to
    # `unsigned_abs()` fails it with `left: (8192, 0)` vs `right: (-8192, 0)`.
    # Replace this case with geometry RT64 actually samples, then delete this
    # entry. Changing its expected pixels to match would leave a green
    # fixture that pins nothing.
    "perspective-textured-triangle-negative-w",
    # OPEN DEFECTS, newly found by extending the corpus past RGBA16/CI4.
    # Recorded so the rest of the corpus stays enforceable while these are
    # investigated -- NOT accepted as correct behaviour. Delete each entry
    # when its defect is fixed and the gate will demand byte-identity.
    #
    # RGBA32: wgpu and RT64 produce different pixels. RGBA/32b was one of the
    # nine format x size combinations the corpus never exercised, which is
    # exactly why a green gate coexisted with visibly wrong output.
    "textured-rect-rgba32",
    # YUV16: wgpu REFUSES where RT64 renders. YUV is the one format family
    # fn64 has never implemented; the refusal is loud rather than silent,
    # which is correct behaviour for an unimplemented feature, but it is a
    # capability gap a ROM using YUV would hit.
    "textured-rect-yuv16",
}

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
    f"  {exp_differs} known divergences: scissor-narrower-than-rect (RT64 paints "
    "outside the scissor, wgpu is right)"
)
