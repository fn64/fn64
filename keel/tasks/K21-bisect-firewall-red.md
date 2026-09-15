---
covers: [B9, T1]
pitch: "grade-all returns to wrong=0 and gate_overlay_regions to its recorded digest, so a discovery change can be judged against a green baseline again."
---
bisect main for the commit that turned nwxe-donor wrong=1 and moved the
gate_overlay_regions digest; decide per finding whether the code or the
recorded expectation is wrong, and fix the one that is. No threshold
tuning; wrong must return to 0 by correctness, not by re-recording.

deliverables:
- the fixing commit on main, or the corrected recorded digest with its evidence

verification:
- scripts/grade-all.sh
- scripts/gate-determinism.sh
