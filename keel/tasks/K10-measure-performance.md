---
covers: [C4, B5]
depends: [K07, K09]
pitch: "Establish an admitted post-fix performance baseline before selecting further renderer or runtime optimization work."
---
measure the actual 30 Hz rendered-frame budget and split guest, join, raster,
TMEM, upload, copyback, presentation, and audio-underrun costs.

deliverables:
- reproducible post-fix performance receipt

verification:
- T5
