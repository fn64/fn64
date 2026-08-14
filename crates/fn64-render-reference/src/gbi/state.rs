// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use fn64_render::{
    GeometryUcodeProfile, RenderError, TaskAdmissionGeneration,
};
use sha2::Digest;
use super::*;
use super::wire::*;
use super::types::*;
use super::matrix::*;
use super::tmem::*;
use super::entries::*;
use super::geometry::*;

/// Human-readable name for command diagnostics.
pub(super) fn opcode_name(opcode: u8) -> &'static str {
    match opcode {
        G_NOOP => "G_NOOP",
        G_VTX => "G_VTX",
        G_MODIFYVTX => "G_MODIFYVTX",
        G_CULLDL => "G_CULLDL",
        G_BRANCH_Z => "G_BRANCH_Z",
        G_TRI1 => "G_TRI1",
        G_TRI2 => "G_TRI2",
        G_QUAD => "G_QUAD",
        G_LINE3D => "G_LINE3D",
        G_TEXRECT => "G_TEXRECT",
        G_TEXRECTFLIP => "G_TEXRECTFLIP",
        G_POPMTX => "G_POPMTX",
        G_MTX => "G_MTX",
        G_MOVEWORD => "G_MOVEWORD",
        G_DL => "G_DL",
        G_ENDDL => "G_ENDDL",
        G_SPNOOP => "G_SPNOOP",
        0xE1 => "G_RDPHALF_1",
        G_SETOTHERMODE_L => "G_SETOTHERMODE_L",
        G_SETOTHERMODE_H => "G_SETOTHERMODE_H",
        G_RDPLOADSYNC => "G_RDPLOADSYNC",
        G_RDPPIPESYNC => "G_RDPPIPESYNC",
        G_RDPTILESYNC => "G_RDPTILESYNC",
        G_RDPFULLSYNC => "G_RDPFULLSYNC",
        G_RDPSETOTHERMODE => "G_RDPSETOTHERMODE",
        G_SETKEYGB => "G_SETKEYGB",
        G_SETKEYR => "G_SETKEYR",
        G_SETCONVERT => "G_SETCONVERT",
        G_SETPRIMDEPTH => "G_SETPRIMDEPTH",
        G_LOADTLUT => "G_LOADTLUT",
        0xF1 => "G_RDPHALF_2",
        G_LOADBLOCK => "G_LOADBLOCK",
        G_LOADTILE => "G_LOADTILE",
        G_SETTILESIZE => "G_SETTILESIZE",
        G_SETTILE => "G_SETTILE",
        G_FILLRECT => "G_FILLRECT",
        G_SETFILLCOLOR => "G_SETFILLCOLOR",
        G_SETFOGCOLOR => "G_SETFOGCOLOR",
        G_SETBLENDCOLOR => "G_SETBLENDCOLOR",
        G_SETCOMBINE => "G_SETCOMBINE",
        G_SETTIMG => "G_SETTIMG",
        G_SETPRIMCOLOR => "G_SETPRIMCOLOR",
        G_SETENVCOLOR => "G_SETENVCOLOR",
        G_SETSCISSOR => "G_SETSCISSOR",
        G_SETZIMG => "G_SETZIMG",
        G_SETCIMG => "G_SETCIMG",
        G_SPECIAL_1 => "G_SPECIAL_1",
        G_SPECIAL_2 => "G_SPECIAL_2",
        G_SPECIAL_3 => "G_SPECIAL_3",
        G_DMA_IO => "G_DMA_IO",
        G_LOAD_UCODE => "G_LOAD_UCODE",
        G_TEXTURE => "G_TEXTURE",
        G_GEOMETRYMODE => "G_GEOMETRYMODE",
        G_MOVEMEM => "G_MOVEMEM",
        _ => "G_<unrecognized>",
    }
}

// --- Recomp rdram memory model (swizzled) -------------------------------
//
// fn64's `rdram` is NOT a flat big-endian image. The N64Recomp memory
// macros (`refs/N64RecompSource/include/recomp.h:95-107`) store every
// aligned 32-bit word in HOST-NATIVE order (`MEM_W` = a bare
// `*(int32_t*)`, no byteswap) and reach sub-word bytes/halfwords through an
// address XOR (`MEM_B` uses `^3`, `MEM_H` uses `^2`) -- the standard
// "byteswap within a native word" trick that makes big-endian sub-word
// access work over a little-endian word array. The PI-DMA path
// (`fn64-runtime/src/rdram.rs:243` `dma_write_bytes`) writes cartridge
// bytes with the SAME per-byte `^3` swizzle, so EVERYTHING in rdram --
// CPU-built display lists AND DMA'd vertex/matrix data -- obeys this one
// model. A decoder that reads it as flat big-endian (the old
// `from_be_bytes`) gets each 32-bit word byte-reversed: OoT's first DL
// command `0xDE...` (G_DL) read flat-BE became `0x000001DE` (opcode
// `0x00`), so the whole list decoded as garbage and produced 0 triangles.
//
// These helpers read logical values THE WAY THE GAME DOES: an aligned word
// is a native-endian `u32` (== the logical big-endian word), and any
// byte/halfword within it is extracted by its logical position. This is
// exactly equivalent to `MEM_W` / `MEM_HU(^2)` / `MEM_BU(^3)`.

/// Read the logical big-endian 32-bit word at aligned byte `off`
/// (`off % 4 == 0` expected; misaligned reads still return the containing
/// word's native value, matching a `MEM_W` on a masked address). Returns 0
/// if the word runs past `rdram`.
#[inline]
pub(super) fn read_u32(rdram: &[u8], off: usize) -> u32 {
    let Some(aligned) = complete_storage_word(rdram, off) else {
        return 0;
    };
    fn64_runtime::RdramView::from_storage(rdram).read_u32(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(aligned).expect("GBI RDRAM address exceeds u32"),
    ))
}

#[inline]
pub(super) fn complete_storage_word(rdram: &[u8], off: usize) -> Option<usize> {
    let aligned = off & !3;
    aligned
        .checked_add(4)
        .filter(|&end| end <= rdram.len())
        .map(|_| aligned)
}

