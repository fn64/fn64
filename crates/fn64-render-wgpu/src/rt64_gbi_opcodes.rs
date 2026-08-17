//! The F3D, F3DEX, S2DEX and S2DEX2 microcode opcode and moveword-offset
//! constant tables: a literal port of the permitted MIT RT64 source pinned at
//! commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! ## Cited sources and their whole-file digests
//!
//! Every digest below was computed independently here with `shasum -a 256`
//! against the pinned checkout at `/private/tmp/fn64-rt64-port-source`, then
//! cross-checked verbatim against `docs/rt64-port-inventory.json`'s
//! `files[path=...].sources.port.sha256`. **All thirteen agree -- no
//! mismatch.** (For all thirteen the inventory's `sources.oracle.sha256`
//! records the identical digest as well, so the oracle and port trees agree
//! on each of these files byte for byte; the oracle tree itself was not
//! read.)
//!
//! Line counts are the inventory's, which count a final unterminated line as
//! a line; `wc -l` reports one less for each of the thirteen.
//!
//! | Source | Lines | SHA-256 | Drift |
//! |---|---|---|---|
//! | `src/gbi/rt64_gbi_f3d.h` | 88 | `3beabfea86ec53de91b5a193a65afee43ebd01b9060437ed0ff8d67701b5e370` | partial: 51/88 lines |
//! | `src/gbi/rt64_gbi_f3dex.h` | 27 | `0e3db76ade3f79933e4a846ea930358df91658def0f8cb49e7bf4e96eb271a5b` | partial: 4/27 lines |
//! | `src/gbi/rt64_gbi_s2dex.h` | 114 | `8a90add3616c83329660bc1a1c37c3a472264f4d06e06d36f9d7f314daf10730` | partial: 20/114 lines |
//! | `src/gbi/rt64_gbi_s2dex2.h` | 24 | `0e78cc4852daf1ba22147c9d52100ebbf370116ccf81da670ae799d4e33b6770` | partial: 9/24 lines |
//! | `src/gbi/rt64_gbi.h` | 77 | `38b849582be5f674c6e9522433a0b5488ebda53d639837ffd73ede31efeeb6da` | partial: 1/77 lines |
//! | `src/gbi/rt64_f3d.h` | 47 | `f052ca77ad7ff52d1c41f146c36c93d4d35522c649f90779bbb9a2f596694294` | cited but not ported |
//! | `src/gbi/rt64_gbi_l3dex2.h` | 17 | `3365ec9ed352236ec5583b04733413f39ca17988019a701e81f486c02540d72d` | cited but not ported |
//! | `src/gbi/rt64_gbi_l3dex2.cpp` | 21 | `bdccc51474cfe2ae32a3a1e237d168aa53832610bef18c3f1d334fb92d4bb063` | cited but not ported |
//! | `src/preset/rt64_preset_inspector.h` | 173 | `3a8637b58e2e5d768980c8889bcf6536c5ea58af11f389230914b7570351c196` | cited but not ported |
//! | `src/rhi/rt64_render_hooks.h` | 19 | `45d14218a4949ffd4913c1e28f765992c067bfb185196efdb0f12a6f2bdf4fb7` | cited but not ported |
//! | `src/rhi/rt64_render_hooks.cpp` | 29 | `3363fe4d12078722b97b2ce6bea219dbfc0c0cd2a44c9c494a2fd8977a1073dc` | cited but not ported |
//! | `src/apple/rt64_apple.h` | 7 | `db4d74b578df718d92766a9f9a9f1d10754473c9c3396a1d54b19d528e7cdbe9` | cited but not ported |
//! | `src/apple/rt64_apple.mm` | 8 | `3d9036c4c956dcb78e9702c1cbaf4e03960bebfd2af64297e393d7eb234f9bc8` | cited but not ported |
//!
//! A digest citation marks the whole file `ported` in the burndown, which is
//! **file**-granular: the four partially-ported headers are credited in full
//! by that mechanism even though only their `#define` blocks are carried
//! here. The per-file fractions above are the honest accounting; the
//! over-credit is recorded, not hidden.
//!
//! `docs/rt64-port-inventory.json`'s `ported_as` for all thirteen paths is
//! still `[]` and does not yet name this module, so
//! `scripts/lint-docs.py`'s mechanical SHA-256 citation scanner reports a
//! `ported_as` drift. That checker is **fail-fast** -- it names one path per
//! run -- so the single line it prints
//! (`src/apple/rt64_apple.h: ported_as drift`) is the first of thirteen, not
//! a thirteenth separate problem; this was confirmed by running
//! `tools/rt64_port_inventory.py`'s `ported_as_for` over all thirteen
//! directly, which expects this module's path for every one of them. The fix
//! is one `docs: regenerate inventory` commit (the established pattern -- see
//! commits `67654533`, `6adaf537`, `1b4df109`, the first of which records the
//! identical fail-fast-checker diagnosis). `docs/rt64-port-inventory.json` is
//! **not** in this card's writable surface, and three sibling lanes are
//! adding modules concurrently, so regenerating from here would race their
//! entries; the reconciliation is deliberately left to the owning ticket.
//!
//! ## What is ported, and why it was missing
//!
//! The `.cpp` bodies of `GBI_F3D`, `GBI_F3DEX`, `GBI_S2DEX2` and the F3D
//! variants are already ported, as `rt64_gbi_f3d.rs` (1452 lines),
//! `rt64_gbi_f3dex.rs` (928), `rt64_gbi_s2dex2.rs` (672) and
//! `rt64_gbi_f3d_variants.rs` (1217). Those modules port **bitfield decode
//! only**: `rt64_gbi_f3d.rs` and `rt64_gbi_f3dex.rs` between them define
//! **zero** constants of any kind (verified: `grep -cE '^\s*(pub )?const '`
//! returns `0` for both), and reference the `F3D_G_*` names only inside `//!`
//! doc quotations of the C++ they decode. The opcode and moveword-offset
//! `#define` tables in the corresponding **headers** were therefore genuinely
//! absent from the crate. This module supplies exactly those tables and
//! nothing else.
//!
//! **Ported** -- the five `#define` blocks, 85 constants total:
//! - `rt64_gbi_f3d.h:9-59`, 51 defines: `F3D_G_MW_POINTS`, the 14
//!   `F3D_G_MWO_{a,b}LIGHT_n`, the 20 F3D opcodes, and the 16 `F3D_G_MV_*`
//!   movemem indices.
//! - `rt64_gbi_f3dex.h:9-12`, 4 defines: the F3DEX opcodes.
//! - `rt64_gbi_s2dex.h:9-28`, 20 defines: 9 S2DEX opcodes, 2 `BGLT_` tile
//!   descriptors, 2 `BG_FLAG_` bits, 7 `OBJRM_` render-mode bits.
//! - `rt64_gbi_s2dex2.h:9-17`, 9 defines: the S2DEX2 opcodes. One of the
//!   nine, `S2DEX2_G_SELECT_DL`, is **not redefined here** -- see "Reuse".
//! - `rt64_gbi.h:12`, 1 define: `UCODE_MAP_SIZE`.
//!
//! **Refused**, per source, with the deciding evidence:
//! - `rt64_gbi_f3d.h:61-88`, `rt64_gbi_f3dex.h:14-27`,
//!   `rt64_gbi_s2dex.h:30-114`, `rt64_gbi_s2dex2.h:20-24` -- the `namespace
//!   RT64 { namespace GBI_* { ... } }` blocks. These are **forward
//!   declarations** of `void f(State *, DisplayList **)` handlers and, in
//!   `rt64_gbi_s2dex.h`, the `#pragma pack(1)` `uObjBg`/`uObjTxtr` structs.
//!   The handler bodies live in the already-ported `.cpp` files; a
//!   declaration with no body has no behavior to port. The packed structs are
//!   a byte-layout claim over RDRAM this card explicitly may not make (see
//!   "Nonclaims").
//! - `src/gbi/rt64_f3d.h` (whole file) -- `enum class F3DENUM` (11 symbolic
//!   names with no assigned values, used only as `std::unordered_map` keys in
//!   `GBI::constants`) plus the `Vp_t` and `OSTask_t` **layout** structs.
//!   `F3DENUM` carries no hardware value: its members are compiler-assigned
//!   ordinals, and RT64 populates `gbi->constants[F3DENUM::X] = <ucode value>`
//!   at `setup` time, so the enum is a lookup key, not a fact. `Vp_t` and
//!   `OSTask_t` are `repr(C)` memory layouts over RDRAM -- refused for the
//!   same reason as `uObjBg`.
//! - `src/gbi/rt64_gbi.h:14-77` -- `GBIUCode`, `GBIFlags`, `GBIInstance`,
//!   `GBISegment`, `GBI`, `GBIManager`. `GBI` holds a
//!   `GBIFunction map[UCODE_MAP_SIZE]` of raw C function pointers and an
//!   `unordered_map<F3DENUM, uint32_t>`; `GBIManager` walks RDRAM to deduce a
//!   microcode. All of it needs the `State`/`RSP` object graph and RDRAM
//!   access, out of scope. Only the array **bound** `UCODE_MAP_SIZE` is a
//!   standalone fact, and that is ported.
//! - `src/gbi/rt64_gbi_l3dex2.{h,cpp}` (whole files) -- `L3DEX2_G_LINE3D
//!   0x08` and `GBI_L3DEX2::{line3D, setup}`. **The existing deliberate
//!   exclusion still holds**: `rt64_gbi_f3d_variants.rs:39-43` states these
//!   two files are "DELIBERATELY EXCLUDED from this module: its only
//!   function, `line3D`, is a bare `assert(false);` with no bitfield read at
//!   all", restated at `:428-430`. That text is present verbatim in the
//!   worktree today. Two further reasons apply here specifically: `setup`'s
//!   body is `GBI_F3DEX2::setup(gbi); gbi->map[L3DEX2_G_LINE3D] = &line3D;`
//!   -- pure dispatch-table wiring, which needs `GBI` (refused above); and
//!   the constant's value `0x08` is **already defined** for this exact
//!   opcode -- see "Reuse".
//! - `src/preset/rt64_preset_inspector.h` (whole file) -- a single
//!   `template <class B, class L, ...> struct PresetLibraryInspector` whose
//!   three methods are ImGui immediate-mode UI: `ImGui::Checkbox`,
//!   `ImGui::Button`, `ImGui::BeginPopupModal`, `ImGui::InputText`,
//!   `ImGui::GetWindowSize`, plus `FileDialog::getOpenFilename` /
//!   `getSaveFilename` and a `RenderWindow` handle. Every one of its returned
//!   `bool`s is the return of an ImGui widget call or a flag set by a button
//!   press; there is no value in the file that is not a UI event. Three
//!   landed preset modules (`rt64_preset_material.rs`,
//!   `rt64_preset_light.rs`, `rt64_preset_scene.rs`) each already record this
//!   header as "not a cited source here" for the same reason. Refused: no
//!   ImGui context, no file dialog, and no GPU exist in this crate's test
//!   surface, and the file contains no hardware fact.
//! - `src/rhi/rt64_render_hooks.{h,cpp}` (whole files) -- three
//!   `using RenderHook* = void(...)` function-pointer typedefs over
//!   `RenderInterface`/`RenderDevice`/`RenderCommandList`/`RenderFramebuffer`
//!   (all from the uncited `common/rt64_plume.h`), three file-scope
//!   `static ... *` globals, three getters and one setter. The behavior is
//!   *"assign three mutable process-global raw function pointers"*. Refused:
//!   the four RHI types are uncited and are live GPU objects; and mutable
//!   global function-pointer state is not a hardware fact -- porting it would
//!   mean inventing a Rust ownership model upstream does not have.
//! - `src/apple/rt64_apple.{h,mm}` (whole files) -- `const char*
//!   GetHomeDirectory()`, whose body is
//!   `return strdup([NSHomeDirectory() UTF8String]);`. Refused on three
//!   independent grounds: the `.mm` is **Objective-C++**, outside the
//!   clean-room Rust port entirely; the behavior is an AppKit/Foundation
//!   platform query with no N64 content; and it is a deliberate leak
//!   (`strdup` with no matching `free` at any call site in the file), which
//!   this port would not reproduce anyway.
//!
//! ## Verbatim key logic
//!
//! ```text
//! // src/gbi/rt64_gbi_f3d.h lines 9-59
//! #define F3D_G_MW_POINTS 0x0c
//! #define F3D_G_MWO_aLIGHT_2 0x20
//! #define F3D_G_MWO_bLIGHT_2 0x24
//! #define F3D_G_MWO_aLIGHT_3 0x40
//! #define F3D_G_MWO_bLIGHT_3 0x44
//! #define F3D_G_MWO_aLIGHT_4 0x60
//! #define F3D_G_MWO_bLIGHT_4 0x64
//! #define F3D_G_MWO_aLIGHT_5 0x80
//! #define F3D_G_MWO_bLIGHT_5 0x84
//! #define F3D_G_MWO_aLIGHT_6 0xa0
//! #define F3D_G_MWO_bLIGHT_6 0xa4
//! #define F3D_G_MWO_aLIGHT_7 0xc0
//! #define F3D_G_MWO_bLIGHT_7 0xc4
//! #define F3D_G_MWO_aLIGHT_8 0xe0
//! #define F3D_G_MWO_bLIGHT_8 0xe4
//! #define F3D_G_NOOP 0xc0
//! #define F3D_G_SETOTHERMODE_H 0xBA
//! #define F3D_G_SETOTHERMODE_L 0xB9
//! #define F3D_G_RDPHALF_1 0xB4
//! #define F3D_G_RDPHALF_2 0xB3
//! #define F3D_G_SPNOOP 0x00
//! #define F3D_G_ENDDL 0xB8
//! #define F3D_G_DL 0x06
//! #define F3D_G_MOVEMEM 0x03
//! #define F3D_G_MOVEWORD 0xBC
//! #define F3D_G_MTX 0x01
//! #define F3D_G_POPMTX 0xBD
//! #define F3D_G_TEXTURE 0xBB
//! #define F3D_G_VTX 0x04
//! #define F3D_G_CULLDL 0xBE
//! #define F3D_G_TRI1 0xBF
//! #define F3D_G_QUAD 0xB5
//! #define F3D_G_SPRITE2D_BASE 0x09
//! #define F3D_G_SETGEOMETRYMODE 0xB7
//! #define F3D_G_CLEARGEOMETRYMODE 0xB6
//! #define F3D_G_MV_VIEWPORT 0x80
//! #define F3D_G_MV_LOOKATY 0x82
//! #define F3D_G_MV_LOOKATX 0x84
//! #define F3D_G_MV_L0 0x86
//! #define F3D_G_MV_L1 0x88
//! #define F3D_G_MV_L2 0x8a
//! #define F3D_G_MV_L3 0x8c
//! #define F3D_G_MV_L4 0x8e
//! #define F3D_G_MV_L5 0x90
//! #define F3D_G_MV_L6 0x92
//! #define F3D_G_MV_L7 0x94
//! #define F3D_G_MV_TXTATT 0x96
//! #define F3D_G_MV_MATRIX_1 0x9e
//! #define F3D_G_MV_MATRIX_2 0x98
//! #define F3D_G_MV_MATRIX_3 0x9a
//! #define F3D_G_MV_MATRIX_4 0x9c
//!
//! // src/gbi/rt64_gbi_f3dex.h lines 9-12
//! #define F3DEX_G_LOAD_UCODE 0xAF
//! #define F3DEX_G_MODIFYVTX 0xB2
//! #define F3DEX_G_BRANCH_Z 0xB0
//! #define F3DEX_G_TRI2 0xB1
//!
//! // src/gbi/rt64_gbi_s2dex.h lines 9-28
//! #define S2DEX_G_BG_1CYC 0x01
//! #define S2DEX_G_BG_COPY 0x02
//! #define S2DEX_G_RDPHALF_0 0xE4
//! #define S2DEX_G_SELECT_DL 0xB0
//! #define S2DEX_G_OBJ_LOADTXTR 0xC1
//! #define S2DEX_G_OBJ_LDTX_SPRITE 0xC2
//! #define S2DEX_G_OBJ_LDTX_RECT 0xC3
//! #define S2DEX_G_OBJ_LDTX_RECT_R 0xC4
//! #define S2DEX_G_OBJ_RENDERMODE 0xB1
//! #define S2DEX_G_BGLT_LOADBLOCK 0x0033
//! #define S2DEX_G_BGLT_LOADTILE 0xFFF4
//! #define S2DEX_G_BG_FLAG_FLIPS 0x01
//! #define S2DEX_G_BG_FLAG_FLIPT 0x10
//! #define S2DEX_G_OBJRM_NOTXCLAMP 0x01
//! #define S2DEX_G_OBJRM_XLU 0x02
//! #define S2DEX_G_OBJRM_ANTIALIAS 0x04
//! #define S2DEX_G_OBJRM_BILERP 0x08
//! #define S2DEX_G_OBJRM_SHRINKSIZE_1 0x10
//! #define S2DEX_G_OBJRM_SHRINKSIZE_2 0x20
//! #define S2DEX_G_OBJRM_WIDEN 0x40
//!
//! // src/gbi/rt64_gbi_s2dex2.h lines 9-17
//! #define S2DEX2_G_OBJ_RENDERMODE 0x0B
//! #define S2DEX2_G_BG_1CYC 0x09
//! #define S2DEX2_G_BG_COPY 0x0A
//! #define S2DEX2_G_RDPHALF_0 0xE4
//! #define S2DEX2_G_SELECT_DL 0x04
//! #define S2DEX2_G_OBJ_LOADTXTR 0x05
//! #define S2DEX2_G_OBJ_LDTX_SPRITE 0x06
//! #define S2DEX2_G_OBJ_LDTX_RECT 0x07
//! #define S2DEX2_G_OBJ_LDTX_RECT_R 0x08
//!
//! // src/gbi/rt64_gbi.h line 12
//! #define UCODE_MAP_SIZE 256
//!
//! // src/gbi/rt64_gbi_l3dex2.h line 9 -- CITED, REFUSED (already defined,
//! // see "Reuse")
//! #define L3DEX2_G_LINE3D 0x08
//! ```
//!
//! ## Reuse, not new type
//!
//! No new type is introduced by this module at all. Every item is a bare
//! `pub const` of a primitive integer type: `u8` for a byte that upstream
//! compares against a display-list opcode or a `p0`-extracted index, `u16`
//! for the two `S2DEX_G_BGLT_*` values that exceed a byte, and `usize` for
//! `UCODE_MAP_SIZE`, which upstream uses only as an array bound. Grouping is
//! by **source file and source declaration order**, never by inferred
//! meaning; no constant is renamed and no hex literal is reformatted
//! (upstream's mixed case -- `0xBA` but `0x8a`, `0xFFF4` but `0xe4` -- is
//! carried through exactly).
//!
//! Two constants from the cited headers are **already defined in this crate
//! and are deliberately not redefined here**:
//!
//! - **`S2DEX2_G_SELECT_DL = 0x04`** (`rt64_gbi_s2dex2.h:13`) is already
//!   `pub const S2DEX2_G_SELECT_DL: u8 = 0x04` at
//!   `crates/fn64-render-wgpu/src/rt64_gbi_s2dex2.rs:296`, citing that exact
//!   source line. This module defines nothing for it, so the crate keeps a
//!   single definition; the tests reach for that definition by its full path
//!   and assert it against the literal `0x04`, so a change on either side is
//!   caught.
//! - **`L3DEX2_G_LINE3D = 0x08`** (`rt64_gbi_l3dex2.h:9`) is already
//!   `pub(super) const G_LINE3D: u8 = 0x08` at
//!   `crates/fn64-render-reference/src/gbi/wire.rs:720`. It is
//!   crate-private there, so it cannot be re-exported across the crate
//!   boundary; combined with the standing `rt64_gbi_f3d_variants.rs`
//!   exclusion (see "Refused"), this module defines nothing for L3DEX2.
//!
//! Nothing else collides. In particular, `crates/fn64-render-reference/src/
//! gbi/wire.rs` does carry a legacy F3D-family opcode block (`L3DEX_G_MTX`
//! `0x01` .. `L3DEX_G_NOOP` `0xc0`, `F3DEX_G_LOAD_UCODE` `0xaf` ..
//! `F3DEX_G_MODIFYVTX` `0xb2`) whose **values** overlap the F3D and F3DEX
//! tables here -- so the earlier report that that crate holds "only the
//! F3DEX2 opcode set" is **refuted**. Those constants are `pub(super)`,
//! visible only inside `fn64-render-reference`'s `gbi` module, and are named
//! for the L3DEX/legacy dispatch that crate decodes rather than for RT64's
//! `F3D_G_*` headers; `crates/fn64-render/src/geometry_task_inspection/mod.rs
//! :61-88` holds a third, file-private (`const`, not `pub`) copy of the same
//! legacy block. Neither is reachable from `fn64-render-wgpu`, and neither
//! cites the RT64 headers this module cites, so redefining the RT64 names
//! here is not a duplication of a reachable definition. The value agreement
//! between the three is itself checked below as an independent cross-check
//! (the F3D high-opcode run and `F3DEX_G_*` are asserted against
//! hand-derived literals, and those literals match what those two crates
//! independently record).
//!
//! ## Admitted domain
//!
//! - **`F3D_G_MWO_{a,b}LIGHT_1` are absent from the cited header and are not
//!   supplied here.** The table starts at `_2`. `G_MWO_aLIGHT_1 0x00` and
//!   `G_MWO_bLIGHT_1 0x04` are defined in `src/shared/rt64_f3d_defines.h:138-
//!   139`, which is **not** a source cited by this card, so they are not
//!   ported. The derived rule below (`aLIGHT_n == 0x20 * (n - 1)`) does
//!   extend to `n = 1` giving `0x00`, which is consistent with that uncited
//!   file, but the extension is recorded as an observation, not asserted as
//!   a ported constant.
//! - **The `{a,b}LIGHT` regularity holds across the whole run, with no
//!   break.** Checked two independent ways and reconciled: (1) the 14
//!   literals as written in the header, and (2) the closed form
//!   `aLIGHT_n = 0x20 * (n - 1)`, `bLIGHT_n = aLIGHT_n + 4`, for
//!   `n` in `2..=8`. Both agree on all 14 values. A third check --
//!   successive differences -- gives a uniform `0x20` step for both the `a`
//!   and the `b` sequence across all six adjacent pairs. No break to pin.
//! - **`F3D_G_MWO_aLIGHT_7` is `0xc0`, numerically equal to `F3D_G_NOOP`
//!   `0xc0`.** This is **not** a within-namespace duplicate and is not
//!   treated as one: `aLIGHT_7` is a *moveword byte offset* into the RSP
//!   light block, while `NOOP` is a *display-list opcode byte*. They are
//!   compared against different extractions of different words and can never
//!   be confused by any dispatch. Similarly `aLIGHT_5` `0x80` equals
//!   `F3D_G_MV_VIEWPORT` `0x80`, and `bLIGHT_8` `0xe4` equals
//!   `S2DEX_G_RDPHALF_0` / `S2DEX2_G_RDPHALF_0` `0xE4`. All three
//!   coincidences are cross-namespace. **Within** each of the four opcode
//!   namespaces there are zero duplicate values -- verified exhaustively in
//!   [`tests`], which is the check the card asked for.
//! - **`F3D_G_MV_MATRIX_1` is declared out of numeric order (pinned, not
//!   fixed).** The 16 `F3D_G_MV_*` values are exactly `0x80, 0x82, ..., 0x9e`
//!   -- a complete step-2 run over `0x80..=0x9e` with no gap and no
//!   duplicate. But the header declares `MATRIX_1 0x9e` **before**
//!   `MATRIX_2 0x98`, `MATRIX_3 0x9a`, `MATRIX_4 0x9c`, so in declaration
//!   order the successive differences are twelve `+2`s, then `+8`, then
//!   `-6`, then `+2`, `+2`. That is genuine upstream: `MATRIX_1` is the
//!   run's numeric maximum, not its minimum, and the numbering does not
//!   follow the declaration. Both orders are asserted separately below --
//!   declaration order as a positional array, numeric order as a sorted
//!   step-2 run -- so neither can be silently "tidied" into the other.
//! - **`S2DEX_G_BG_FLAG_FLIPT` skips three bit positions (pinned, not
//!   fixed).** `FLIPS` is `0x01` (bit 0) and `FLIPT` is `0x10` (bit 4). Bits
//!   1, 2 and 3 are unclaimed between them. "Correcting" `FLIPT` to `0x02`
//!   would change which byte in the `uObjBg` flip word selects the T-axis
//!   flip, so the gap is asserted explicitly.
//! - **`S2DEX_G_OBJRM_*` is a complete low-7-bit power-of-two run.** Checked
//!   two independent ways: the 7 literals as written, and `1 << i` for
//!   `i` in `0..=6`. Both agree; the union is `0x7f` and bit 7 (`0x80`) is
//!   unclaimed by any macro in the file. That the run is complete with no
//!   skipped bit is itself asserted, because the sibling `BG_FLAG` pair
//!   above shows this header is willing to skip bits.
//! - **`S2DEX_G_BGLT_LOADBLOCK 0x0033` and `S2DEX_G_BGLT_LOADTILE 0xFFF4`
//!   are not opcodes and are typed `u16`.** They exceed a byte and upstream
//!   writes them with a four-digit width, so they are carried as `u16`. What
//!   they select in the RDP tile-load path is not determined by this header;
//!   only the values are claimed.
//! - **`UCODE_MAP_SIZE` is typed `usize`, not `u32`.** Upstream's only use
//!   is the array bound `GBIFunction map[UCODE_MAP_SIZE]`
//!   (`rt64_gbi.h:62`), and Rust array bounds are `usize`. The value `256`
//!   is also exactly `u8::MAX as usize + 1`, i.e. the map is indexed by a
//!   full opcode byte and every one of the 85 ported opcode constants is in
//!   range by construction; that relationship is asserted below.
//!
//! ## Nonclaims
//!
//! - **No claim about memory layout, size, alignment, `repr(C)`, or ABI.**
//!   Nothing here is a struct. The `#pragma pack(push,1)` `uObjBg_t`,
//!   `uObjScaleBg_t`, `uObjBg`, `uObjTxtr` and `uObjTxSprite` structs at
//!   `rt64_gbi_s2dex.h:33-100`, and `Vp_t` / `OSTask_t` at
//!   `rt64_f3d.h:25-47`, are explicitly **not** ported: reproducing them
//!   would be a byte-offset claim over RDRAM, which this card forbids.
//! - **No claim that any constant here is *dispatched on*.** This module
//!   defines values; it wires nothing. `GBI::map` -- the 256-entry function
//!   pointer table these opcodes index -- is refused (see "Refused"), so no
//!   opcode-to-handler association is asserted, in either direction.
//! - **No claim about the *meaning* of any value on real hardware** beyond
//!   what the cited headers state. In particular no claim about which
//!   microcode revisions accept which opcode, about `S2DEX_G_BGLT_*`'s role
//!   in the tile-load path, or about `F3D_G_SPRITE2D_BASE`'s operand layout.
//! - **No claim about the gaps in the F3D low-opcode block.** The low F3D
//!   opcodes present are `0x00, 0x01, 0x03, 0x04, 0x06, 0x09`; `0x02`,
//!   `0x05`, `0x07` and `0x08` are absent from the header. The tests assert
//!   *that they are absent from this table*, which is a fact about the
//!   header, and make no claim that those encodings are unused by the
//!   hardware -- `L3DEX2_G_LINE3D` is `0x08`, which shows at least one of
//!   them is used elsewhere.
//! - **No claim of completeness for any microcode.** Each table is exactly
//!   the `#define`s in one header. F3D opcodes reaching the RDP (`0xE4`
//!   and up) are defined in the uncited `rt64_f3d_defines.h`, not here.
//! - **No UB was found in any cited source, and no DEVIATION is taken.**
//!   The five ported blocks are integer-literal `#define`s with no
//!   arithmetic, no cast, no indexing and no pointer use. Every test below
//!   pins upstream's values as written; none repairs anything. (The one UB-
//!   adjacent construct seen anywhere in the thirteen files is
//!   `rt64_apple.mm:7`'s unmatched `strdup` -- a leak, not UB -- and that
//!   file is refused outright, so no deviation arises from it.)
//! - **No claim about declaration order as a language-level guarantee.**
//!   The order facts asserted below are asserted through **positional
//!   arrays**, which is the one form that is pinnable in safe Rust; no
//!   struct field order is claimed anywhere (cf. `rt64_shared_params.rs:255`).

