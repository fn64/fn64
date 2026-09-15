---
name: ROM compatibility funnel
---
in: a normalized ROM named only by an external private manifest
out: stage receipts for discover, pack, compile, boot, and sustained execution
! every stage has an explicit denominator and failure frontier
! zero unsupported destinations is required for the full-game gate
! earlier-stage success never implies a later-stage result
! a stage receipt binds the git rev, binary digest, and resource policy that produced it
! a resource cap or unrecognised tool output is never reported as a frontier
! frontier clusters rank mechanisms by ROMs unblocked, never by site occurrences
? cold discovery alone proves playability
