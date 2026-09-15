---
name: Relocated boot-image slice is refused as a duplicate mapping
covers: [C3]
reported: 2026-09-15
---
repro: cold discovery on a ROM whose boot code copies a slice of the IPL3
image to a second VA and calls it there (7 corpus ROMs; see
docs/plans/corpus-campaign-2026-09-15.md)
expected: the slice is admitted as a Supported mapping at the relocated VA
(>= 3 proven direct-call targets land on prologues under one delta)
actual: delta-vote never votes with boot-bank calls, and the untabled
strategy drops any region overlapping the IPL3 copy in ROM space; the
recompile gate reports N `outside_all_mappings` destinations