// `S2DEX2_G_SELECT_DL` (`rt64_gbi_s2dex2.h:13`) is deliberately NOT defined
// here: `crates/fn64-render-wgpu/src/rt64_gbi_s2dex2.rs:296` already defines
// it, citing that same source line. Consumers should use that one. The tests
// below reach for it by its full path and assert it against the literal
// `0x04`, so a change on either side is caught, without this module either
// shadowing it or adding an import the non-test build would not use.

// ---------------------------------------------------------------------------
// src/gbi/rt64_gbi.h
// ---------------------------------------------------------------------------

/// `UCODE_MAP_SIZE` (`rt64_gbi.h:12`). Upstream's sole use is the array bound
/// `GBIFunction map[UCODE_MAP_SIZE]` (`rt64_gbi.h:62`), hence `usize`.
pub const UCODE_MAP_SIZE: usize = 256;

// ---------------------------------------------------------------------------
// src/gbi/rt64_gbi_f3d.h -- moveword offsets, declaration order
// ---------------------------------------------------------------------------

/// `F3D_G_MW_POINTS` (`rt64_gbi_f3d.h:9`).
pub const F3D_G_MW_POINTS: u8 = 0x0c;

/// `F3D_G_MWO_aLIGHT_2` (`rt64_gbi_f3d.h:10`).
pub const F3D_G_MWO_A_LIGHT_2: u8 = 0x20;
/// `F3D_G_MWO_bLIGHT_2` (`rt64_gbi_f3d.h:11`).
pub const F3D_G_MWO_B_LIGHT_2: u8 = 0x24;
/// `F3D_G_MWO_aLIGHT_3` (`rt64_gbi_f3d.h:12`).
pub const F3D_G_MWO_A_LIGHT_3: u8 = 0x40;
/// `F3D_G_MWO_bLIGHT_3` (`rt64_gbi_f3d.h:13`).
pub const F3D_G_MWO_B_LIGHT_3: u8 = 0x44;
/// `F3D_G_MWO_aLIGHT_4` (`rt64_gbi_f3d.h:14`).
pub const F3D_G_MWO_A_LIGHT_4: u8 = 0x60;
/// `F3D_G_MWO_bLIGHT_4` (`rt64_gbi_f3d.h:15`).
pub const F3D_G_MWO_B_LIGHT_4: u8 = 0x64;
/// `F3D_G_MWO_aLIGHT_5` (`rt64_gbi_f3d.h:16`).
pub const F3D_G_MWO_A_LIGHT_5: u8 = 0x80;
/// `F3D_G_MWO_bLIGHT_5` (`rt64_gbi_f3d.h:17`).
pub const F3D_G_MWO_B_LIGHT_5: u8 = 0x84;
/// `F3D_G_MWO_aLIGHT_6` (`rt64_gbi_f3d.h:18`).
pub const F3D_G_MWO_A_LIGHT_6: u8 = 0xa0;
/// `F3D_G_MWO_bLIGHT_6` (`rt64_gbi_f3d.h:19`).
pub const F3D_G_MWO_B_LIGHT_6: u8 = 0xa4;
/// `F3D_G_MWO_aLIGHT_7` (`rt64_gbi_f3d.h:20`). Numerically equal to
/// [`F3D_G_NOOP`]; a cross-namespace coincidence, not a duplicate (see the
/// module doc's "Admitted domain").
pub const F3D_G_MWO_A_LIGHT_7: u8 = 0xc0;
/// `F3D_G_MWO_bLIGHT_7` (`rt64_gbi_f3d.h:21`).
pub const F3D_G_MWO_B_LIGHT_7: u8 = 0xc4;
/// `F3D_G_MWO_aLIGHT_8` (`rt64_gbi_f3d.h:22`).
pub const F3D_G_MWO_A_LIGHT_8: u8 = 0xe0;
/// `F3D_G_MWO_bLIGHT_8` (`rt64_gbi_f3d.h:23`).
pub const F3D_G_MWO_B_LIGHT_8: u8 = 0xe4;

