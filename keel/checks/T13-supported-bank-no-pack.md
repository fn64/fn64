---
covers: [B11, C3]
---
given a ROM whose discovery admits a Supported bank with zero proven blocks
when gate-rom-recompile runs
then no block pack is emitted for that bank and the gate reaches HEADLINE
and destinations inside that bank still classify as mapped_not_proven_code
and every ROM that certified on the first 2026-09-15 campaign still certifies
