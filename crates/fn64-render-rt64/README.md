# fn64-rt64

Empty placeholder crate. This is deliberate, not an oversight.

`docs/DESIGN.md` section 1 explains why all C++/RT64 interop is quarantined
in its own crate rather than mixed into `fn64-runtime`. Section 1's
rationale point 2 is why this crate has no real implementation yet: the RSP
**gfx** task handoff signature RT64 needs to consume is, per
`aki-recomp/runtime/ABI-SURFACE.md` section (e), **not yet visible from
generated code in either ported game's current corpus** — no
`osSpTaskLoad`/`osSpTaskStartGo` `_recomp` call site has been reached by a
`profile.toml` rename wave yet. That document is explicit that this is "a
real gap, not a resolved ABI point."

Writing this crate's real content now would mean guessing a call shape
instead of extracting it mechanically the way every other ABI surface
symbol in this project was — exactly the kind of unevidenced claim
`AGENTS.md` rules out ("Cite actual bytes/addresses/commits, not a
plausible-sounding story," per the sibling `faki-tools` project's stricter
version of the same rule, which this project inherits in spirit).

**What is already resolved and can land whenever this crate gets its first
real wave** (see `docs/DESIGN.md` section 5, wave 4): the RSP **audio**
ucode task boundary. `ABI-SURFACE.md` section (e) has the full byte-verified
config (`text_offset`, `text_size`, `text_address`, indirect branch targets)
sourced from `games/NWXE/rsp/wm2000_audio.toml`. Audio task submission is
unblocked today; gfx task submission is blocked on new evidence, specifically
a `profile.toml` rename wave reaching an `osSpTaskLoad`/`osSpTaskStartGo`
call site so its real signature can be extracted the same mechanical way as
everything else in `ABI-SURFACE.md`.

Until then: this crate compiles, exposes nothing, and exists so the
workspace's dependency graph (`fn64-shell -> fn64-rt64 -> fn64-runtime`) is
real and buildable rather than aspirational.