// ---------------------------------------------------------------------------
// src/gbi/rt64_gbi_f3d.h -- opcodes, declaration order
// ---------------------------------------------------------------------------

/// `F3D_G_NOOP` (`rt64_gbi_f3d.h:24`).
pub const F3D_G_NOOP: u8 = 0xc0;
/// `F3D_G_SETOTHERMODE_H` (`rt64_gbi_f3d.h:25`).
pub const F3D_G_SETOTHERMODE_H: u8 = 0xBA;
/// `F3D_G_SETOTHERMODE_L` (`rt64_gbi_f3d.h:26`).
pub const F3D_G_SETOTHERMODE_L: u8 = 0xB9;
/// `F3D_G_RDPHALF_1` (`rt64_gbi_f3d.h:27`).
pub const F3D_G_RDPHALF_1: u8 = 0xB4;
/// `F3D_G_RDPHALF_2` (`rt64_gbi_f3d.h:28`).
pub const F3D_G_RDPHALF_2: u8 = 0xB3;
/// `F3D_G_SPNOOP` (`rt64_gbi_f3d.h:29`).
pub const F3D_G_SPNOOP: u8 = 0x00;
/// `F3D_G_ENDDL` (`rt64_gbi_f3d.h:30`).
pub const F3D_G_ENDDL: u8 = 0xB8;
/// `F3D_G_DL` (`rt64_gbi_f3d.h:31`).
pub const F3D_G_DL: u8 = 0x06;
/// `F3D_G_MOVEMEM` (`rt64_gbi_f3d.h:32`).
pub const F3D_G_MOVEMEM: u8 = 0x03;
/// `F3D_G_MOVEWORD` (`rt64_gbi_f3d.h:33`).
pub const F3D_G_MOVEWORD: u8 = 0xBC;
/// `F3D_G_MTX` (`rt64_gbi_f3d.h:34`).
pub const F3D_G_MTX: u8 = 0x01;
/// `F3D_G_POPMTX` (`rt64_gbi_f3d.h:35`).
pub const F3D_G_POPMTX: u8 = 0xBD;
/// `F3D_G_TEXTURE` (`rt64_gbi_f3d.h:36`).
pub const F3D_G_TEXTURE: u8 = 0xBB;
/// `F3D_G_VTX` (`rt64_gbi_f3d.h:37`).
pub const F3D_G_VTX: u8 = 0x04;
/// `F3D_G_CULLDL` (`rt64_gbi_f3d.h:38`).
pub const F3D_G_CULLDL: u8 = 0xBE;
/// `F3D_G_TRI1` (`rt64_gbi_f3d.h:39`).
pub const F3D_G_TRI1: u8 = 0xBF;
/// `F3D_G_QUAD` (`rt64_gbi_f3d.h:40`).
pub const F3D_G_QUAD: u8 = 0xB5;
/// `F3D_G_SPRITE2D_BASE` (`rt64_gbi_f3d.h:41`).
pub const F3D_G_SPRITE2D_BASE: u8 = 0x09;
/// `F3D_G_SETGEOMETRYMODE` (`rt64_gbi_f3d.h:42`).
pub const F3D_G_SETGEOMETRYMODE: u8 = 0xB7;
/// `F3D_G_CLEARGEOMETRYMODE` (`rt64_gbi_f3d.h:43`).
pub const F3D_G_CLEARGEOMETRYMODE: u8 = 0xB6;