/// Read a logical byte at byte offset `off` (recomp `MEM_BU`: physical
/// index `off ^ 3`). Returns 0 past the end.
#[inline]
pub(super) fn read_u8(rdram: &[u8], off: usize) -> u8 {
    if complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_u8(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Read a logical signed 16-bit halfword at byte offset `off` (recomp
/// `MEM_H`). The two logical bytes `off` (MSB) and `off+1` (LSB) are read
/// through the `^3` byte swizzle and recombined big-endian. Returns 0 past
/// the end.
#[inline]
pub(super) fn read_i16(rdram: &[u8], off: usize) -> i16 {
    if !off.is_multiple_of(2) || complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_i16(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Read a logical unsigned 16-bit halfword at byte offset `off`.
#[inline]
pub(super) fn read_u16(rdram: &[u8], off: usize) -> u16 {
    if !off.is_multiple_of(2) || complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_u16(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Resolve a (possibly segmented) F3DEX2 address to a flat rdram byte
/// offset. The top byte is the segment number; the low 24 bits are the
/// offset within that segment. If a segment base was registered (via
/// `G_MOVEWORD`/`G_MW_SEGMENT`) it is added; segment 0 is the identity
/// (physical) segment on real hardware, so an unset segment resolves to its
/// low-24-bit offset unchanged -- which is also exactly what the pre-
/// existing non-segmented fixtures (segment byte 0x00, e.g. addr 0x1000)
/// rely on, keeping them working unchanged.
pub(super) fn resolve_addr(segments: &[u32; 16], addr: u32) -> usize {
    let seg = ((addr >> 24) & 0x0F) as usize;
    let off = (addr & 0x00FF_FFFF) as usize;
    segments[seg] as usize + off
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DecodeAdmissionPolicy {
    pub(super) raw_window_bytes: Option<(usize, usize)>,
    pub(super) force_branch: bool,
}

/// Decoder state carried across (possibly nested via `G_DL`) command
/// streams.
pub(super) struct DecodeState {
    /// Behavior-bearing identity for the active geometry microcode generation.
    /// Wire decoding derives its family from this value, while NoN and future
    /// variant behavior must not collapse to that broad family.
    pub(super) profile: GeometryUcodeProfile,
    pub(super) vtx_cache: [Vertex; MAX_GEOMETRY_VERTEX_CACHE],
    /// Slot validity is distinct from vertex contents: the all-zero default
    /// is a possible transformed vertex, but BranchW must not observe a slot
    /// that the active microcode generation has never loaded.
    pub(super) vtx_loaded: [bool; MAX_GEOMETRY_VERTEX_CACHE],
    pub(super) ops: Vec<RenderOp>,
    pub(super) segments: [u32; 16],
    /// Projection * modelview, recomputed whenever either changes. `None`
    /// means "no transform loaded yet" -> vertices pass through as raw `ob`
    /// screen coords (preserves the pre-existing raw-coordinate fixtures).
    pub(super) mvp: Option<Mat4>,
    /// Matrix staged by `G_MOVEMEM G_MV_MATRIX` until the public compound
    /// command's `G_MOVEWORD G_MW_FORCEMTX` marker makes it authoritative.
    pub(super) pending_forced_mvp: Option<Mat4>,
    pub(super) proj: Option<Mat4>,
    pub(super) modelview: Mat4,
    pub(super) mv_stack: Vec<Mat4>,
    /// Viewport scale/translate (screen mapping), if a `G_MOVEMEM` viewport
    /// was seen. Fields: `(sx, sy, sz, tx, ty, tz)` -- x/y map NDC to pixels,
    /// z maps NDC-z to the depth range (all already divided by 4 in
    /// `read_viewport`). Transformed vertices require this state: inventing a
    /// host-sized default would hide a missing `G_MV_VIEWPORT` DMA and map the
    /// same display list differently from hardware. With no matrix at all the
    /// raw `ob` coordinates retain the reference-fixture convention.
    pub(super) viewport: Option<Viewport>,
    pub(super) scissor: Option<ScissorRect>,
    /// Current F3DEX2 geometry mode (the `G_GEOMETRYMODE` accumulator). Its
    /// `G_CULL_FRONT`/`G_CULL_BACK` bits decide per-triangle culling.
    pub(super) geometry_mode: u32,
    /// RDP other-mode H/L plus blend-alpha threshold. F3DEX2 partial updates
    /// mutate this shared state; each emitted triangle snapshots it.
    pub(super) other_mode: OtherMode,
    /// RDP color-combiner + primitive/environment register state. This is
    /// independent of other-mode/render state, but snapshotted beside it.
    pub(super) combiner: CombinerState,
    /// Constant blender inputs. `blend_color.a` is mirrored into `other_mode`
    /// for alpha compare; the full RGBA values feed the framebuffer blender.
    pub(super) blend_color: [u8; 4],
    pub(super) fog_color: [u8; 4],
    /// Raw 32-bit RDP fill-color register. RGBA16 targets consume alternating
    /// high/low halfwords; RGBA32 targets consume the whole word per pixel.
    pub(super) fill_color: u32,
    /// Most recent `G_RDPHALF_1` payload. F3DEX2's two-command BranchLessZ
    /// sequence stages its segmented target here before `G_BRANCH_Z`.
    pub(super) rdp_half_1: Option<u32>,
    pub(super) dl_depth: u32,
    /// Total commands decoded this frame (all streams), checked against
    /// [`MAX_DL_COMMANDS`] so a cyclic branch list terminates.
    pub(super) cmds_decoded: u32,
    /// Texture-mapping decode state (SETTIMG image latch, tile descriptors,
    /// TLUT palette and G_TEXTURE enable/scale). See [`TexState`].
    pub(super) tex: TexState,
    /// Vertex-lighting decode state (`G_MV_LIGHT` diffuse/ambient structs +
    /// `G_MW_NUMLIGHT` count). Applied at `G_VTX` time when the geometry
    /// mode's `G_LIGHTING` bit is set. See [`LightState`].
    pub(super) lights: LightState,
    /// Screen-space X/Y directions loaded by `gSPLookAt`, consumed when
    /// `G_TEXTURE_GEN` replaces explicit vertex texture coordinates.
    pub(super) look_at: LookAtState,
    pub(super) fog: FogFactor,
    /// Explicit `gSPPerspNormalize` value. `None` means the display list has
    /// not programmed it; F3DEX2 ucode reloads preserve the live value.
    pub(super) persp_normalize: PerspectiveNormalize,
    /// RSP primitive clipping rectangle relative to the viewport. This is
    /// deliberately separate from `G_CULLDL`'s ordinary frustum codes.
    pub(super) clip_ratio: ClipRatio,
    /// First self-loaded text image not admitted as F3DEX2-compatible.
    /// Ordered decode stops at that load boundary.
    pub(super) unsupported_ucode_reload: Option<UcodeDigest>,
    /// Every admitted `G_LOAD_UCODE` generation in command order. Repeated
    /// addresses and identities stay distinct so native execution can be
    /// matched positionally instead of inheriting RT64's address cache.
    pub(super) admission_generations: Vec<TaskAdmissionGeneration>,
    /// Optional adapter-requested raw backing-store windows captured at the
    /// exact command boundary where each admitted self-load becomes active.
    /// The reference renderer leaves this disabled; native RT64 admission
    /// uses it because the pinned GBI database hashes more than one IMEM bank.
    pub(super) admission_raw_window_bytes: Option<(usize, usize)>,
    pub(super) admission_raw_windows: Vec<TaskAdmissionRawWindow>,
    pub(super) admission_raw_window_error: Option<String>,
    /// Active RT64 host enhancement. This is inspection policy rather than
    /// emulated RSP state: pinned RT64 overrides the ordinary BranchZ
    /// comparison when the field is enabled, so admission must follow the
    /// same executed command path before native entry.
    pub(super) force_branch: bool,
}

/// Public F3DEX2 vertex-fog state. With `G_FOG` enabled, the RSP generates
/// shade alpha as `clamp(ndc_z * multiplier + offset, 0, 255)`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FogFactor {
    pub(super) multiplier: i16,
    pub(super) offset: i16,
}

/// Limited-precision RSP perspective-divide normalization. In the float
/// reference path every nonzero scale cancels between transformed coordinates
/// and W; an explicitly programmed zero makes the divide degenerate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PerspectiveNormalize(pub(super) Option<u16>);

impl PerspectiveNormalize {
    pub(super) fn rejects_geometry(self) -> bool {
        self.0 == Some(0)
    }
}

/// Per-side public `FRUSTRATIO_1..6` coefficients. The macro normally writes
/// the same ratio to all four fields, but the RSP state is updated one word at
/// a time, so retaining each side independently preserves command ordering.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ClipRatio {
    pub(super) neg_x: u8,
    pub(super) neg_y: u8,
    pub(super) pos_x: u8,
    pub(super) pos_y: u8,
}

impl Default for ClipRatio {
    fn default() -> Self {
        Self {
            neg_x: 1,
            neg_y: 1,
            pos_x: 1,
            pos_y: 1,
        }
    }
}

impl ClipRatio {
    pub(super) fn write(&mut self, offset: u16, value: u32) {
        assert_eq!(
            value & 0xffff_0000,
            0,
            "G_MOVEWORD G_MW_CLIP ratio value must occupy the public low halfword"
        );
        let low = value as u16;
        match offset {
            G_MWO_CLIP_RNX | G_MWO_CLIP_RNY => {
                let ratio = u8::try_from(low).unwrap_or_else(|_| {
                    panic!(
                        "G_MOVEWORD G_MW_CLIP negative-side ratio {low:#06x} is not FRUSTRATIO_1..6"
                    )
                });
                assert!(
                    (1..=6).contains(&ratio),
                    "G_MOVEWORD G_MW_CLIP negative-side ratio {low:#06x} is not FRUSTRATIO_1..6"
                );
                if offset == G_MWO_CLIP_RNX {
                    self.neg_x = ratio;
                } else {
                    self.neg_y = ratio;
                }
            }
            G_MWO_CLIP_RPX | G_MWO_CLIP_RPY => {
                let signed = low as i16;
                assert!(
                    (-6..=-1).contains(&signed),
                    "G_MOVEWORD G_MW_CLIP positive-side ratio {low:#06x} is not -FRUSTRATIO_1..6"
                );
                let ratio = (-signed) as u8;
                if offset == G_MWO_CLIP_RPX {
                    self.pos_x = ratio;
                } else {
                    self.pos_y = ratio;
                }
            }
            _ => panic!(
                "G_MOVEWORD G_MW_CLIP offset {offset:#06x} is not a public clip-ratio destination"
            ),
        }
    }
}

pub(super) fn fresh_decode_state() -> DecodeState {
    fresh_decode_state_for_profile(GeometryUcodeProfile::from_public_family(
        GeometryWireFamily::F3dex2,
    ))
}

pub(super) fn fresh_decode_state_for_profile(profile: GeometryUcodeProfile) -> DecodeState {
    DecodeState {
        profile,
        vtx_cache: [Vertex::default(); MAX_GEOMETRY_VERTEX_CACHE],
        vtx_loaded: [false; MAX_GEOMETRY_VERTEX_CACHE],
        ops: Vec::new(),
        segments: [0u32; 16],
        mvp: None,
        pending_forced_mvp: None,
        proj: None,
        modelview: identity(),
        mv_stack: Vec::new(),
        viewport: None,
        scissor: None,
        geometry_mode: 0,
        other_mode: OtherMode::default(),
        combiner: CombinerState::default(),
        blend_color: [0; 4],
        fog_color: [0; 4],
        fill_color: 0,
        rdp_half_1: None,
        dl_depth: 0,
        cmds_decoded: 0,
        tex: TexState::default(),
        lights: LightState::default(),
        look_at: LookAtState::default(),
        fog: FogFactor::default(),
        persp_normalize: PerspectiveNormalize::default(),
        clip_ratio: ClipRatio::default(),
        unsupported_ucode_reload: None,
        admission_generations: Vec::new(),
        admission_raw_window_bytes: None,
        admission_raw_windows: Vec::new(),
        admission_raw_window_error: None,
        force_branch: false,
    }
}

/// Reset only the state that public F3DEX2 self-loading does not maintain.
///
/// The public F3DEX2 release notes explicitly retain the display-list stack,
/// matrix stack, modelview/projection matrices, segment table, scissor,
/// other mode, perspective normalization, and viewport. They explicitly say
/// that the combined MP matrix, geometry mode, lights, and vertex cache are
/// not retained. Independent RDP state also remains live. State absent from
/// the exhaustive maintained list is reset rather than guessed persistent.
pub(super) fn reset_rsp_state_from_ucode_load(state: &mut DecodeState) {
    state.vtx_cache = [Vertex::default(); MAX_GEOMETRY_VERTEX_CACHE];
    state.vtx_loaded = [false; MAX_GEOMETRY_VERTEX_CACHE];
    state.mvp = None;
    state.pending_forced_mvp = None;
    state.geometry_mode = 0;
    state.rdp_half_1 = None;
    state.tex.tex_enabled = false;
    state.tex.tex_tile = 0;
    state.tex.tex_max_level = 0;
    state.tex.tex_scale_s = 0.0;
    state.tex.tex_scale_t = 0.0;
    state.lights = LightState::default();
    state.look_at = LookAtState::default();
    state.fog = FogFactor::default();
    state.clip_ratio = ClipRatio::default();
}

/// The public F3DEX loadable-microcode contract initializes all RSP geometry
/// state, including segments, matrices, viewport, and display-list links.
/// RDP registers/TMEM remain independent and are deliberately not touched.
pub(super) fn reset_legacy_rsp_state_from_ucode_load(state: &mut DecodeState) {
    assert_eq!(
        state.dl_depth, 0,
        "F3DEX/L3DEX G_LOAD_UCODE inside a called display list resets link state and cannot return"
    );
    state.vtx_cache = [Vertex::default(); MAX_GEOMETRY_VERTEX_CACHE];
    state.vtx_loaded = [false; MAX_GEOMETRY_VERTEX_CACHE];
    state.segments = [0; 16];
    state.mvp = None;
    state.pending_forced_mvp = None;
    state.proj = None;
    state.modelview = identity();
    state.mv_stack.clear();
    state.viewport = None;
    state.geometry_mode = 0;
    state.rdp_half_1 = None;
    state.tex.tex_enabled = false;
    state.tex.tex_tile = 0;
    state.tex.tex_max_level = 0;
    state.tex.tex_scale_s = 0.0;
    state.tex.tex_scale_t = 0.0;
    state.lights = LightState::default();
    state.look_at = LookAtState::default();
    state.fog = FogFactor::default();
    state.persp_normalize = PerspectiveNormalize::default();
    state.clip_ratio = ClipRatio::default();
}

pub(super) fn initialize_geometry_profile_state(state: &mut DecodeState, profile: GeometryUcodeProfile) {
    state.profile = profile;
    let family = profile.wire_family();
    match family {
        GeometryWireFamily::F3dlx => {
            // The public F3DLX contract starts with clipping enabled and
            // permits later G_CLIPPING set/clear commands.
            state.geometry_mode |= LEGACY_G_CLIPPING;
        }
        GeometryWireFamily::F3dlxRej
        | GeometryWireFamily::F3dex2Rej
        | GeometryWireFamily::F3dlx2Rej => {
            // The public reject-box contract starts at FRUSTRATIO_2.
            state.clip_ratio = ClipRatio {
                neg_x: 2,
                neg_y: 2,
                pos_x: 2,
                pos_y: 2,
            };
        }
        GeometryWireFamily::F3dex2
        | GeometryWireFamily::F3dex2NoN
        | GeometryWireFamily::F3dzex2
        | GeometryWireFamily::L3dex2 => {
            // F3DEX2 changed the public CLIPRATIO default from 1 to 2.
            state.clip_ratio = ClipRatio {
                neg_x: 2,
                neg_y: 2,
                pos_x: 2,
                pos_y: 2,
            };
        }
        GeometryWireFamily::Fast3d | GeometryWireFamily::F3dex | GeometryWireFamily::L3dex => {}
    }
}

#[cfg(test)]
pub(super) fn initialize_geometry_family_state(state: &mut DecodeState, family: GeometryWireFamily) {
    initialize_geometry_profile_state(state, GeometryUcodeProfile::from_public_family(family));
}

/// F3DEX2 vertex-lighting decode state (`F3DEX2-CONCEPTS.md` §2.4). The
/// RSP holds up to 7 directional lights plus one ambient; `num_dir` selects
/// how many directional slots are active, and the ambient light is the slot
/// at index `num_dir`. Directions are stored NORMALIZED in eye/model space
/// (s8 ÷127); the light-space transform uses the current modelview.
#[derive(Clone, Debug)]
pub(super) struct LightState {
    /// Diffuse light slots (`G_MV_LIGHT`): direction (unit, s8÷127) + RGB
    /// color (0..1). Slot `num_dir` doubles as the ambient's color carrier
    /// when written, but ambient is read via `ambient` below.
    pub(super) dir: [DirLight; MAX_LIGHTS],
    /// Ambient light color (0..1) -- the highest-numbered light slot.
    pub(super) ambient: [f32; 3],
    /// Number of active directional lights (`G_MW_NUMLIGHT` / 24).
    pub(super) num_dir: usize,
}

impl Default for LightState {
    fn default() -> Self {
        LightState {
            dir: [DirLight::default(); MAX_LIGHTS],
            // A conservative default: no ambient, no directionals, so a DL
            // that enables G_LIGHTING but (somehow) loaded no lights renders
            // dark rather than garbage -- but real OoT always loads both.
            ambient: [0.0, 0.0, 0.0],
            num_dir: 0,
        }
    }
}

/// One decoded directional light: a unit direction (light-space, s8÷127) and
/// an RGB diffuse color (0..1).
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct DirLight {
    pub(super) dir: [f32; 3],
    pub(super) col: [f32; 3],
}

pub(super) const TMEM_BYTES: usize = 4 * 1024;
pub(super) const TMEM_HALF_BYTES: usize = TMEM_BYTES / 2;

/// Physical RDP texture memory in bank order. A validity mask is retained per
/// byte so an uninitialized fetch traps by exact TMEM address instead of
/// manufacturing a color. Four-bit writes mark only the nibble transferred.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Tmem {
    pub(super) bytes: Box<[u8; TMEM_BYTES]>,
    pub(super) valid: Box<[u8; TMEM_BYTES]>,
}

impl Default for Tmem {
    fn default() -> Self {
        Self {
            bytes: Box::new([0; TMEM_BYTES]),
            valid: Box::new([0; TMEM_BYTES]),
        }
    }
}

impl std::fmt::Debug for Tmem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let initialized_bits = self
            .valid
            .iter()
            .map(|mask| mask.count_ones() as usize)
            .sum::<usize>();
        f.debug_struct("Tmem")
            .field("initialized_bits", &initialized_bits)
            .finish_non_exhaustive()
    }
}

/// Immutable TMEM view captured with a primitive. The RDP register file may
/// be mutated by later commands before the backend rasterizes the operation,
/// so retaining only a tile number would violate command ordering.
#[derive(Clone, Debug)]
pub(crate) struct TmemTexture {
    pub(super) storage: std::rc::Rc<Tmem>,
    pub(super) tile: Tile,
    pub(super) texture_lut: u8,
    /// Memoized palette entries for this view's `texture_lut` mode.
    ///
    /// A CI texel's color is a pure function of `(index, texture_lut)`, and
    /// both `storage` and `texture_lut` are immutable for the life of this
    /// view -- so decoding an index twice cannot observe a different color.
    /// Palettized primitives dominate this renderer's profile (`read_tlut`
    /// was its single largest symbol, larger than the whole color combiner),
    /// because bilinear filtering re-decodes four TMEM palette entries for
    /// every pixel.
    ///
    /// Filled lazily rather than eagerly: `G_LOADTLUT` need not populate all
    /// 256 entries, and an eager pass would read -- and trap on -- entries the
    /// primitive never samples. Per-index laziness keeps the uninitialized-
    /// TMEM assert firing for exactly the indices the old code validated.
    palette: Box<TlutCache>,
}

/// Sparse 256-entry memo of decoded TLUT colors.
///
/// `Cell` rather than `RefCell`: the entry is `Copy`, so get/set need no
/// borrow flag, and the hit path becomes a plain load and compare. `Box`ed so
/// cloning a `TmemTexture` (done per primitive) does not copy the table
/// inline.
#[derive(Clone, Debug)]
struct TlutCache {
    entries: [std::cell::Cell<Option<[u8; 4]>>; 256],
}

impl Default for TlutCache {
    fn default() -> Self {
        Self {
            entries: [const { std::cell::Cell::new(None) }; 256],
        }
    }
}

/// The memo is derived state, so two views comparing equal must not be
/// distinguished by how many palette entries either happens to have decoded.
/// Equality stays defined by the identity fields alone -- the same relation
/// the derived impl provided before the cache existed.
impl PartialEq for TmemTexture {
    fn eq(&self, other: &Self) -> bool {
        self.storage == other.storage
            && self.tile == other.tile
            && self.texture_lut == other.texture_lut
    }
}

impl Eq for TmemTexture {}

impl TmemTexture {
    pub(super) fn new(storage: std::rc::Rc<Tmem>, tile: Tile, texture_lut: u8) -> Self {
        Self {
            storage,
            tile,
            texture_lut,
            palette: Box::new(TlutCache::default()),
        }
    }

    /// Decode TLUT `index`, reusing this view's memo. Identical in value to
    /// `Tmem::read_tlut`, including its uninitialized-TMEM trap on the first
    /// access to any given index.
    #[inline]
    fn tlut_color(&self, index: usize) -> [u8; 4] {
        assert!(
            index < 256,
            "CI texel index {index} exceeds the 256-entry TLUT"
        );
        if let Some(color) = self.palette.entries[index].get() {
            return color;
        }
        self.tlut_color_miss(index)
    }

    /// The cold half of `tlut_color`, kept out of line so the hit path stays
    /// a load, a compare, and a return at every call site.
    #[cold]
    fn tlut_color_miss(&self, index: usize) -> [u8; 4] {
        let color = self.storage.read_tlut(index, self.texture_lut);
        self.palette.entries[index].set(Some(color));
        color
    }
}

/// The uninitialized-TMEM trap, out of line so `Tmem::read_byte` can inline.
///
/// Reproduces `assert_eq!(valid & mask, mask, ..)`'s message byte for byte --
/// `gbi::tests::group4` asserts the exact panic text, and that diagnostic is
/// the reason TMEM carries a validity mask at all.
#[cold]
#[inline(never)]
fn uninitialized_tmem_read(context: String, address: usize, valid: u8, mask: u8) -> ! {
    panic!(
        "assertion `left == right` failed: {context} reads uninitialized TMEM bits at byte {address:#05x}\n  left: {}\n right: {}",
        valid & mask,
        mask
    );
}

impl Tmem {
    #[inline]
    pub(super) fn physical_byte(logical: usize, odd_row: bool) -> usize {
        // Programming Manual 13.9 and SGI Load Block usage notes: odd rows
        // exchange the two 32-bit longs in each 64-bit word. TMEM addressing
        // wraps in the 12-bit physical byte domain.
        (logical & (TMEM_BYTES - 1)) ^ if odd_row { 4 } else { 0 }
    }

    pub(super) fn is_initialized(&self) -> bool {
        self.valid.iter().any(|mask| *mask != 0)
    }

    pub(super) fn write_byte(&mut self, logical: usize, odd_row: bool, value: u8) {
        let address = Self::physical_byte(logical, odd_row);
        self.bytes[address] = value;
        self.valid[address] = u8::MAX;
    }

    pub(super) fn write_nibble(&mut self, logical: usize, odd_row: bool, high: bool, value: u8) {
        let address = Self::physical_byte(logical, odd_row);
        let mask = if high { 0xf0 } else { 0x0f };
        let shifted = if high { value << 4 } else { value };
        self.bytes[address] = (self.bytes[address] & !mask) | (shifted & mask);
        self.valid[address] |= mask;
    }

    /// `context` is a lazy diagnostic evaluated ONLY on the uninitialized-read
    /// panic path. Passing a closure (not a formatted `&str`) keeps the
    /// per-texel hot loop from allocating/formatting a context string on every
    /// valid read -- a live profile showed that eager formatting dominating the
    /// software rasterizer's texel fetch. `FnOnce` avoids any dynamic dispatch.
    ///
    /// `#[inline]` with the failure arm split into a `#[cold]` helper: this is
    /// called one to four times per texel, and bilinear filtering multiplies
    /// that by four again per pixel. Inlining lets the caller keep the
    /// physical address in a register across a multi-byte texel; keeping the
    /// panic formatting out of line stops it from inflating the inlined body.
    #[inline]
    pub(super) fn read_byte(
        &self,
        logical: usize,
        odd_row: bool,
        mask: u8,
        context: impl FnOnce() -> String,
    ) -> u8 {
        let address = Self::physical_byte(logical, odd_row);
        if self.valid[address] & mask != mask {
            uninitialized_tmem_read(context(), address, self.valid[address], mask);
        }
        self.bytes[address]
    }

    #[inline]
    pub(super) fn row_base(tile: Tile, row: usize) -> usize {
        usize::from(tile.tmem) * 8 + row * usize::from(tile.line) * 8
    }

    pub(super) fn write_texel(
        &mut self,
        tile: Tile,
        x: usize,
        row: usize,
        odd_row: bool,
        size: u8,
        value: u32,
    ) {
        let base = Self::row_base(tile, row);
        match size {
            G_IM_SIZ_4B => {
                self.write_nibble(base + x / 2, odd_row, x.is_multiple_of(2), value as u8)
            }
            G_IM_SIZ_8B => self.write_byte(base + x, odd_row, value as u8),
            G_IM_SIZ_16B => {
                for (byte, value) in (value as u16).to_be_bytes().into_iter().enumerate() {
                    self.write_byte(base + x * 2 + byte, odd_row, value);
                }
            }
            G_IM_SIZ_32B => {
                assert!(
                    base < TMEM_HALF_BYTES,
                    "32-bit texture load base {base:#05x} is outside low TMEM"
                );
                let [r, g, b, a] = value.to_be_bytes();
                let low = (base + x * 2) & (TMEM_HALF_BYTES - 1);
                self.write_byte(low, odd_row, r);
                self.write_byte(low + 1, odd_row, g);
                self.write_byte(low + TMEM_HALF_BYTES, odd_row, b);
                self.write_byte(low + TMEM_HALF_BYTES + 1, odd_row, a);
            }
            _ => unreachable!("RDP image size is a two-bit field"),
        }
    }

    pub(super) fn write_yuv_pair(
        &mut self,
        tile: Tile,
        pair: usize,
        row: usize,
        odd_row: bool,
        yuyv: [u8; 4],
    ) {
        let base = Self::row_base(tile, row);
        assert!(
            base < TMEM_HALF_BYTES,
            "YUV texture load base {base:#05x} is outside low TMEM"
        );
        let low = (base + pair * 2) & (TMEM_HALF_BYTES - 1);
        let [y0, u, y1, v] = yuyv;
        self.write_byte(low, odd_row, u);
        self.write_byte(low + 1, odd_row, v);
        self.write_byte(low + TMEM_HALF_BYTES, odd_row, y0);
        self.write_byte(low + TMEM_HALF_BYTES + 1, odd_row, y1);
    }

    #[inline]
    pub(super) fn read_texel(&self, tile: Tile, x: usize, row: usize, size: u8) -> u32 {
        let base = Self::row_base(tile, row);
        let odd_row = (usize::from(tile.ult) / 4 + row) & 1 != 0;
        // Lazy diagnostic: only formatted if a read hits uninitialized TMEM.
        // `tmem`, `x`, `row` are Copy, so a fresh closure per call is free.
        let tmem = tile.tmem;
        let ctx = || format!("tile at TMEM word {tmem} texel ({x}, {row})");
        match size {
            G_IM_SIZ_4B => {
                let high = x.is_multiple_of(2);
                let mask = if high { 0xf0 } else { 0x0f };
                let byte = self.read_byte(base + x / 2, odd_row, mask, ctx);
                u32::from(if high { byte >> 4 } else { byte & 0x0f })
            }
            G_IM_SIZ_8B => u32::from(self.read_byte(base + x, odd_row, 0xff, ctx)),
            G_IM_SIZ_16B => {
                let bytes = [
                    self.read_byte(base + x * 2, odd_row, 0xff, ctx),
                    self.read_byte(base + x * 2 + 1, odd_row, 0xff, ctx),
                ];
                u32::from(u16::from_be_bytes(bytes))
            }
            G_IM_SIZ_32B => {
                assert!(
                    base < TMEM_HALF_BYTES,
                    "32-bit texture sample base {base:#05x} is outside low TMEM"
                );
                let low = (base + x * 2) & (TMEM_HALF_BYTES - 1);
                u32::from_be_bytes([
                    self.read_byte(low, odd_row, 0xff, ctx),
                    self.read_byte(low + 1, odd_row, 0xff, ctx),
                    self.read_byte(low + TMEM_HALF_BYTES, odd_row, 0xff, ctx),
                    self.read_byte(low + TMEM_HALF_BYTES + 1, odd_row, 0xff, ctx),
                ])
            }
            _ => unreachable!("RDP image size is a two-bit field"),
        }
    }

    pub(super) fn write_tlut(&mut self, base_word: u16, index: usize, value: u16) {
        assert!(
            base_word >= 256,
            "G_LOADTLUT destination word {base_word} is outside high TMEM"
        );
        let base = usize::from(base_word) * 8 + index * 8;
        let [hi, lo] = value.to_be_bytes();
        for bank in 0..4 {
            self.write_byte(base + bank * 2, false, hi);
            self.write_byte(base + bank * 2 + 1, false, lo);
        }
    }

    pub(super) fn read_tlut(&self, index: usize, mode: u8) -> [u8; 4] {
        assert!(
            index < 256,
            "CI texel index {index} exceeds the 256-entry TLUT"
        );
        let base = TMEM_HALF_BYTES + index * 8;
        let ctx = || format!("TLUT index {index}");
        let value = u16::from_be_bytes([
            self.read_byte(base, false, 0xff, ctx),
            self.read_byte(base + 1, false, 0xff, ctx),
        ]);
        match mode {
            2 => rgba5551_to_rgba8888(value),
            3 => ia16_to_rgba8888((value >> 8) as u8, value as u8),
            _ => crate::render_unsupported_panic(
                "render.gbi.texture-lut-mode",
                format!("CI texture sampled with texture-LUT mode {mode}, expected RGBA16 or IA16"),
            ),
        }
    }
}

impl TmemTexture {
    #[inline]
    pub(super) fn raw_texel(&self, x: usize, y: usize) -> u32 {
        self.storage.read_texel(self.tile, x, y, self.tile.siz)
    }

    #[inline]
    pub(super) fn sample(&self, x: usize, y: usize) -> [u8; 4] {
        if self.tile.fmt == G_IM_FMT_YUV && self.tile.siz == G_IM_SIZ_16B {
            let base = Tmem::row_base(self.tile, y);
            let odd_row = (usize::from(self.tile.ult) / 4 + y) & 1 != 0;
            let pair = x / 2;
            let tmem = self.tile.tmem;
            let ctx = || format!("YUV tile at TMEM word {tmem} texel ({x}, {y})");
            let low = base + pair * 2;
            let high = low + TMEM_HALF_BYTES;
            let u = self.storage.read_byte(low, odd_row, 0xff, ctx);
            let v = self.storage.read_byte(low + 1, odd_row, 0xff, ctx);
            let luma = self.storage.read_byte(high + (x & 1), odd_row, 0xff, ctx);
            return [luma, u, v, 255];
        }

        let raw = self.raw_texel(x, y);
        // EN_TLUT is a pipeline mode, not a tile-format property. With it
        // enabled, every 4-bit texel is palette-relative and every 8-bit
        // texel is a direct high-TMEM index regardless of the tile's declared
        // format. WM2000 relies on this for an IA8-declared title image.
        if self.texture_lut != 0 {
            match self.tile.siz {
                G_IM_SIZ_4B => {
                    let index = (usize::from(self.tile.palette) << 4) | raw as usize;
                    return self.tlut_color(index);
                }
                G_IM_SIZ_8B => return self.tlut_color(raw as usize),
                _ => crate::render_unsupported_panic(
                    "render.gbi.texture-lut-size",
                    format!(
                        "texture-LUT sampling of a {}-coded {}b tile at TMEM word {} is unsupported",
                        self.tile.fmt, self.tile.siz, self.tile.tmem
                    ),
                ),
            }
        }
        match (self.tile.fmt, self.tile.siz) {
            (G_IM_FMT_RGBA, G_IM_SIZ_16B) => rgba5551_to_rgba8888(raw as u16),
            (G_IM_FMT_RGBA, G_IM_SIZ_32B) => raw.to_be_bytes(),
            (G_IM_FMT_RGBA, G_IM_SIZ_8B) | (G_IM_FMT_I, G_IM_SIZ_8B) => i8_to_rgba8888(raw as u8),
            (G_IM_FMT_RGBA, G_IM_SIZ_4B) | (G_IM_FMT_I, G_IM_SIZ_4B) => i4_to_rgba8888(raw as u8),
            (G_IM_FMT_IA, G_IM_SIZ_16B) => ia16_to_rgba8888((raw >> 8) as u8, raw as u8),
            (G_IM_FMT_IA, G_IM_SIZ_8B) => ia8_to_rgba8888(raw as u8),
            (G_IM_FMT_IA, G_IM_SIZ_4B) => ia4_to_rgba8888(raw as u8),
            // The TLUT-enabled paths returned above; without the TLUT an
            // 8-bit index decodes as intensity (hardware's CI-as-I behavior)
            // and a 4-bit index still has no palette to resolve through --
            // `read_tlut` keeps its loud mode trap for that case.
            (G_IM_FMT_CI, G_IM_SIZ_8B) => i8_to_rgba8888(raw as u8),
            (G_IM_FMT_CI, G_IM_SIZ_4B) => {
                let index = (usize::from(self.tile.palette) << 4) | raw as usize;
                self.tlut_color(index)
            }
            (format, size) => crate::render_unsupported_panic(
                "render.gbi.texture-format",
                format!(
                    "TMEM tile uses unsupported texture format {format} size {size} at word {}",
                    self.tile.tmem
                ),
            ),
        }
    }
}

/// Texture-pipeline decode state (`F3DEX2-CONCEPTS.md` §5). Kept as a
/// sub-struct so the transform/geometry state above stays readable.
#[derive(Clone, Debug, Default)]
pub(super) struct TexState {
    /// `G_SETTIMG`: the source texture image -- segmented addr + format +
    /// size-code. Latched; no data moves until a `G_LOAD*`.
    pub(super) timg_addr: u32,
    pub(super) timg_fmt: u8,
    pub(super) timg_siz: u8,
    pub(super) timg_width: u16,
    /// The 8 RDP tile descriptors (`G_SETTILE`/`G_SETTILESIZE`).
    pub(super) tiles: [Tile; 8],
    /// The RDP's physical 4 KiB texture memory. Loads mutate this storage;
    /// render tiles merely reinterpret it.
    pub(super) tmem: std::rc::Rc<Tmem>,
    /// `G_LOADTLUT` palette: up to 256 RGBA8888 entries decoded from the
    /// TLUT image (CI textures index into this).
    pub(super) tlut: Vec<[u8; 4]>,
    /// `G_TEXTURE`: texturing enabled?
    pub(super) tex_enabled: bool,
    /// `G_TEXTURE`: which tile descriptor is active (0-7).
    pub(super) tex_tile: u8,
    /// `G_TEXTURE`: number of MIP levels following the primitive tile.
    pub(super) tex_max_level: u8,
    /// `G_TEXTURE` S/T scale (U0.16 -> f32), applied to the raw vertex S/T
    /// before texel addressing.
    pub(super) tex_scale_s: f32,
    pub(super) tex_scale_t: f32,
}

/// RDP register/TMEM state that survives RSP task boundaries.
///
/// `G_TEXTURE` enable/tile/scale fields live in the RSP microcode state and
/// are deliberately cleared when capturing this snapshot. The texture-image
/// latch, tile descriptors, TMEM validity/data, TLUT, other mode, combiner,
/// constant colors, fill color, and scissor are RDP state and remain live for
/// the next HLE task or raw DPC submission.
#[derive(Clone, Debug, Default)]
pub(crate) struct RdpDecodeState {
    pub(super) tex: TexState,
    pub(super) scissor: Option<ScissorRect>,
    pub(super) other_mode: OtherMode,
    pub(super) combiner: CombinerState,
    pub(super) blend_color: [u8; 4],
    pub(super) fog_color: [u8; 4],
    pub(super) fill_color: u32,
}

impl RdpDecodeState {
    pub(crate) fn texture_filter(&self) -> TextureFilter {
        self.other_mode.texture_filter()
    }

    pub(super) fn begin_task(&self) -> DecodeState {
        let mut state = fresh_decode_state();
        state.tex = self.tex.clone();
        state.scissor = self.scissor;
        state.other_mode = self.other_mode;
        state.combiner = self.combiner;
        state.blend_color = self.blend_color;
        state.fog_color = self.fog_color;
        state.fill_color = self.fill_color;
        state
    }

    pub(super) fn commit_task(&mut self, state: &DecodeState) {
        self.tex = state.tex.clone();
        // These fields are owned by F3DEX2, not the RDP. rspboot/ucode
        // initialization establishes them for each task; carrying them here
        // would make a later task textured without issuing G_TEXTURE.
        self.tex.tex_enabled = false;
        self.tex.tex_tile = 0;
        self.tex.tex_max_level = 0;
        self.tex.tex_scale_s = 0.0;
        self.tex.tex_scale_t = 0.0;
        self.scissor = state.scissor;
        self.other_mode = state.other_mode;
        self.combiner = state.combiner;
        self.blend_color = state.blend_color;
        self.fog_color = state.fog_color;
        self.fill_color = state.fill_color;
    }

    /// Lower the public non-rotating S2DEX object rectangle to the same typed
    /// RDP operation produced by `G_TEXRECT`. Programming Manual Chapter 25,
    /// section 4.2.3 states that this is the operation S2DEX performs in the
    /// RSP. This initial slice deliberately admits only the mode-independent,
    /// non-flipped form; object render-mode corrections remain loud.
    pub(crate) fn object_rectangle(
        &mut self,
        sprite: crate::s2dex::ObjectSprite,
    ) -> Result<RenderOp, RenderError> {
        self.object_rectangle_with_mode(sprite, crate::s2dex::ObjectRenderMode::default())
    }

    /// Object rectangle lowering with the task-local S2DEX correction mode.
    /// The typed sampler already defines integer coordinates at texel centers,
    /// so `bilerp` records that the RSP's documented half-texel correction was
    /// requested and validates it against TF without applying a second shift.
    pub(crate) fn object_rectangle_with_mode(
        &mut self,
        sprite: crate::s2dex::ObjectSprite,
        object_mode: crate::s2dex::ObjectRenderMode,
    ) -> Result<RenderOp, RenderError> {
        let reject = |reason: String| RenderError::Backend {
            backend: "reference-s2dex",
            reason,
        };
        if sprite.padding_x != 0 || sprite.padding_y != 0 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE uObjSprite padding must be zero, got paddingX={} paddingY={}",
                sprite.padding_x, sprite.padding_y
            )));
        }
        if sprite.scale_w == 0 || sprite.scale_h == 0 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE scale must be nonzero, got scaleW={} scaleH={}",
                sprite.scale_w, sprite.scale_h
            )));
        }
        if sprite.scale_w > i16::MAX as u16 || sprite.scale_h > i16::MAX as u16 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE scale exceeds the RDP signed S5.10 gradient range: scaleW={} scaleH={}",
                sprite.scale_w, sprite.scale_h
            )));
        }
        if sprite.image_w == 0
            || sprite.image_h == 0
            || !sprite.image_w.is_multiple_of(32)
            || !sprite.image_h.is_multiple_of(32)
        {
            return Err(crate::render_unsupported_error(
                "reference-s2dex",
                "render.gbi.s2dex.object-rectangle",
                format!(
                    "G_OBJ_RECTANGLE initial slice requires positive whole-texel u10.5 dimensions, got imageW={} imageH={}",
                    sprite.image_w, sprite.image_h
                ),
            ));
        }
        if sprite.image_stride == 0 || sprite.image_stride > 0x01ff {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE imageStride={} is outside the RDP tile-line range 1..=511",
                sprite.image_stride
            )));
        }
        if sprite.image_address > 0x01ff {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE imageAdrs={} exceeds the RDP tile TMEM-address range 0..=511",
                sprite.image_address
            )));
        }
        if sprite.image_format > G_IM_FMT_I || sprite.image_size > G_IM_SIZ_32B {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE texture format={} size={} is outside public G_IM_FMT/G_IM_SIZ encodings",
                sprite.image_format, sprite.image_size
            )));
        }
        if sprite.image_palette > 7 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE imagePal={} is outside the public S2DEX range 0..=7",
                sprite.image_palette
            )));
        }
        if sprite.image_flags != 0 {
            return Err(crate::render_unsupported_error(
                "reference-s2dex",
                "render.gbi.s2dex.object-rectangle",
                format!(
                    "G_OBJ_RECTANGLE imageFlags={:#04x} requests unsupported S/T flip correction",
                    sprite.image_flags
                ),
            ));
        }

        let mut state = self.begin_task();
        let width = u32::from(sprite.image_w / 32);
        let height = u32::from(sprite.image_h / 32);
        let tile = &mut state.tex.tiles[0];
        tile.fmt = sprite.image_format;
        tile.siz = sprite.image_size;
        tile.line = sprite.image_stride;
        tile.tmem = sprite.image_address;
        tile.palette = sprite.image_palette;
        tile.clamp_s = true;
        tile.clamp_t = true;
        tile.mirror_s = false;
        tile.mirror_t = false;
        tile.mask_s = 0;
        tile.mask_t = 0;
        tile.shift_s = 0;
        tile.shift_t = 0;
        tile.uls = 0;
        tile.ult = 0;
        tile.lrs = u16::try_from((width - 1) * 4).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE image width {width} exceeds tile bounds"
            ))
        })?;
        tile.lrt = u16::try_from((height - 1) * 4).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE image height {height} exceeds tile bounds"
            ))
        })?;

        let ulx = f32::from(sprite.obj_x) / 4.0;
        let uly = f32::from(sprite.obj_y) / 4.0;
        let screen_width = sprite.image_w as f32 * 32.0 / sprite.scale_w as f32;
        let screen_height = sprite.image_h as f32 * 32.0 / sprite.scale_h as f32;
        let cycle_type = state.other_mode.cycle_type();
        if cycle_type == CycleType::Fill {
            return Err(crate::render_unsupported_error(
                "reference-s2dex",
                "render.gbi.s2dex.object-rectangle",
                "G_OBJ_RECTANGLE cannot execute in Fill cycle; S2DEX supports one-cycle, two-cycle, and copy modes",
            ));
        }
        if cycle_type == CycleType::Copy && sprite.scale_w != 1 << 10 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE copy mode cannot scale X; scaleW={} must be 1024",
                sprite.scale_w
            )));
        }
        if cycle_type != CycleType::Copy {
            use crate::s2dex::{ObjectFilterCorrection, ObjectTextureClamp};
            match (
                state.other_mode.texture_filter(),
                object_mode.filter_correction,
            ) {
                (TextureFilter::Point, ObjectFilterCorrection::PointOrAverage)
                | (TextureFilter::Bilinear, ObjectFilterCorrection::Bilinear) => {}
                (TextureFilter::Average, ObjectFilterCorrection::PointOrAverage)
                    if object_mode.perimeter.is_none()
                        && object_mode.texture_clamp == ObjectTextureClamp::Perimeter => {}
                (TextureFilter::Point, ObjectFilterCorrection::Bilinear) => {
                    return Err(reject(
                        "G_OBJ_RECTANGLE G_OBJRM_BILERP is set while the RDP texture filter is Point"
                            .into(),
                    ));
                }
                (TextureFilter::Bilinear, ObjectFilterCorrection::PointOrAverage) => {
                    return Err(reject(
                        "G_OBJ_RECTANGLE Bilinear texture filter requires G_OBJRM_BILERP correction"
                            .into(),
                    ));
                }
                (TextureFilter::Average, ObjectFilterCorrection::Bilinear) => {
                    return Err(reject(
                        "G_OBJ_RECTANGLE Average texture filter does not use G_OBJRM_BILERP correction"
                            .into(),
                    ));
                }
                (TextureFilter::Average, ObjectFilterCorrection::PointOrAverage) => {
                    return Err(crate::render_unsupported_error(
                        "reference-s2dex",
                        "render.gbi.s2dex.object-rectangle",
                        "G_OBJ_RECTANGLE Average texture filter combined with perimeter correction or G_OBJRM_NOTXCLAMP requires unpublished filter-footprint arithmetic",
                    ));
                }
                (filter, _) => {
                    return Err(crate::render_unsupported_error(
                        "reference-s2dex",
                        "render.gbi.s2dex.object-rectangle",
                        format!(
                            "G_OBJ_RECTANGLE texture filter {filter:?} has no admitted S2DEX correction mode"
                        ),
                    ));
                }
            }
        } else if object_mode.filter_correction == crate::s2dex::ObjectFilterCorrection::Bilinear {
            return Err(reject(
                "G_OBJ_RECTANGLE Copy cycle does not support G_OBJRM_BILERP".into(),
            ));
        }
        let inclusive = cycle_type == CycleType::Copy;
        let storage = state.tex.tmem.clone();
        let texture_lut = state.other_mode.texture_lut();
        let rectangle = TextureRectangle {
            ulx,
            uly,
            lrx: ulx + screen_width - if inclusive { 1.0 } else { 0.0 },
            lry: uly + screen_height - if inclusive { 1.0 } else { 0.0 },
            tile: 0,
            s: 0.0,
            t: 0.0,
            dsdx: if inclusive {
                4 << 10
            } else {
                sprite.scale_w as i16
            },
            dtdy: sprite.scale_h as i16,
            flip: false,
            other_mode: state.other_mode,
            combiner: state.combiner,
            blender: active_blender(&state),
            scissor: state.scissor,
            texture: texture_for_tile(&state.tex, 0, texture_lut, &storage),
            texture1: texture_for_tile(&state.tex, 1, texture_lut, &storage),
            fill_color: state.fill_color,
        };
        self.commit_task(&state);
        Ok(RenderOp::TextureRectangle(rectangle))
    }
}

