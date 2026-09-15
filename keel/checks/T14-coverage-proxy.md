---
covers: [C3]
---
given a campaign with catalog rows and pack/recompile receipts
when the rank report and dashboard build
then every ROM row carries mapped_bytes / code_run_bytes and recompiled_bytes / code_run_bytes
and a ROM with no recompile receipt shows the ratio as not_run, never 0
and the report prints the corpus median and the count of certified ROMs under 50% recompiled
and "certified" and "covered" are never summed into one number
