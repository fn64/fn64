# M3.3d integration handoff

The private `vi` module is declared from `src/lib.rs`; it intentionally exports
no public backend API yet. `execute_cpu_oracle` consumes M3.3a's
`DeviceRgba16Bytes`; do not add a raw-byte overload or a conversion from
`N64RecompRdramStorageBytes`.

M3.3c now composes the retained WGSL into its exact 4x2 native-fill submission
and validates the GPU bytes against this module's typed CPU oracle. This is a
bounded live mechanism, not a general VI claim. Any later integration must
preserve the remaining nonclaims until separately proven: no live
`ViPresentation` adapter, surface path, general
VI/filter/interlace behavior, RT64 parity, or performance evidence.

The live adapter must consume one complete `fn64_render::ViPresentation` and
derive every field admitted by `M3dPresentationSpec` while its retrace-scoped
physical-memory authority remains live. It must not move VI timing, guest
memory, or publication policy into this backend.