/// One RDP tile descriptor (`G_SETTILE` + `G_SETTILESIZE`,
/// `F3DEX2-CONCEPTS.md` §5.1) -- only the fields the reference sampler needs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Tile {
    pub(super) fmt: u8,
    pub(super) siz: u8,
    /// Line stride in 64-bit words (`G_SETTILE` `line`).
    pub(super) line: u16,
    /// Base address in 64-bit TMEM words (`G_SETTILE` `tmem`).
    pub(super) tmem: u16,
    /// TLUT palette bank (CI4 uses this as the high nibble of the index).
    pub(super) palette: u8,
    pub(super) clamp_s: bool,
    pub(super) clamp_t: bool,
    pub(super) mirror_s: bool,
    pub(super) mirror_t: bool,
    pub(super) mask_s: u8,
    pub(super) mask_t: u8,
    pub(super) shift_s: u8,
    pub(super) shift_t: u8,
    /// Tile active extent from `G_SETTILESIZE` (10.2 -> ÷4 texels).
    pub(super) uls: u16,
    pub(super) ult: u16,
    pub(super) lrs: u16,
    pub(super) lrt: u16,
}

/// Apply the public `G_SETTILE` wire fields without disturbing the extent
/// owned by `G_SETTILESIZE`/load commands.
pub(super) fn apply_set_tile(tile: &mut Tile, w0: u32, w1: u32) {
    tile.fmt = ((w0 >> 21) & 0x07) as u8;
    tile.siz = ((w0 >> 19) & 0x03) as u8;
    tile.line = ((w0 >> 9) & 0x01ff) as u16;
    tile.tmem = (w0 & 0x01ff) as u16;
    tile.palette = ((w1 >> 20) & 0x0f) as u8;
    let cm_t = ((w1 >> 18) & 0x03) as u8;
    tile.mask_t = ((w1 >> 14) & 0x0f) as u8;
    tile.shift_t = ((w1 >> 10) & 0x0f) as u8;
    let cm_s = ((w1 >> 8) & 0x03) as u8;
    tile.mask_s = ((w1 >> 4) & 0x0f) as u8;
    tile.shift_s = (w1 & 0x0f) as u8;
    tile.clamp_s = cm_s & 0x02 != 0;
    tile.clamp_t = cm_t & 0x02 != 0;
    tile.mirror_s = cm_s & 0x01 != 0;
    tile.mirror_t = cm_t & 0x01 != 0;
}

