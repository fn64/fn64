---
covers: [B7, T10, C3]
depends: [K17]
parallel_with: [K19]
pitch: "Boot code that copies part of its own image to a second address is mapped there, so seven more corpus titles certify without a table."
---
add a `relocated_slice_vote` step to the untabled strategy in
crates/fn64-discover: vote sources are proven boot-bank direct-call targets
that are outside all mappings; landing sites are `addiu sp,sp,-N` prologue
offsets over the whole ROM; admit the unique delta with >= 3 distinct-target
votes and >= 2x margin whose extent (min..max target offset, page-rounded)
lies inside the ROM, is addressable, and is disjoint from existing mappings
in VA space. ROM-space overlap with the IPL3 copy is allowed for this step
only. Conclude Supported, cite the votes in the evidence note. Never weaken
any existing admission rule. Red test first on two corpus ROMs via env vars
(skip when unset), then grade-all wrong==0 and gate-determinism must pass.

deliverables:
- crates/fn64-discover/src/delta_vote.rs
- crates/fn64-discover/src/lib.rs
- crates/fn64-discover/tests/relocated_slice_vote.rs

verification:
- cargo nextest run -p fn64-discover
- scripts/grade-all.sh
- scripts/gate-determinism.sh
- scripts/corpus-campaign.zsh --rom-dir <ntsc> --rom-ids <the 7 ids>