// ---------------------------------------------------------------------------
// src/gbi/rt64_gbi_f3d.h -- movemem indices, declaration order
// ---------------------------------------------------------------------------

/// `F3D_G_MV_VIEWPORT` (`rt64_gbi_f3d.h:44`).
pub const F3D_G_MV_VIEWPORT: u8 = 0x80;
/// `F3D_G_MV_LOOKATY` (`rt64_gbi_f3d.h:45`).
pub const F3D_G_MV_LOOKATY: u8 = 0x82;
/// `F3D_G_MV_LOOKATX` (`rt64_gbi_f3d.h:46`).
pub const F3D_G_MV_LOOKATX: u8 = 0x84;
/// `F3D_G_MV_L0` (`rt64_gbi_f3d.h:47`).
pub const F3D_G_MV_L0: u8 = 0x86;
/// `F3D_G_MV_L1` (`rt64_gbi_f3d.h:48`).
pub const F3D_G_MV_L1: u8 = 0x88;
/// `F3D_G_MV_L2` (`rt64_gbi_f3d.h:49`).
pub const F3D_G_MV_L2: u8 = 0x8a;
/// `F3D_G_MV_L3` (`rt64_gbi_f3d.h:50`).
pub const F3D_G_MV_L3: u8 = 0x8c;
/// `F3D_G_MV_L4` (`rt64_gbi_f3d.h:51`).
pub const F3D_G_MV_L4: u8 = 0x8e;
/// `F3D_G_MV_L5` (`rt64_gbi_f3d.h:52`).
pub const F3D_G_MV_L5: u8 = 0x90;
/// `F3D_G_MV_L6` (`rt64_gbi_f3d.h:53`).
pub const F3D_G_MV_L6: u8 = 0x92;
/// `F3D_G_MV_L7` (`rt64_gbi_f3d.h:54`).
pub const F3D_G_MV_L7: u8 = 0x94;
/// `F3D_G_MV_TXTATT` (`rt64_gbi_f3d.h:55`).
pub const F3D_G_MV_TXTATT: u8 = 0x96;
/// `F3D_G_MV_MATRIX_1` (`rt64_gbi_f3d.h:56`). Declared **before**
/// `MATRIX_2..4` but numerically **after** all three; see the module doc's
/// "Admitted domain".
pub const F3D_G_MV_MATRIX_1: u8 = 0x9e;
/// `F3D_G_MV_MATRIX_2` (`rt64_gbi_f3d.h:57`).
pub const F3D_G_MV_MATRIX_2: u8 = 0x98;
/// `F3D_G_MV_MATRIX_3` (`rt64_gbi_f3d.h:58`).
pub const F3D_G_MV_MATRIX_3: u8 = 0x9a;
/// `F3D_G_MV_MATRIX_4` (`rt64_gbi_f3d.h:59`).
pub const F3D_G_MV_MATRIX_4: u8 = 0x9c;

