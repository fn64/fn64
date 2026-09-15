---
covers: [B7, C3]
---
given a ROM whose proven boot bank direct-calls >= 3 distinct VAs outside every mapping
when the relocated-slice vote runs over the whole ROM's prologue sites
then a unique delta with >= 3 votes and >= 2x margin yields one Supported mapping at the relocated VA
and a tied or under-margin vote stays Open with the vote counts reported
and the answer-key gates still grade wrong == 0 and the closure digest is unchanged
and the recompile gate on such a ROM reports fewer unsupported destinations than before