/// Parsed viewport: screen scale/translate in pixels (x, y) plus a depth
/// scale/translate (z), all already ÷4 from the N64 quarter-pixel encoding
/// (`F3DEX2-CONCEPTS.md` §3.5).
#[derive(Copy, Clone, Debug)]
pub(super) struct Viewport {
    pub(super) sx: f32,
    pub(super) sy: f32,
    pub(super) sz: f32,
    pub(super) tx: f32,
    pub(super) ty: f32,
    pub(super) tz: f32,
}

/// Max `G_DL` *call* (G_DL_PUSH) recursion depth honored, matching the real
/// F3DEX2 display-list return stack (18 entries; the older 10-entry figure
/// is F3D/F3DEX). Only pushes count -- a gsSPBranchList tail-jump replaces
/// the DL pointer and consumes NO stack entry on hardware, so branch chains
/// (which OoT uses liberally) must not count against this.
pub(super) const MAX_DL_DEPTH: u32 = 18;

/// Whole-decode command budget: bounds a cyclic/corrupt DL (e.g. a branch
/// list that branches to itself), which the hardware would spin on forever.
/// A real OoT frame decodes on the order of 10^4 commands; 2^20 is far above
/// any legitimate frame while still terminating promptly on a cycle.
pub(super) const MAX_DL_COMMANDS: u32 = 1 << 20;
pub(super) const MAX_GEOMETRY_VERTEX_CACHE: usize = 128;