// ---------------------------------------------------------------------------
// src/gbi/rt64_gbi_f3dex.h
// ---------------------------------------------------------------------------

/// `F3DEX_G_LOAD_UCODE` (`rt64_gbi_f3dex.h:9`).
pub const F3DEX_G_LOAD_UCODE: u8 = 0xAF;
/// `F3DEX_G_MODIFYVTX` (`rt64_gbi_f3dex.h:10`).
pub const F3DEX_G_MODIFYVTX: u8 = 0xB2;
/// `F3DEX_G_BRANCH_Z` (`rt64_gbi_f3dex.h:11`).
pub const F3DEX_G_BRANCH_Z: u8 = 0xB0;
/// `F3DEX_G_TRI2` (`rt64_gbi_f3dex.h:12`).
pub const F3DEX_G_TRI2: u8 = 0xB1;

// ---------------------------------------------------------------------------
// src/gbi/rt64_gbi_s2dex.h
// ---------------------------------------------------------------------------

/// `S2DEX_G_BG_1CYC` (`rt64_gbi_s2dex.h:9`).
pub const S2DEX_G_BG_1CYC: u8 = 0x01;
/// `S2DEX_G_BG_COPY` (`rt64_gbi_s2dex.h:10`).
pub const S2DEX_G_BG_COPY: u8 = 0x02;
/// `S2DEX_G_RDPHALF_0` (`rt64_gbi_s2dex.h:11`).
pub const S2DEX_G_RDPHALF_0: u8 = 0xE4;
/// `S2DEX_G_SELECT_DL` (`rt64_gbi_s2dex.h:12`). Distinct from the S2DEX2
/// opcode of the same role, which is `0x04`.
pub const S2DEX_G_SELECT_DL: u8 = 0xB0;
/// `S2DEX_G_OBJ_LOADTXTR` (`rt64_gbi_s2dex.h:13`).
pub const S2DEX_G_OBJ_LOADTXTR: u8 = 0xC1;
/// `S2DEX_G_OBJ_LDTX_SPRITE` (`rt64_gbi_s2dex.h:14`).
pub const S2DEX_G_OBJ_LDTX_SPRITE: u8 = 0xC2;
/// `S2DEX_G_OBJ_LDTX_RECT` (`rt64_gbi_s2dex.h:15`).
pub const S2DEX_G_OBJ_LDTX_RECT: u8 = 0xC3;
/// `S2DEX_G_OBJ_LDTX_RECT_R` (`rt64_gbi_s2dex.h:16`).
pub const S2DEX_G_OBJ_LDTX_RECT_R: u8 = 0xC4;
/// `S2DEX_G_OBJ_RENDERMODE` (`rt64_gbi_s2dex.h:17`).
pub const S2DEX_G_OBJ_RENDERMODE: u8 = 0xB1;

/// `S2DEX_G_BGLT_LOADBLOCK` (`rt64_gbi_s2dex.h:18`). Wider than a byte; not
/// an opcode.
pub const S2DEX_G_BGLT_LOADBLOCK: u16 = 0x0033;
/// `S2DEX_G_BGLT_LOADTILE` (`rt64_gbi_s2dex.h:19`). Wider than a byte; not
/// an opcode.
pub const S2DEX_G_BGLT_LOADTILE: u16 = 0xFFF4;

/// `S2DEX_G_BG_FLAG_FLIPS` (`rt64_gbi_s2dex.h:20`), bit 0.
pub const S2DEX_G_BG_FLAG_FLIPS: u8 = 0x01;
/// `S2DEX_G_BG_FLAG_FLIPT` (`rt64_gbi_s2dex.h:21`), bit 4 -- **not** bit 1.
/// Bits 1..3 are unclaimed; the gap is genuine upstream (see the module
/// doc's "Admitted domain").
pub const S2DEX_G_BG_FLAG_FLIPT: u8 = 0x10;

/// `S2DEX_G_OBJRM_NOTXCLAMP` (`rt64_gbi_s2dex.h:22`), bit 0.
pub const S2DEX_G_OBJRM_NOTXCLAMP: u8 = 0x01;
/// `S2DEX_G_OBJRM_XLU` (`rt64_gbi_s2dex.h:23`), bit 1.
pub const S2DEX_G_OBJRM_XLU: u8 = 0x02;
/// `S2DEX_G_OBJRM_ANTIALIAS` (`rt64_gbi_s2dex.h:24`), bit 2.
pub const S2DEX_G_OBJRM_ANTIALIAS: u8 = 0x04;
/// `S2DEX_G_OBJRM_BILERP` (`rt64_gbi_s2dex.h:25`), bit 3.
pub const S2DEX_G_OBJRM_BILERP: u8 = 0x08;
/// `S2DEX_G_OBJRM_SHRINKSIZE_1` (`rt64_gbi_s2dex.h:26`), bit 4.
pub const S2DEX_G_OBJRM_SHRINKSIZE_1: u8 = 0x10;
/// `S2DEX_G_OBJRM_SHRINKSIZE_2` (`rt64_gbi_s2dex.h:27`), bit 5.
pub const S2DEX_G_OBJRM_SHRINKSIZE_2: u8 = 0x20;
/// `S2DEX_G_OBJRM_WIDEN` (`rt64_gbi_s2dex.h:28`), bit 6.
pub const S2DEX_G_OBJRM_WIDEN: u8 = 0x40;

// ---------------------------------------------------------------------------
// src/gbi/rt64_gbi_s2dex2.h
//
// `S2DEX2_G_SELECT_DL` (line 13) is NOT redefined here -- it is re-exported
// from `rt64_gbi_s2dex2.rs:296` at the top of this module.
// ---------------------------------------------------------------------------

