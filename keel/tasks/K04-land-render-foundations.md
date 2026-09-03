---
covers: [C2, C4, B2]
depends: [K01, K02]
pitch: "Extract renderer ownership, copyback sharing, worker reuse, and replay foundations as independently reviewable units."
---
start with the early renderer spine through exact task replay; preserve literal
RT64 arithmetic while isolating scheduling and resource policy.

deliverables:
- renderer foundation landing units

verification:
- T2
