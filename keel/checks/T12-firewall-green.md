---
covers: [B9, C3, C5]
---
given the canonical answer-key environment on a clean main checkout
when scripts/grade-all.sh and scripts/gate-determinism.sh run
then every graded config reports wrong == 0 at the README recall figures
and every determinism gate matches its recorded digest ten of ten times
