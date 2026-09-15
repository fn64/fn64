---
covers: [B8, C3]
---
given a ROM whose untabled strategy admits a Supported mapping
when gate-rom-recompile runs
then destinations inside that mapping classify as mapped_not_proven_code, never outside_all_mappings
and the receipt still labels the bank Supported, not Proven
and the AKI regression ROMs remain at unsupported == 0