/// `S2DEX2_G_OBJ_RENDERMODE` (`rt64_gbi_s2dex2.h:9`).
pub const S2DEX2_G_OBJ_RENDERMODE: u8 = 0x0B;
/// `S2DEX2_G_BG_1CYC` (`rt64_gbi_s2dex2.h:10`).
pub const S2DEX2_G_BG_1CYC: u8 = 0x09;
/// `S2DEX2_G_BG_COPY` (`rt64_gbi_s2dex2.h:11`).
pub const S2DEX2_G_BG_COPY: u8 = 0x0A;
/// `S2DEX2_G_RDPHALF_0` (`rt64_gbi_s2dex2.h:12`). Same value as the S2DEX
/// opcode of the same name.
pub const S2DEX2_G_RDPHALF_0: u8 = 0xE4;
/// `S2DEX2_G_OBJ_LOADTXTR` (`rt64_gbi_s2dex2.h:14`).
pub const S2DEX2_G_OBJ_LOADTXTR: u8 = 0x05;
/// `S2DEX2_G_OBJ_LDTX_SPRITE` (`rt64_gbi_s2dex2.h:15`).
pub const S2DEX2_G_OBJ_LDTX_SPRITE: u8 = 0x06;
/// `S2DEX2_G_OBJ_LDTX_RECT` (`rt64_gbi_s2dex2.h:16`).
pub const S2DEX2_G_OBJ_LDTX_RECT: u8 = 0x07;
/// `S2DEX2_G_OBJ_LDTX_RECT_R` (`rt64_gbi_s2dex2.h:17`).
pub const S2DEX2_G_OBJ_LDTX_RECT_R: u8 = 0x08;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt64_gbi_s2dex2::S2DEX2_G_SELECT_DL;

    /// Every `#define` in `rt64_gbi_f3d.h:9-59`, in declaration order, as a
    /// positional array of `(name, value)`. Array order is pinnable in safe
    /// Rust (unlike struct field order), so this is the form the
    /// declaration-order facts are asserted through.
    const F3D_DECLARATION_ORDER: [(&str, u8); 51] = [
        ("F3D_G_MW_POINTS", F3D_G_MW_POINTS),
        ("F3D_G_MWO_aLIGHT_2", F3D_G_MWO_A_LIGHT_2),
        ("F3D_G_MWO_bLIGHT_2", F3D_G_MWO_B_LIGHT_2),
        ("F3D_G_MWO_aLIGHT_3", F3D_G_MWO_A_LIGHT_3),
        ("F3D_G_MWO_bLIGHT_3", F3D_G_MWO_B_LIGHT_3),
        ("F3D_G_MWO_aLIGHT_4", F3D_G_MWO_A_LIGHT_4),
        ("F3D_G_MWO_bLIGHT_4", F3D_G_MWO_B_LIGHT_4),
        ("F3D_G_MWO_aLIGHT_5", F3D_G_MWO_A_LIGHT_5),
        ("F3D_G_MWO_bLIGHT_5", F3D_G_MWO_B_LIGHT_5),
        ("F3D_G_MWO_aLIGHT_6", F3D_G_MWO_A_LIGHT_6),
        ("F3D_G_MWO_bLIGHT_6", F3D_G_MWO_B_LIGHT_6),
        ("F3D_G_MWO_aLIGHT_7", F3D_G_MWO_A_LIGHT_7),
        ("F3D_G_MWO_bLIGHT_7", F3D_G_MWO_B_LIGHT_7),
        ("F3D_G_MWO_aLIGHT_8", F3D_G_MWO_A_LIGHT_8),
        ("F3D_G_MWO_bLIGHT_8", F3D_G_MWO_B_LIGHT_8),
        ("F3D_G_NOOP", F3D_G_NOOP),
        ("F3D_G_SETOTHERMODE_H", F3D_G_SETOTHERMODE_H),
        ("F3D_G_SETOTHERMODE_L", F3D_G_SETOTHERMODE_L),
        ("F3D_G_RDPHALF_1", F3D_G_RDPHALF_1),
        ("F3D_G_RDPHALF_2", F3D_G_RDPHALF_2),
        ("F3D_G_SPNOOP", F3D_G_SPNOOP),
        ("F3D_G_ENDDL", F3D_G_ENDDL),
        ("F3D_G_DL", F3D_G_DL),
        ("F3D_G_MOVEMEM", F3D_G_MOVEMEM),
        ("F3D_G_MOVEWORD", F3D_G_MOVEWORD),
        ("F3D_G_MTX", F3D_G_MTX),
        ("F3D_G_POPMTX", F3D_G_POPMTX),
        ("F3D_G_TEXTURE", F3D_G_TEXTURE),
        ("F3D_G_VTX", F3D_G_VTX),
        ("F3D_G_CULLDL", F3D_G_CULLDL),
        ("F3D_G_TRI1", F3D_G_TRI1),
        ("F3D_G_QUAD", F3D_G_QUAD),
        ("F3D_G_SPRITE2D_BASE", F3D_G_SPRITE2D_BASE),
        ("F3D_G_SETGEOMETRYMODE", F3D_G_SETGEOMETRYMODE),
        ("F3D_G_CLEARGEOMETRYMODE", F3D_G_CLEARGEOMETRYMODE),
        ("F3D_G_MV_VIEWPORT", F3D_G_MV_VIEWPORT),
        ("F3D_G_MV_LOOKATY", F3D_G_MV_LOOKATY),
        ("F3D_G_MV_LOOKATX", F3D_G_MV_LOOKATX),
        ("F3D_G_MV_L0", F3D_G_MV_L0),
        ("F3D_G_MV_L1", F3D_G_MV_L1),
        ("F3D_G_MV_L2", F3D_G_MV_L2),
        ("F3D_G_MV_L3", F3D_G_MV_L3),
        ("F3D_G_MV_L4", F3D_G_MV_L4),
        ("F3D_G_MV_L5", F3D_G_MV_L5),
        ("F3D_G_MV_L6", F3D_G_MV_L6),
        ("F3D_G_MV_L7", F3D_G_MV_L7),
        ("F3D_G_MV_TXTATT", F3D_G_MV_TXTATT),
        ("F3D_G_MV_MATRIX_1", F3D_G_MV_MATRIX_1),
        ("F3D_G_MV_MATRIX_2", F3D_G_MV_MATRIX_2),
        ("F3D_G_MV_MATRIX_3", F3D_G_MV_MATRIX_3),
        ("F3D_G_MV_MATRIX_4", F3D_G_MV_MATRIX_4),
    ];

    /// The literal values of `rt64_gbi_f3d.h:9-59`, hand-transcribed from the
    /// header a second time and kept **separate** from the constants above.
    /// Checking `F3D_DECLARATION_ORDER` against this is the first of the two
    /// independent readings the card requires; the derived-property tests
    /// below are the second.
    const F3D_LITERALS: [u8; 51] = [
        0x0c, 0x20, 0x24, 0x40, 0x44, 0x60, 0x64, 0x80, 0x84, 0xa0, 0xa4, 0xc0, 0xc4, 0xe0, 0xe4,
        0xc0, 0xBA, 0xB9, 0xB4, 0xB3, 0x00, 0xB8, 0x06, 0x03, 0xBC, 0x01, 0xBD, 0xBB, 0x04, 0xBE,
        0xBF, 0xB5, 0x09, 0xB7, 0xB6, 0x80, 0x82, 0x84, 0x86, 0x88, 0x8a, 0x8c, 0x8e, 0x90, 0x92,
        0x94, 0x96, 0x9e, 0x98, 0x9a, 0x9c,
    ];

    /// Reading one: the 51 constants, positionally, against an independently
    /// transcribed literal list. Any single-value edit, any dropped entry
    /// (length changes) and any adjacent swap (positions disagree) is caught.
    #[test]
    fn gbi_opcodes_f3d_table_matches_independent_transcription() {
        assert_eq!(
            F3D_DECLARATION_ORDER.len(),
            F3D_LITERALS.len(),
            "the two independent readings of rt64_gbi_f3d.h:9-59 disagree on length"
        );
        assert_eq!(F3D_DECLARATION_ORDER.len(), 51);
        for (i, (name, value)) in F3D_DECLARATION_ORDER.iter().enumerate() {
            assert_eq!(
                *value, F3D_LITERALS[i],
                "{name} (declaration position {i}) disagrees between the two readings"
            );
        }
    }

    /// Reading two for the light block: the closed form
    /// `aLIGHT_n == 0x20 * (n - 1)` and `bLIGHT_n == aLIGHT_n + 4`, plus the
    /// uniform-step check, reconciled against all 14 literals. This is the
    /// regularity the card asked to verify holds across the whole run before
    /// asserting it -- it does, with no break.
    #[test]
    fn gbi_opcodes_f3d_light_offsets_are_regular_across_the_whole_run() {
        let a: [u8; 7] = [
            F3D_G_MWO_A_LIGHT_2,
            F3D_G_MWO_A_LIGHT_3,
            F3D_G_MWO_A_LIGHT_4,
            F3D_G_MWO_A_LIGHT_5,
            F3D_G_MWO_A_LIGHT_6,
            F3D_G_MWO_A_LIGHT_7,
            F3D_G_MWO_A_LIGHT_8,
        ];
        let b: [u8; 7] = [
            F3D_G_MWO_B_LIGHT_2,
            F3D_G_MWO_B_LIGHT_3,
            F3D_G_MWO_B_LIGHT_4,
            F3D_G_MWO_B_LIGHT_5,
            F3D_G_MWO_B_LIGHT_6,
            F3D_G_MWO_B_LIGHT_7,
            F3D_G_MWO_B_LIGHT_8,
        ];

        // Literal reading, hand-transcribed a third time.
        assert_eq!(a, [0x20, 0x40, 0x60, 0x80, 0xa0, 0xc0, 0xe0]);
        assert_eq!(b, [0x24, 0x44, 0x64, 0x84, 0xa4, 0xc4, 0xe4]);

        // Derived reading: n runs 2..=8, so index i corresponds to n = i + 2.
        for (i, &av) in a.iter().enumerate() {
            let n = (i as u8) + 2;
            assert_eq!(
                av,
                0x20u8.wrapping_mul(n - 1),
                "aLIGHT_{n} breaks the 0x20 * (n - 1) rule"
            );
            assert_eq!(b[i], av + 4, "bLIGHT_{n} is not aLIGHT_{n} + 4");
        }

        // Third reading: successive differences are a uniform 0x20 in both
        // sequences, with no break anywhere in the run.
        for i in 0..6 {
            assert_eq!(a[i + 1] - a[i], 0x20, "aLIGHT step breaks at index {i}");
            assert_eq!(b[i + 1] - b[i], 0x20, "bLIGHT step breaks at index {i}");
        }

        // The rule extends to n = 1 giving 0x00 / 0x04, which is what the
        // uncited rt64_f3d_defines.h:138-139 records. Recorded as an
        // observation about the rule, not as a ported constant.
        assert_eq!(0x20u8.wrapping_mul(1 - 1), 0x00);
    }

    /// The 16 `F3D_G_MV_*` values form a complete step-2 run over
    /// `0x80..=0x9e` in **numeric** order, while `MATRIX_1` sits out of place
    /// in **declaration** order. Both facts are pinned so neither can be
    /// smoothed into the other.
    #[test]
    fn gbi_opcodes_f3d_movemem_run_is_complete_but_declared_out_of_order() {
        let declared: [u8; 16] = [
            F3D_G_MV_VIEWPORT,
            F3D_G_MV_LOOKATY,
            F3D_G_MV_LOOKATX,
            F3D_G_MV_L0,
            F3D_G_MV_L1,
            F3D_G_MV_L2,
            F3D_G_MV_L3,
            F3D_G_MV_L4,
            F3D_G_MV_L5,
            F3D_G_MV_L6,
            F3D_G_MV_L7,
            F3D_G_MV_TXTATT,
            F3D_G_MV_MATRIX_1,
            F3D_G_MV_MATRIX_2,
            F3D_G_MV_MATRIX_3,
            F3D_G_MV_MATRIX_4,
        ];

        // Numeric reading: sorting gives exactly 0x80, 0x82, ..., 0x9e.
        let mut sorted = declared;
        sorted.sort_unstable();
        let expected: [u8; 16] = [
            0x80, 0x82, 0x84, 0x86, 0x88, 0x8a, 0x8c, 0x8e, 0x90, 0x92, 0x94, 0x96, 0x98, 0x9a,
            0x9c, 0x9e,
        ];
        assert_eq!(sorted, expected, "the MV run is not a complete step-2 run");
        for i in 0..15 {
            assert_eq!(
                sorted[i + 1] - sorted[i],
                2,
                "MV numeric step breaks at {i}"
            );
        }

        // Declaration reading: the irregularity, pinned exactly. Twelve +2
        // steps, then +8 (TXTATT 0x96 -> MATRIX_1 0x9e), then -6 (MATRIX_1
        // 0x9e -> MATRIX_2 0x98), then +2, +2.
        let steps: Vec<i16> = (0..15)
            .map(|i| i16::from(declared[i + 1]) - i16::from(declared[i]))
            .collect();
        assert_eq!(
            steps,
            vec![2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 8, -6, 2, 2],
            "the MATRIX_1 declaration-order irregularity was not preserved"
        );

        // MATRIX_1 is the run's numeric maximum despite being declared first
        // of the four matrix entries.
        assert_eq!(F3D_G_MV_MATRIX_1, 0x9e);
        assert!(F3D_G_MV_MATRIX_1 > F3D_G_MV_MATRIX_2);
        assert!(F3D_G_MV_MATRIX_1 > F3D_G_MV_MATRIX_3);
        assert!(F3D_G_MV_MATRIX_1 > F3D_G_MV_MATRIX_4);

        // The eight light indices are 0x86 + 2i.
        let lights = [
            F3D_G_MV_L0,
            F3D_G_MV_L1,
            F3D_G_MV_L2,
            F3D_G_MV_L3,
            F3D_G_MV_L4,
            F3D_G_MV_L5,
            F3D_G_MV_L6,
            F3D_G_MV_L7,
        ];
        for (i, &v) in lights.iter().enumerate() {
            assert_eq!(v, 0x86 + 2 * (i as u8), "F3D_G_MV_L{i} breaks 0x86 + 2i");
        }
    }

    /// The 20 F3D opcodes: no within-namespace duplicate, a fully contiguous
    /// high run `0xB3..=0xC0`, and a low block with exactly four absent
    /// encodings.
    #[test]
    fn gbi_opcodes_f3d_opcode_block_shape_is_pinned() {
        let ops: [(&str, u8); 20] = [
            ("NOOP", F3D_G_NOOP),
            ("SETOTHERMODE_H", F3D_G_SETOTHERMODE_H),
            ("SETOTHERMODE_L", F3D_G_SETOTHERMODE_L),
            ("RDPHALF_1", F3D_G_RDPHALF_1),
            ("RDPHALF_2", F3D_G_RDPHALF_2),
            ("SPNOOP", F3D_G_SPNOOP),
            ("ENDDL", F3D_G_ENDDL),
            ("DL", F3D_G_DL),
            ("MOVEMEM", F3D_G_MOVEMEM),
            ("MOVEWORD", F3D_G_MOVEWORD),
            ("MTX", F3D_G_MTX),
            ("POPMTX", F3D_G_POPMTX),
            ("TEXTURE", F3D_G_TEXTURE),
            ("VTX", F3D_G_VTX),
            ("CULLDL", F3D_G_CULLDL),
            ("TRI1", F3D_G_TRI1),
            ("QUAD", F3D_G_QUAD),
            ("SPRITE2D_BASE", F3D_G_SPRITE2D_BASE),
            ("SETGEOMETRYMODE", F3D_G_SETGEOMETRYMODE),
            ("CLEARGEOMETRYMODE", F3D_G_CLEARGEOMETRYMODE),
        ];
        assert_no_duplicate_values("F3D opcodes", &ops);

        // High run: 0xB3..=0xC0 inclusive, all 14 present, none missing.
        // F3D_G_NOOP 0xc0 CLOSES this run -- it is not an outlier.
        let mut high: Vec<u8> = ops.iter().map(|&(_, v)| v).filter(|&v| v >= 0xB0).collect();
        high.sort_unstable();
        assert_eq!(high, (0xB3u8..=0xC0).collect::<Vec<_>>());

        // Low block: exactly these six, with 0x02/0x05/0x07/0x08 absent from
        // this header (no claim about hardware -- see "Nonclaims").
        let mut low: Vec<u8> = ops.iter().map(|&(_, v)| v).filter(|&v| v < 0xB0).collect();
        low.sort_unstable();
        assert_eq!(low, vec![0x00, 0x01, 0x03, 0x04, 0x06, 0x09]);
        for absent in [0x02u8, 0x05, 0x07, 0x08] {
            assert!(
                !low.contains(&absent),
                "{absent:#04x} is not defined in rt64_gbi_f3d.h"
            );
        }
    }

    /// `rt64_gbi_f3dex.h:9-12`, all four, plus the contiguity of the
    /// `0xAF..=0xB2` block and disjointness from the F3D opcodes.
    #[test]
    fn gbi_opcodes_f3dex_table_is_a_contiguous_four_opcode_block() {
        let declared: [(&str, u8); 4] = [
            ("F3DEX_G_LOAD_UCODE", F3DEX_G_LOAD_UCODE),
            ("F3DEX_G_MODIFYVTX", F3DEX_G_MODIFYVTX),
            ("F3DEX_G_BRANCH_Z", F3DEX_G_BRANCH_Z),
            ("F3DEX_G_TRI2", F3DEX_G_TRI2),
        ];
        // Reading one: literals in declaration order (note this is NOT
        // ascending -- the header declares 0xAF, 0xB2, 0xB0, 0xB1).
        assert_eq!(
            declared.map(|(_, v)| v),
            [0xAF, 0xB2, 0xB0, 0xB1],
            "the F3DEX declaration order was reordered"
        );
        assert_no_duplicate_values("F3DEX opcodes", &declared);

        // Reading two: sorted, the four fill 0xAF..=0xB2 with no gap.
        let mut sorted = declared.map(|(_, v)| v);
        sorted.sort_unstable();
        assert_eq!(sorted, [0xAF, 0xB0, 0xB1, 0xB2]);
        for i in 0..3 {
            assert_eq!(sorted[i + 1] - sorted[i], 1);
        }

        // Disjoint from the F3D opcode set: F3D's high run starts at 0xB3.
        assert!(sorted.iter().all(|&v| v < 0xB3));
    }

    /// `rt64_gbi_s2dex.h:9-28`, all 20, split into its four sub-blocks. The
    /// `BG_FLAG` bit gap and the `OBJRM` power-of-two completeness are the
    /// two shape facts pinned here.
    #[test]
    fn gbi_opcodes_s2dex_tables_pin_the_bit_gap_and_the_flag_run() {
        let opcodes: [(&str, u8); 9] = [
            ("S2DEX_G_BG_1CYC", S2DEX_G_BG_1CYC),
            ("S2DEX_G_BG_COPY", S2DEX_G_BG_COPY),
            ("S2DEX_G_RDPHALF_0", S2DEX_G_RDPHALF_0),
            ("S2DEX_G_SELECT_DL", S2DEX_G_SELECT_DL),
            ("S2DEX_G_OBJ_LOADTXTR", S2DEX_G_OBJ_LOADTXTR),
            ("S2DEX_G_OBJ_LDTX_SPRITE", S2DEX_G_OBJ_LDTX_SPRITE),
            ("S2DEX_G_OBJ_LDTX_RECT", S2DEX_G_OBJ_LDTX_RECT),
            ("S2DEX_G_OBJ_LDTX_RECT_R", S2DEX_G_OBJ_LDTX_RECT_R),
            ("S2DEX_G_OBJ_RENDERMODE", S2DEX_G_OBJ_RENDERMODE),
        ];
        assert_eq!(
            opcodes.map(|(_, v)| v),
            [0x01, 0x02, 0xE4, 0xB0, 0xC1, 0xC2, 0xC3, 0xC4, 0xB1],
            "the S2DEX opcode declaration order or a value changed"
        );
        assert_no_duplicate_values("S2DEX opcodes", &opcodes);

        // The four OBJ_LDTX* opcodes are a contiguous 0xC1..=0xC4 run.
        let ldtx = [
            S2DEX_G_OBJ_LOADTXTR,
            S2DEX_G_OBJ_LDTX_SPRITE,
            S2DEX_G_OBJ_LDTX_RECT,
            S2DEX_G_OBJ_LDTX_RECT_R,
        ];
        assert_eq!(ldtx, [0xC1, 0xC2, 0xC3, 0xC4]);
        for i in 0..3 {
            assert_eq!(ldtx[i + 1] - ldtx[i], 1);
        }

        // The two wide BGLT values.
        assert_eq!(S2DEX_G_BGLT_LOADBLOCK, 0x0033);
        assert_eq!(S2DEX_G_BGLT_LOADTILE, 0xFFF4);
        assert!(u16::from(u8::MAX) < S2DEX_G_BGLT_LOADTILE);

        // BG_FLAG: bit 0 then bit 4. THE GAP IS GENUINE -- bits 1..3 are
        // unclaimed. This asserts the gap rather than smoothing it away.
        assert_eq!(S2DEX_G_BG_FLAG_FLIPS, 0x01);
        assert_eq!(S2DEX_G_BG_FLAG_FLIPT, 0x10);
        assert_eq!(S2DEX_G_BG_FLAG_FLIPS.trailing_zeros(), 0);
        assert_eq!(S2DEX_G_BG_FLAG_FLIPT.trailing_zeros(), 4);
        assert_ne!(
            S2DEX_G_BG_FLAG_FLIPT, 0x02,
            "FLIPT is bit 4, not bit 1 -- do not 'correct' the gap"
        );
        assert_eq!(S2DEX_G_BG_FLAG_FLIPS | S2DEX_G_BG_FLAG_FLIPT, 0x11);

        // OBJRM: reading one, the seven literals in declaration order.
        let objrm = [
            S2DEX_G_OBJRM_NOTXCLAMP,
            S2DEX_G_OBJRM_XLU,
            S2DEX_G_OBJRM_ANTIALIAS,
            S2DEX_G_OBJRM_BILERP,
            S2DEX_G_OBJRM_SHRINKSIZE_1,
            S2DEX_G_OBJRM_SHRINKSIZE_2,
            S2DEX_G_OBJRM_WIDEN,
        ];
        assert_eq!(objrm, [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40]);

        // OBJRM reading two: the derived form 1 << i, with NO skipped bit --
        // unlike BG_FLAG above. Each is a single bit and bit i is exactly i.
        let mut union = 0u8;
        for (i, &v) in objrm.iter().enumerate() {
            assert_eq!(v, 1u8 << i, "OBJRM entry {i} is not 1 << {i}");
            assert_eq!(v.count_ones(), 1, "OBJRM entry {i} is not a single bit");
            assert_eq!(union & v, 0, "OBJRM entry {i} repeats an earlier bit");
            union |= v;
        }
        // Reconciled: the union is the low seven bits; bit 7 is unclaimed.
        assert_eq!(union, 0x7f);
        assert_eq!(union.count_ones(), 7);
        assert_eq!(union & 0x80, 0);
    }

    /// `rt64_gbi_s2dex2.h:9-17`, all nine including the re-exported
    /// `S2DEX2_G_SELECT_DL`. The eight sub-`0x80` opcodes form a contiguous
    /// `0x04..=0x0B` run.
    #[test]
    fn gbi_opcodes_s2dex2_table_is_contiguous_and_reuses_select_dl() {
        let declared: [(&str, u8); 9] = [
            ("S2DEX2_G_OBJ_RENDERMODE", S2DEX2_G_OBJ_RENDERMODE),
            ("S2DEX2_G_BG_1CYC", S2DEX2_G_BG_1CYC),
            ("S2DEX2_G_BG_COPY", S2DEX2_G_BG_COPY),
            ("S2DEX2_G_RDPHALF_0", S2DEX2_G_RDPHALF_0),
            ("S2DEX2_G_SELECT_DL", S2DEX2_G_SELECT_DL),
            ("S2DEX2_G_OBJ_LOADTXTR", S2DEX2_G_OBJ_LOADTXTR),
            ("S2DEX2_G_OBJ_LDTX_SPRITE", S2DEX2_G_OBJ_LDTX_SPRITE),
            ("S2DEX2_G_OBJ_LDTX_RECT", S2DEX2_G_OBJ_LDTX_RECT),
            ("S2DEX2_G_OBJ_LDTX_RECT_R", S2DEX2_G_OBJ_LDTX_RECT_R),
        ];
        assert_eq!(
            declared.map(|(_, v)| v),
            [0x0B, 0x09, 0x0A, 0xE4, 0x04, 0x05, 0x06, 0x07, 0x08],
            "the S2DEX2 declaration order or a value changed"
        );
        assert_no_duplicate_values("S2DEX2 opcodes", &declared);

        // The re-exported constant must still be 0x04. If rt64_gbi_s2dex2.rs
        // ever changes it, this fails rather than silently diverging.
        assert_eq!(S2DEX2_G_SELECT_DL, 0x04);

        // The eight non-RDP opcodes are a contiguous 0x04..=0x0B run.
        let mut low: Vec<u8> = declared
            .iter()
            .map(|&(_, v)| v)
            .filter(|&v| v < 0x80)
            .collect();
        low.sort_unstable();
        assert_eq!(low, (0x04u8..=0x0B).collect::<Vec<_>>());

        // S2DEX2 renumbers wholesale relative to S2DEX: the same roles get
        // different bytes in the two microcodes, except RDPHALF_0.
        assert_eq!(S2DEX2_G_RDPHALF_0, S2DEX_G_RDPHALF_0);
        assert_ne!(S2DEX2_G_SELECT_DL, S2DEX_G_SELECT_DL);
        assert_ne!(S2DEX2_G_BG_1CYC, S2DEX_G_BG_1CYC);
        assert_ne!(S2DEX2_G_BG_COPY, S2DEX_G_BG_COPY);
        assert_ne!(S2DEX2_G_OBJ_RENDERMODE, S2DEX_G_OBJ_RENDERMODE);
        assert_ne!(S2DEX2_G_OBJ_LOADTXTR, S2DEX_G_OBJ_LOADTXTR);
    }

    /// `UCODE_MAP_SIZE`, and the one relationship it has to the rest of this
    /// module: every ported opcode byte indexes the map in range.
    #[test]
    fn gbi_opcodes_ucode_map_size_covers_every_ported_opcode() {
        assert_eq!(UCODE_MAP_SIZE, 256);
        assert_eq!(UCODE_MAP_SIZE, usize::from(u8::MAX) + 1);

        // Every u8 constant in this module is < UCODE_MAP_SIZE by
        // construction; asserting it over the widest table makes the
        // relationship explicit rather than implicit in the type.
        for (name, value) in F3D_DECLARATION_ORDER {
            assert!(
                usize::from(value) < UCODE_MAP_SIZE,
                "{name} would index GBI::map out of bounds"
            );
        }
    }

    /// The three cross-namespace numeric coincidences, recorded so that a
    /// future reader does not mistake them for duplicates and "fix" one.
    #[test]
    fn gbi_opcodes_cross_namespace_coincidences_are_recorded_not_duplicates() {
        // A moveword light offset that happens to equal an opcode byte.
        assert_eq!(F3D_G_MWO_A_LIGHT_7, F3D_G_NOOP);
        assert_eq!(F3D_G_MWO_A_LIGHT_7, 0xc0);
        // A moveword light offset that happens to equal a movemem index.
        assert_eq!(F3D_G_MWO_A_LIGHT_5, F3D_G_MV_VIEWPORT);
        assert_eq!(F3D_G_MWO_A_LIGHT_5, 0x80);
        // A moveword light offset that happens to equal an S2DEX opcode.
        assert_eq!(F3D_G_MWO_B_LIGHT_8, S2DEX_G_RDPHALF_0);
        assert_eq!(F3D_G_MWO_B_LIGHT_8, 0xE4);
    }

    /// Exhaustive within-namespace duplicate check. Each of the four opcode
    /// namespaces is checked against itself; none has a duplicate.
    fn assert_no_duplicate_values(namespace: &str, entries: &[(&str, u8)]) {
        for i in 0..entries.len() {
            for j in (i + 1)..entries.len() {
                assert_ne!(
                    entries[i].1, entries[j].1,
                    "{namespace}: {} and {} share the value {:#04x}",
                    entries[i].0, entries[j].0, entries[i].1
                );
            }
        }
    }
}
