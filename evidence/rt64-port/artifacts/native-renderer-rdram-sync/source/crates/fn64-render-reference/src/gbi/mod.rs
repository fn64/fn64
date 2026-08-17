//! A digest-typed geometry display-list decoder: enough opcodes to turn real
//! OoT-era display list (segmented vertex/matrix data, an MVP transform
//! stack, and G_TRI1/G_TRI2/G_QUAD triangle commands) into screen-space
//! filled polygons. Public no-op commands are explicit; reserved, malformed,
//! and unknown commands trap with their wire context so coverage is never
//! silently overstated.
//!
//! ## Provenance
//!
//! `Gfx` is the N64 SDK's public 64-bit-word display-list command
//! encoding: every `gsSP*`/`gsDP*` macro in the publicly published
//! `gbi.h` header (redistributed in countless public SDK-header mirrors
//! and referenced throughout N64 homebrew/modding documentation) packs to
//! exactly this two-`u32` shape -- opcode in the top byte of the first
//! word, remaining fields packed by the specific opcode. Every opcode byte
//! value and bit-field offset below is cited to the F3DEX_GBI_2 branch of
//! the public `gbi.h` (`ultra64/gbi.h`): `gDma1p`/`gDma2p` word packing
//! (gbi.h ~2046-2090), `gsSPVertex` (~2150), `__gsSP1Triangle_w1` (~2320,
//! F3DEX branch: indices in w0, each `v*2`), `gsSPMatrix` (~2106),
//! `gsMoveWd`/`gsSPSegment` (~2267/2578), `gsSPDisplayList` (~2177). This
//! module reads only the raw wire values, not any vendor SDK/microcode C
//! source -- the encoding is packaging-level ABI, the same standing as this
//! project's other public-ABI citations (`os_task.h`'s `OSTask_t`).
//!
//! ## Scope
//!
//! Interpreted (real effect): `G_VTX` (load transformed vertices into the
//! active family's cache), `G_MODIFYVTX` (all four public post-transform cache
//! fields), `G_TRI1`/`G_TRI2`/`G_QUAD` (triangles referencing loaded slots),
//! `G_MTX`/`G_POPMTX` (modelview/projection stack), `G_CULLDL` (clip-code
//! volume culling), `G_RDPHALF_1` + `G_BRANCH_Z` (screen-depth tail branch),
//! F3DZEX2 `G_BRANCH_W` (strict homogeneous-W tail branch),
//! `G_LINE3D` (clipped variable-width lines), `G_MOVEWORD` (segment,
//! light-count/color, fog, and force-matrix writes), `G_DL` (call/jump into a nested display
//! list), `G_SETOTHERMODE_H/L` (RDP cycle/filter/dither/render/alpha/
//! coverage/depth/blender state), `G_SETBLENDCOLOR` (alpha-test threshold),
//! `G_SETSCISSOR` (raster clip rectangle), `G_SETCONVERT`,
//! `G_SETKEYR`/`G_SETKEYGB`, `G_SETCIMG`, `G_SETFILLCOLOR`,
//! fill-cycle `G_FILLRECT`, copy/one/two-cycle `G_TEXRECT`, normal-cycle
//! `G_TEXRECTFLIP`, `G_RDPFULLSYNC`, and
//! `G_ENDDL` (stop). Renderable work is returned as an ordered [`RenderOp`]
//! stream; the compatibility triangle-only view is derived from that stream.
//!
//! `G_DMA_IO` executes against persistent RSP memory; unsupported
//! move-word/move-memory subindices, the three reserved special opcodes, and
//! any unrecognized byte are malformed-command traps. Texture, lighting, RDP
//! other-mode, alpha compare, and the color-combiner and framebuffer-blender
//! inputs needed by OoT are decoded.
// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use std::fmt::Write as _;

pub use fn64_render::{F3dex2UcodeCatalog, GeometryUcodeCatalog, GeometryWireFamily, UcodeDigest};


mod wire;
mod types;
mod matrix;
mod tmem;
mod state;
mod entries;
mod stream;
mod geometry;

/// TEMP instrumentation (env `FN64_DUMP_PROJ=1`): true only while dumping the
/// projection/vertex data for the FIRST substantial gameplay frame, then it
/// self-disables so the log is one frame, not the whole boot. Gated entirely
/// behind the env var; no cost when unset. Remove/keep behind the flag.
#[cfg(not(test))]
mod projdump;

pub use wire::*;
pub use types::*;
pub use state::*;
pub use entries::*;
pub use geometry::*;

#[cfg(test)]
mod tests;
