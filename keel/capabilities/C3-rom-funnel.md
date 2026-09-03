---
name: ROM compatibility funnel
---
in: a normalized ROM named only by an external private manifest
out: stage receipts for discover, pack, compile, boot, and sustained execution
! every stage has an explicit denominator and failure frontier
! zero unsupported destinations is required for the full-game gate
! earlier-stage success never implies a later-stage result
? cold discovery alone proves playability
