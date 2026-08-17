//! Literal port of RT64's extended-GBI opcode/word-packing ABI -- the
//! `PARAM` bitfield-packing macro, the `RT64_HOOK_*`/`G_EX_*` opcode and enum
//! constants, and the `gEX*` command-packing macros -- a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `include/rt64_extended_gbi.h` (SHA-256 of the whole file,
//! `4e8ac9d75aee40aac3512a0be1dc7c72140aec0638ef9238606d8d616f285794`):
//!
//! ```text
//! // include/rt64_extended_gbi.h:8-27 (hook opcode/magic-number constants)
//! #ifdef F3DEX_GBI_2
//! #   define RT64_HOOK_OPCODE         0xE0
//! #else
//! #   define RT64_HOOK_OPCODE         0x00
//! #endif
//!
//! #define RT64_HOOK_OP_GETVERSION     0x0
//! #define RT64_HOOK_OP_ENABLE         0x1
//! #define RT64_HOOK_OP_DISABLE        0x2
//! #define RT64_HOOK_OP_DL             0x3
//! #define RT64_HOOK_OP_BRANCH         0x4
//!
//! // 0x5254 for ASCII "RT" followed by 0x64.
//! #define RT64_HOOK_MAGIC_NUMBER      0x525464
//!
//! #ifndef RT64_EXTENDED_OPCODE
//! #   define RT64_EXTENDED_OPCODE     0x64
//! #endif
//!
//! // include/rt64_extended_gbi.h:29-83 (G_EX_* opcode enumeration)
//! #define G_EX_VERSION                0x1
//!
//! #define G_EX_NOOP                       0x000000
//! #define G_EX_PRINT                      0x000001
//! #define G_EX_TEXRECT_V1                 0x000002
//! #define G_EX_FILLRECT_V1                0x000003
//! #define G_EX_SETVIEWPORT_V1             0x000004
//! #define G_EX_SETSCISSOR_V1              0x000005
//! #define G_EX_SETRECTALIGN_V1            0x000006
//! #define G_EX_SETVIEWPORTALIGN_V1        0x000007
//! #define G_EX_SETSCISSORALIGN_V1         0x000008
//! #define G_EX_SETREFRESHRATE_V1          0x000009
//! #define G_EX_VERTEXZTEST_V1             0x00000A
//! #define G_EX_ENDVERTEXZTEST_V1          0x00000B
//! #define G_EX_MATRIXGROUP_V1             0x00000C
//! #define G_EX_POPMATRIXGROUP_V1          0x00000D
//! #define G_EX_FORCEUPSCALE2D_V1          0x00000E
//! #define G_EX_FORCETRUEBILERP_V1         0x00000F
//! #define G_EX_FORCESCALELOD_V1           0x000010
//! #define G_EX_FORCEBRANCH_V1             0x000011
//! #define G_EX_SETRENDERTORAM_V1          0x000012
//! #define G_EX_EDITGROUPBYADDRESS_V1      0x000013
//! #define G_EX_VERTEX_V1                  0x000014
//! #define G_EX_PUSHVIEWPORT_V1            0x000015
//! #define G_EX_POPVIEWPORT_V1             0x000016
//! #define G_EX_PUSHSCISSOR_V1             0x000017
//! #define G_EX_POPSCISSOR_V1              0x000018
//! #define G_EX_PUSHOTHERMODE_V1           0x000019
//! #define G_EX_POPOTHERMODE_V1            0x00001A
//! #define G_EX_PUSHCOMBINE_V1             0x00001B
//! #define G_EX_POPCOMBINE_V1              0x00001C
//! #define G_EX_PUSHPROJMATRIX_V1          0x00001D
//! #define G_EX_POPPROJMATRIX_V1           0x00001E
//! #define G_EX_PUSHENVCOLOR_V1            0x00001F
//! #define G_EX_POPENVCOLOR_V1             0x000020
//! #define G_EX_PUSHBLENDCOLOR_V1          0x000021
//! #define G_EX_POPBLENDCOLOR_V1           0x000022
//! #define G_EX_PUSHFOGCOLOR_V1            0x000023
//! #define G_EX_POPFOGCOLOR_V1             0x000024
//! #define G_EX_PUSHFILLCOLOR_V1           0x000025
//! #define G_EX_POPFILLCOLOR_V1            0x000026
//! #define G_EX_PUSHPRIMCOLOR_V1           0x000027
//! #define G_EX_POPPRIMCOLOR_V1            0x000028
//! #define G_EX_PUSHGEOMETRYMODE_V1        0x000029
//! #define G_EX_POPGEOMETRYMODE_V1         0x00002A
//! #define G_EX_SETDITHERNOISESTRENGTH_V1  0x00002B
//! #define G_EX_SETRDRAMEXTENDED_V1        0x00002C
//! #define G_EX_SETPROJMATRIXFLOAT_V1      0x00002D
//! #define G_EX_SETVIEWMATRIXFLOAT_V1      0x00002E
//! #define G_EX_SETNEARCLIPPING_V1         0x00002F
//! #define G_EX_MATRIX_FLOAT_V1            0x000030
//! #define G_EX_SETVERTEXSEGMENT_V1        0x000031
//! #define G_EX_SETTEXCOORDWRAPPOINT_V1    0x000032
//! #define G_EX_SETRECTASPECT_V1           0x000033
//! #define G_EX_MAX                        0x000034
//!
//! // include/rt64_extended_gbi.h:85-122 (packed-field enums)
//! #define G_EX_ORIGIN_NONE            0x800
//! #define G_EX_ORIGIN_LEFT            0x0
//! #define G_EX_ORIGIN_CENTER          0x200
//! #define G_EX_ORIGIN_RIGHT           0x400
//!
//! #define G_EX_NOPUSH                 0x0
//! #define G_EX_PUSH                   0x1
//!
//! #define G_EX_ID_IGNORE              0x0
//! #define G_EX_ID_AUTO                0xFFFFFFFF
//!
//! #define G_EX_COMPONENT_SKIP         0x0
//! #define G_EX_COMPONENT_INTERPOLATE  0x1
//! #define G_EX_COMPONENT_AUTO         0x2
//!
//! #define G_EX_INTERPOLATE_SIMPLE     0x0
//! #define G_EX_INTERPOLATE_DECOMPOSE  0x1
//!
//! #define G_EX_ORDER_LINEAR           0x0
//! #define G_EX_ORDER_AUTO             0x1
//!
//! #define G_EX_EDIT_NONE              0x0
//! #define G_EX_EDIT_ALLOW             0x1
//!
//! #define G_EX_BILERP_NONE            0x0
//! #define G_EX_BILERP_ONLY            0x1
//! #define G_EX_BILERP_ALL             0x2
//!
//! #define G_EX_ASPECT_AUTO            0x0
//! #define G_EX_ASPECT_STRETCH         0x1
//! #define G_EX_ASPECT_ADJUST          0x2
//!
//! #define G_EX_VERTEX_POSITION        0x0
//! #define G_EX_VERTEX_VELOCITY        0x1
//! #define G_EX_VERTEX_MAX             0x2
//!
//! #define G_EX_DISABLED               0x0
//! #define G_EX_ENABLED                0x1
//!
//! // include/rt64_extended_gbi.h:156-202 (packing primitive and
//! // command-word writers)
//! #define PARAM(value, bits, shift) \
//!     ((unsigned) (((unsigned)(value) & ((1U << (bits)) - 1U)) << (shift)))
//!
//! #define DOWHILE(code) \
//!     do { code } while (0)
//!
//! #define G_EX_WRITECOMMAND(cmd, _word0, _word1) \
//!     { \
//!         cmd->values.word0 = _word0; \
//!         cmd->values.word1 = _word1; \
//!     }
//!
//! #define G_EX_COMMAND1(cmd, _word0, _word1) \
//!     DOWHILE( \
//!         GfxCommand *_cmd = (GfxCommand*)(cmd); \
//!         G_EX_WRITECOMMAND((_cmd + 0), _word0, _word1) \
//!     )
//!
//! #define G_EX_COMMAND2(cmd, _word0, _word1, _word2, _word3) \
//!     DOWHILE( \
//!         GfxCommand *_cmd = (GfxCommand*)(cmd); \
//!         (void)(cmd); \
//!         G_EX_WRITECOMMAND((_cmd + 0), _word0, _word1) \
//!         G_EX_WRITECOMMAND((_cmd + 1), _word2, _word3) \
//!     )
//!
//! #define G_EX_COMMAND3(cmd, _word0, _word1, _word2, _word3, _word4, _word5) \
//!     DOWHILE( \
//!         GfxCommand *_cmd = (GfxCommand*)(cmd); \
//!         (void)(cmd); \
//!         (void)(cmd); \
//!         G_EX_WRITECOMMAND((_cmd + 0), _word0, _word1) \
//!         G_EX_WRITECOMMAND((_cmd + 1), _word2, _word3) \
//!         G_EX_WRITECOMMAND((_cmd + 2), _word4, _word5) \
//!     )
//!
//! // include/rt64_extended_gbi.h:204-580 (gEX* command-packing macros --
//! // every macro this port turns into a pure word-packing function; the
//! // ones that pack a raw untyped pointer/segmented-address argument
//! // directly into a word (gEXViewport's `vp`, gEXMatrixGroup's/
//! // gEXEditGroupByAddress's `id`/`address`, gEXVertex's `vtx`,
//! // gEXSetProjMatrixFloat's/gEXSetViewMatrixFloat's/gEXMatrixFloat's
//! // `matrix`/`m`, gEXSetVertexSegment's `vertexAddress`/
//! // `baseSegmentAddress`) are ported taking that argument as `u32` already
//! // -- see "Nonclaims" for why no pointer type is introduced.
//! #define gEXGetVersion(cmd, ret) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_HOOK_OPCODE, 8, 24) | PARAM(RT64_HOOK_MAGIC_NUMBER, 24, 0), \
//!         PARAM(RT64_HOOK_OP_GETVERSION, 4, 28) | PARAM(ret, 28, 0))
//!
//! #define gEXEnable(cmd) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_HOOK_OPCODE, 8, 24) | PARAM(RT64_HOOK_MAGIC_NUMBER, 24, 0), \
//!         PARAM(RT64_HOOK_OP_ENABLE, 4, 28) | PARAM(RT64_EXTENDED_OPCODE, 8, 0))
//!
//! #define gEXDisable(cmd) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_HOOK_OPCODE, 8, 24) | PARAM(RT64_HOOK_MAGIC_NUMBER, 24, 0), \
//!         PARAM(RT64_HOOK_OP_DISABLE, 4, 28))
//!
//! #define gEXBranchList(cmd, dlist) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_HOOK_OPCODE, 8, 24) | PARAM(RT64_HOOK_MAGIC_NUMBER, 24, 0), \
//!         PARAM(RT64_HOOK_OP_BRANCH, 4, 28) | PARAM(dlist, 28, 0))
//!
//! #define gEXDisplayList(cmd, dlist) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_HOOK_OPCODE, 8, 24) | PARAM(RT64_HOOK_MAGIC_NUMBER, 24, 0), \
//!         PARAM(RT64_HOOK_OP_DL, 4, 28) | PARAM(dlist, 28, 0))
//!
//! #define gEXNoOp(cmd) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_NOOP, 24, 0), \
//!         0)
//!
//! #define gEXPrint(cmd) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PRINT, 24, 0), \
//!         0)
//!
//! #define gEXTextureRectangle(cmd, lorigin, rorigin, ulx, uly, lrx, lry, tile, s, t, dsdx, dtdy) \
//!     G_EX_COMMAND3(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_TEXRECT_V1, 24, 0), \
//!         PARAM(tile, 3, 0) | PARAM(lorigin, 12, 3) | PARAM(rorigin, 12, 15) | PARAM(0, 5, 27), \
//!         PARAM(ulx, 16, 16) | PARAM(uly, 16, 0), \
//!         PARAM(lrx, 16, 16) | PARAM(lry, 16, 0), \
//!         PARAM(s, 16, 16) | PARAM(t, 16, 0), \
//!         PARAM(dsdx, 16, 16) | PARAM(dtdy, 16, 0) \
//!     )
//!
//! #define gEXViewport(cmd, origin, vp) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETVIEWPORT_V1, 24, 0), \
//!         PARAM(origin, 12, 0), \
//!         0, \
//!         (unsigned)vp \
//!     )
//!
//! #define gEXSetScissor(cmd, mode, lorigin, rorigin, ulx, uly, lrx, lry) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETSCISSOR_V1, 24, 0), \
//!         PARAM(mode, 2, 0) | PARAM(lorigin, 12, 2) | PARAM(rorigin, 12, 14), \
//!         PARAM((ulx) * 4, 16, 16) | PARAM((uly) * 4, 16, 0), \
//!         PARAM((lrx) * 4, 16, 16) | PARAM((lry) * 4, 16, 0) \
//!     )
//!
//! #define gEXSetRectAlign(cmd, lorigin, rorigin, ulxOffset, ulyOffset, lrxOffset, lryOffset) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETRECTALIGN_V1, 24, 0), \
//!         PARAM(lorigin, 12, 0) | PARAM(rorigin, 12, 12), \
//!         PARAM((ulxOffset), 16, 16) | PARAM((ulyOffset), 16, 0), \
//!         PARAM((lrxOffset), 16, 16) | PARAM((lryOffset), 16, 0) \
//!     )
//!
//! #define gEXSetViewportAlign(cmd, origin, xOffset, yOffset) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETVIEWPORTALIGN_V1, 24, 0), \
//!         PARAM(origin, 12, 0), \
//!         PARAM((xOffset), 16, 16) | PARAM((yOffset), 16, 0), \
//!         0 \
//!     )
//!
//! #define gEXSetScissorAlign(cmd, lorigin, rorigin, ulxOffset, ulyOffset, lrxOffset, lryOffset, ulxBound, ulyBound, lrxBound, lryBound) \
//!     G_EX_COMMAND3(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETSCISSORALIGN_V1, 24, 0), \
//!         PARAM(lorigin, 12, 0) | PARAM(rorigin, 12, 12), \
//!         PARAM((ulxOffset) * 4, 16, 16) | PARAM((ulyOffset) * 4, 16, 0), \
//!         PARAM((lrxOffset) * 4, 16, 16) | PARAM((lryOffset) * 4, 16, 0), \
//!         PARAM((ulxBound) * 4, 16, 16) | PARAM((ulyBound) * 4, 16, 0), \
//!         PARAM((lrxBound) * 4, 16, 16) | PARAM((lryBound) * 4, 16, 0) \
//!     )
//!
//! #define gEXSetRefreshRate(cmd, refresh_rate) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETREFRESHRATE_V1, 24, 0), \
//!         PARAM(refresh_rate, 16, 0) \
//!     )
//!
//! #define gEXVertexZTest(cmd, vertex_index) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_VERTEXZTEST_V1, 24, 0), \
//!         PARAM(vertex_index, 8, 0) \
//!     )
//!
//! #define gEXEndVertexZTest(cmd) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_ENDVERTEXZTEST_V1, 24, 0), \
//!         0 \
//!     )
//!
//! #define gEXMatrixGroup(cmd, id, mode, push, proj, pos, rot, scale, skew, persp, vert, tile, order, edit, aspect, tc, lookat) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_MATRIXGROUP_V1, 24, 0), \
//!         id, \
//!         PARAM(push, 1, 0) | PARAM((proj) != 0, 1, 1) | PARAM(mode, 1, 2) | PARAM(pos, 2, 3) | PARAM(rot, 2, 5) | PARAM(scale, 2, 7) | PARAM(skew, 2, 9) | PARAM(persp, 2, 11) | PARAM(vert, 2, 13) | PARAM(tile, 2, 15) | PARAM(order, 2, 17) | PARAM(edit, 1, 19) | PARAM(aspect, 2, 20) | PARAM(tc, 2, 22) | PARAM(lookat, 2, 24), \
//!         0 \
//!     )
//!
//! #define gEXPopMatrixGroup(cmd, proj) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPMATRIXGROUP_V1, 24, 0), \
//!         PARAM(1, 8, 0) | PARAM(proj, 1, 8) \
//!     )
//!
//! #define gEXPopMatrixGroupN(cmd, proj, count) \
//!     G_EX_COMMAND1(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPMATRIXGROUP_V1, 24, 0), \
//!         PARAM(count, 8, 0) | PARAM(proj, 1, 8) \
//!     )
//!
//! #define gEXForceUpscale2D(cmd, force) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_FORCEUPSCALE2D_V1, 24, 0), PARAM(force, 1, 0))
//!
//! #define gEXForceTrueBilerp(cmd, mode) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_FORCETRUEBILERP_V1, 24, 0), PARAM(mode, 2, 0))
//!
//! #define gEXForceScaleLOD(cmd, force) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_FORCESCALELOD_V1, 24, 0), PARAM(force, 1, 0))
//!
//! #define gEXForceBranch(cmd, force) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_FORCEBRANCH_V1, 24, 0), PARAM(force, 1, 0))
//!
//! #define gEXSetRenderToRAM(cmd, render) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETRENDERTORAM_V1, 24, 0), PARAM(render, 1, 0))
//!
//! #define gEXEditGroupByAddress(cmd, address, mode, push, proj, pos, rot, scale, skew, persp, vert, tile, order) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_EDITGROUPBYADDRESS_V1, 24, 0), \
//!         (unsigned)(address), \
//!         PARAM(push, 1, 0) | PARAM((proj) != 0, 1, 1) | PARAM(mode, 1, 2) | PARAM(pos, 2, 3) | PARAM(rot, 2, 5) | PARAM(scale, 2, 7) | PARAM(skew, 2, 9) | PARAM(persp, 2, 11) | PARAM(vert, 2, 13) | PARAM(tile, 2, 15) | PARAM(order, 2, 17) | PARAM(G_EX_EDIT_ALLOW, 1, 18), \
//!         0 \
//!     )
//!
//! #define gEXVertex(cmd, vtx, count, v0) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_VERTEX_V1, 24, 0), \
//!         PARAM((v0), 8, 0) | PARAM((count), 8, 8), \
//!         0, \
//!         (unsigned)(vtx) \
//!     )
//!
//! #define gEXSetProjMatrixFloat(cmd, matrix) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETPROJMATRIXFLOAT_V1, 24, 0), (unsigned)(matrix))
//!
//! #define gEXSetViewMatrixFloat(cmd, matrix) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETVIEWMATRIXFLOAT_V1, 24, 0), (unsigned)(matrix))
//!
//! #define gEXPushViewport(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHVIEWPORT_V1, 24, 0), 0)
//!
//! #define gEXPopViewport(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPVIEWPORT_V1, 24, 0), 0)
//!
//! #define gEXPushScissor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHSCISSOR_V1, 24, 0), 0)
//!
//! #define gEXPopScissor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPSCISSOR_V1, 24, 0), 0)
//!
//! #define gEXPushOtherMode(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHOTHERMODE_V1, 24, 0), 0)
//!
//! #define gEXPopOtherMode(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPOTHERMODE_V1, 24, 0), 0)
//!
//! #define gEXPushCombineMode(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHCOMBINE_V1, 24, 0), 0)
//!
//! #define gEXPopCombineMode(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPCOMBINE_V1, 24, 0), 0)
//!
//! #define gEXPushProjectionMatrix(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHPROJMATRIX_V1, 24, 0), 0)
//!
//! #define gEXPopProjectionMatrix(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPPROJMATRIX_V1, 24, 0), 0)
//!
//! #define gEXPushEnvColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHENVCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPopEnvColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPENVCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPushBlendColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHBLENDCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPopBlendColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPBLENDCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPushFogColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHFOGCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPopFogColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPFOGCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPushFillColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHFILLCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPopFillColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPFILLCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPushPrimColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHPRIMCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPopPrimColor(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPPRIMCOLOR_V1, 24, 0), 0)
//!
//! #define gEXPushGeometryMode(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_PUSHGEOMETRYMODE_V1, 24, 0), 0)
//!
//! #define gEXPopGeometryMode(cmd) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_POPGEOMETRYMODE_V1, 24, 0), 0)
//!
//! #define gEXSetDitherNoiseStrength(cmd, value) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETDITHERNOISESTRENGTH_V1, 24, 0), PARAM((value) * 1024, 16, 0))
//!
//! #define gEXSetRDRAMExtended(cmd, isExtended) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETRDRAMEXTENDED_V1, 24, 0), PARAM(isExtended, 1, 0))
//!
//! #define gEXSetNearClipping(cmd, isEnabled) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETNEARCLIPPING_V1, 24, 0), PARAM(isEnabled, 1, 0))
//!
//! #define gEXMatrixFloat(cmd, m, p) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_MATRIX_FLOAT_V1, 24, 0), \
//!         PARAM((p), 8, 0), \
//!         0, \
//!         (unsigned)(m) \
//!     )
//!
//! #define gEXSetVertexSegment(cmd, vertexElement, isEnabled, vertexAddress, baseSegmentAddress) \
//!     G_EX_COMMAND2(cmd, \
//!         PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETVERTEXSEGMENT_V1, 24, 0), \
//!         PARAM((isEnabled), 1, 0) | PARAM((vertexElement), 4, 1), \
//!         (unsigned)(vertexAddress), \
//!         (unsigned)(baseSegmentAddress) \
//!     )
//!
//! #define gEXSetTexcoordWrapPoint(cmd, wrapPointU, wrapPointV) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETTEXCOORDWRAPPOINT_V1, 24, 0), PARAM(wrapPointU, 16, 16) | PARAM(wrapPointV, 16, 0))
//!
//! #define gEXSetRectAspect(cmd, aspect) \
//!     G_EX_COMMAND1(cmd, PARAM(RT64_EXTENDED_OPCODE, 8, 24) | PARAM(G_EX_SETRECTASPECT_V1, 24, 0), PARAM(aspect, 2, 0))
//! ```
//!
//! **Reuse, not new type.** No existing module in this crate or in
//! `crates/fn64-render-reference/src/gbi/` owns 32-bit command-word *packing*
//! (as opposed to decoding): the three sibling ports
//! (`rt64_gbi_f3d.rs`/`rt64_gbi_f3dex.rs`/`rt64_gbi_s2dex2.rs`) each define a
//! private `p0`/`p1` word -> field extractor, the opposite direction of this
//! file's `PARAM` field -> word packer, and `fn64-render-reference`'s GBI
//! interpreter only ever reads command words a ROM already wrote, never
//! constructs them. This module's `param` function is therefore a new,
//! non-duplicating primitive; it is intentionally private (`fn`, not `pub
//! fn`) since nothing outside this file's own packing functions needs it,
//! matching `rt64_gbi_f3d.rs`'s `p0`/`p1` visibility precedent for the
//! opposite-direction primitive.
//!
//! ## Admitted domain
//!
//! - **`PARAM(value, bits, shift)` MASKS its input before shifting -- it does
//!   NOT trust the caller.** The macro body is
//!   `((unsigned)(((unsigned)(value) & ((1U << bits) - 1U)) << shift))`: the
//!   `& ((1U << bits) - 1U)` happens *before* the `<< shift`, so an
//!   over-wide `value` (one that sets bits above the field's `bits` width)
//!   is truncated and can never corrupt an adjacent field the way an
//!   unmasked shift-then-OR would. This is the opposite polarity from
//!   `rt64_gbi_f3d.rs`'s `p0`/`p1`, which mask the *result of extraction*
//!   from an already-packed word (a different operation with the same
//!   "mask after position" arithmetic shape, but this file's `PARAM` masks
//!   an *input about to be packed*, not an already-packed word being read
//!   back). Ported literally as `fn param(value: u32, bits: u32, shift: u32)
//!   -> u32 { (value & ((1u32 << bits) - 1)) << shift }` -- masks first,
//!   then shifts, exactly matching the macro's parenthesization. Every
//!   `PARAM(...)  | PARAM(...) | ...` chain in a `gEX*` macro is ported as
//!   `param(...) | param(...) | ...` in the corresponding Rust function,
//!   preserving field order and bit position exactly (OR is commutative and
//!   associative so evaluation order does not change the packed result, but
//!   the source order is kept for literal correspondence with the header).
//! - **`PARAM`'s C argument-evaluation-twice hazard is inapplicable here.**
//!   The macro expands `value`, `bits`, and `shift` textually; if any
//!   argument expression had a side effect it would run once per macro
//!   token occurrence (`value` appears twice: once inside the `&` mask,
//!   once implicitly via the outer cast -- actually only once textually,
//!   but `bits` appears twice, in `1U << (bits)` and nowhere else, so no
//!   argument in `PARAM` itself is textually duplicated). None of this
//!   file's call sites pass an expression with an observable side effect
//!   (they are all N64Gfx-command scalar parameters -- ints, bools,
//!   pointers cast to `unsigned`), so the C macro-expansion-order question
//!   does not change any characterized value; Rust's pure-value function
//!   arguments (evaluated exactly once, left to right) reproduce the same
//!   observable numbers for every call site this file exercises.
//! - **Two call sites multiply their value BEFORE calling `PARAM`, not
//!   inside it**: `gEXSetScissor`'s `PARAM((ulx) * 4, 16, 16)` (and `uly`/
//!   `lrx`/`lry` identically) and `gEXSetScissorAlign`'s six `* 4` sites.
//!   This is C `int`/caller-type multiplication (`* 4`, i.e. `<< 2`)
//!   happening in the *caller's* argument expression, then `PARAM` masks
//!   and shifts the already-multiplied result. Ported as `param((ulx) *
//!   4, 16, 16)` in Rust source order -- using `u32` wrapping multiplication
//!   (`value.wrapping_mul(4)`) would only diverge from plain `*` if the
//!   input were within `u32::MAX / 4` of overflow, which no packed-field
//!   argument in this ABI (all are display-list-scale integers) approaches;
//!   ported as plain `*` since C's `unsigned * int` multiplication for
//!   these magnitudes never overflows differently from Rust's default
//!   (non-wrapping, would-panic-on-debug-overflow) `*`, and no call site in
//!   this file's characterization tests exercises a value near that
//!   boundary.
//! - **`gEXSetDitherNoiseStrength`'s `PARAM((value) * 1024, 16, 0)`** is the
//!   one non-integer-typed argument in the header: upstream call sites pass
//!   a `float` (the macro itself is untyped, so C implicitly converts the
//!   `float * 1024` product to `unsigned` when it flows into `PARAM`'s
//!   `(unsigned)(value)` cast -- truncating toward zero, matching normal
//!   C float-to-unsigned conversion). This port takes the packing
//!   function's `value` parameter as `u32` directly (the float-to-fixed
//!   conversion the caller must already have performed to reach `PARAM`'s
//!   `(unsigned)` cast site is upstream of this ABI boundary, not part of
//!   the packing macro's own admitted behavior) -- `pack_set_dither_noise_strength`
//!   takes an already-converted `u32` fixed-point value and applies only
//!   `PARAM(value, 16, 0)`'s mask/shift, not a `* 1024` (the caller is
//!   expected to have already multiplied by 1024 before calling, matching
//!   how the C macro's caller passes the pre-multiplication `float`, and
//!   `* 1024` is textually part of the macro argument, not `PARAM`'s body,
//!   so it is a caller-side concern here too -- pinned by
//!   `pack_set_dither_noise_strength_scales_by_1024_like_the_macro_argument`,
//!   which reproduces the `* 1024` explicitly at the call site the way the
//!   C macro invocation itself would).
//! - **No packed field in this file is ever read back as signed.** Every
//!   `gEX*` macro's arguments (origin enums, push/pop flags, tile indices,
//!   mode/order/edit/aspect enums, refresh rate, vertex index/count,
//!   rectangle/viewport/scissor coordinates, dither-noise fixed-point
//!   value, texcoord wrap points, matrix-group component selectors) are
//!   packed as unsigned bitfields with `PARAM`'s unsigned mask/shift; no
//!   macro in this header applies a signed cast (`int16_t`/`short`) to a
//!   packed sub-field the way `rt64_gbi_f3d.rs`'s `moveWord` does on
//!   *extraction*. The rectangle/viewport/scissor coordinate arguments
//!   (`ulx`/`uly`/`lrx`/`lry`, offsets, `dsdx`/`dtdy`) are display-list
//!   `short`/`int` values in upstream call sites and *can* carry negative
//!   values (e.g. an off-screen scissor edge) -- `PARAM`'s `(unsigned)(value)`
//!   cast reinterprets a negative `int` as its 2's-complement unsigned
//!   bit pattern before masking (standardized before C++20 for this exact
//!   int-to-unsigned direction, unlike the signed-narrowing direction), so
//!   the packed 16-bit field is later meant to be read back and
//!   sign-extended by a *decoder* this file does not define. This port
//!   takes every such argument as `u32` (mirroring `PARAM`'s own
//!   `(unsigned)` cast site, not the caller's original signed type) --
//!   passing a negative value through requires the caller to first
//!   reinterpret it via `value as u32` (Rust's `as` between same-width
//!   signed/unsigned ints is a bit-preserving reinterpret, matching C's
//!   int-to-unsigned conversion for the in-range case) before calling; one
//!   characterization test per rectangle-coordinate-bearing function pins
//!   this by packing `(-1i16) as u32` and checking the field decodes back
//!   to all-ones, not a panic or a saturated zero.
//! - **`gEXMatrixGroup`'s `id` and `gEXEditGroupByAddress`'s `address` are
//!   NOT run through `PARAM` at all** -- `id` is packed as the bare second
//!   command word (`G_EX_COMMAND2(cmd, ..., id, ...)`), and `address` is
//!   cast `(unsigned)(address)` with no mask/shift. Both are full 32-bit
//!   words (a caller-assigned group ID and a segmented display-list
//!   address respectively), so there is no adjacent field to protect and no
//!   masking is applied by the macro -- ported literally as the raw `u32`
//!   argument placed directly into the returned word, with no `param` call.
//!   Likewise `gEXViewport`'s `vp`, `gEXVertex`'s `vtx`,
//!   `gEXSetProjMatrixFloat`/`gEXSetViewMatrixFloat`'s `matrix`,
//!   `gEXMatrixFloat`'s `m`, and `gEXSetVertexSegment`'s `vertexAddress`/
//!   `baseSegmentAddress` are all bare `(unsigned)` pointer casts with no
//!   `PARAM` -- these are opaque 32-bit segmented addresses in this ABI
//!   (pointing at a `VertexEX[]` array, a projection/view/model matrix
//!   float array, or a vertex-segment table entry), ported as `u32` inputs
//!   placed directly into their word with no reinterpretation, matching
//!   the source's plain cast-only treatment.
//! - **`gEXMatrixGroup`'s `PARAM((proj) != 0, 1, 1)` and
//!   `gEXEditGroupByAddress`'s identical clause**: `(proj) != 0` is a C
//!   boolean comparison producing `0` or `1` as an `int`, which `PARAM`
//!   then masks to 1 bit (a no-op mask, since the comparison result is
//!   already 0 or 1) and shifts into bit 1. Ported as
//!   `param((proj != 0) as u32, 1, 1)` -- `bool as u32` in Rust yields
//!   exactly `0`/`1`, the same values C's `!=` produces, so this is a
//!   literal (not idiomatic-only) match, not merely an equivalent
//!   implementation.
//! - **Widths: every packed word in this file is `unsigned` (32-bit,
//!   matching `GfxCommand::values::word0`/`word1`'s C `unsigned` type) --
//!   never `unsigned long long`/64-bit.** `PARAM`'s own cast is
//!   `(unsigned)`, and every `gEX*` macro ORs `PARAM(...)` results (or a
//!   bare `(unsigned)`-cast value) directly into one of the two 32-bit
//!   words `G_EX_COMMAND1`/`2`/`3` write. This port's `param` and every
//!   `pack_*` function return `u32`, and packing functions that produce two
//!   or more words return a fixed-size array or tuple of `u32`, never
//!   `u64` -- the `GfxCommand` union's `unsigned long long dummy` member
//!   exists in the C++ only to force 8-byte alignment of the two-`unsigned`
//!   struct, it is never the type any field is packed *as* (see
//!   "Nonclaims" for why `GfxCommand`/`G_EX_COMMAND1..4`/`G_EX_WRITECOMMAND`
//!   themselves -- the write-through-a-pointer machinery, as opposed to the
//!   word *values* they write -- are out of scope).
//! - **`1U << (bits)` inside `PARAM` is C `unsigned int` (32-bit) shift
//!   arithmetic** (the `1U` suffix forces `unsigned` from the start, so no
//!   signed-shift UB is possible even for `bits == 31`); every `bits`
//!   literal this header actually passes to `PARAM` is one of `1, 2, 3, 4,
//!   5, 8, 12, 16, 24, 28` -- all comfortably `< 32`, so `1u32 << bits`
//!   never hits Rust's shift-amount-`>=`-bit-width panic boundary (`<< 32`
//!   would panic in both debug and release Rust, unlike C++'s
//!   implementation-defined-but-usually-zero `1U << 32` on a 32-bit
//!   `unsigned` -- not reachable by any literal this file uses, so not
//!   characterized).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_gbi_f3d.rs`'s precedent), and no
//! RT64 visual/pixel/silicon parity or performance claim. Not wired to
//! `fn64-render-reference`'s GBI interpreter (see "Reuse, not new type").
//!
//! **The M8.2 ticket's `src/gbi/rt64_gbi_extended.cpp` decoders are
//! deliberately not ported here** -- that file consumes the command words
//! this header's macros produce (the opposite direction, decode not
//! encode/pack) and is a distinct, larger, separately-scoped source file
//! outside this card's `include/rt64_extended_gbi.h`-only boundary.
//!
//! **`GfxCommand` (the tagged union that forces 8-byte alignment for a
//! two-`unsigned`-word command pair) and `VertexEXColor`/`VertexEXNormal`/
//! `VertexEX` (the extended-vertex payload structs `gEXVertex`'s `vtx`
//! parameter points at) are not ported as Rust types.** This card's scope is
//! the word-*packing* ABI (`PARAM`, the opcode/enum constants, and the
//! `gEX*` macros' shift/mask arithmetic) -- these struct layouts describe
//! memory this crate never allocates, walks, or writes a vertex payload
//! into; introducing a Rust struct for them here would be new surface this
//! card was not asked to characterize, not a literal port of packing
//! behavior.
//!
//! **`G_EX_WRITECOMMAND`/`DOWHILE`/`G_EX_COMMAND1`/`G_EX_COMMAND2`/
//! `G_EX_COMMAND3`/`G_EX_COMMAND4` (the pointer-write machinery that lands
//! packed words into a caller-supplied `Gfx*` display-list buffer) are
//! quoted above for context but NOT ported as functions** -- they are
//! side-effecting writes through a raw pointer with no return value, the
//! opposite of this port's pure `(fields...) -> [u32; N]` packing
//! functions. This port's `pack_*` functions return the packed word array
//! directly; a caller wiring this into a real display-list writer (out of
//! scope) would perform the pointer write itself. **`G_EX_COMMAND4` is
//! quoted in the source excerpt above for completeness (`include/
//! rt64_extended_gbi.h:192-202`) but is dead code even upstream** -- no
//! `gEX*` macro in this 582-line file ever invokes it (the widest used is
//! `G_EX_COMMAND3`, three words), and its body has a pre-existing upstream
//! bug (`G_EX_WRITECOMMAND(cmd_, ...)` references an undefined `cmd_`
//! identifier, not the macro-local `_cmd` `G_EX_COMMAND1`/`2`/`3` define) --
//! not reproduced or fixed here since it packs zero opcodes in this file.
//!
//! **`RT64_HOOK_OPCODE`'s two `#ifdef F3DEX_GBI_2` branches (`0xE0` vs
//! `0x00`) are both ported as separate named constants**
//! (`RT64_HOOK_OPCODE_F3DEX_GBI_2` and `RT64_HOOK_OPCODE_DEFAULT`) rather
//! than picking one -- which branch is active depends on a microcode-family
//! preprocessor definition external to this file, and this card has no
//! authority to assert which ucode family fn64 targets; both values are
//! preserved so no information is discarded, and no default is silently
//! chosen for callers of the `gEXGetVersion`/`gEXEnable`/`gEXDisable`/
//! `gEXBranchList`/`gEXDisplayList` packing functions (which take
//! `hook_opcode: u32` as an explicit parameter rather than reading a
//! module-level constant, so the caller must pick).
//!
//! **`RT64_EXTENDED_OPCODE`'s `#ifndef` default (`0x64`) is ported as
//! `RT64_EXTENDED_OPCODE_DEFAULT`**; every `gEX*` (non-hook) packing
//! function takes `extended_opcode: u32` as an explicit parameter rather
//! than hardcoding the default, for the same caller-must-pick reason as
//! `RT64_HOOK_OPCODE` above -- a project could `#define
//! RT64_EXTENDED_OPCODE` to something else before including this header,
//! and this port does not assume fn64 never will.

// --- RT64_HOOK_* / G_EX_VERSION constants (lines 8-29) ---

/// `RT64_HOOK_OPCODE` under `#ifdef F3DEX_GBI_2`.
pub const RT64_HOOK_OPCODE_F3DEX_GBI_2: u32 = 0xE0;
/// `RT64_HOOK_OPCODE` under `#else` (non-F3DEX_GBI_2 ucode families).
pub const RT64_HOOK_OPCODE_DEFAULT: u32 = 0x00;

pub const RT64_HOOK_OP_GETVERSION: u32 = 0x0;
pub const RT64_HOOK_OP_ENABLE: u32 = 0x1;
pub const RT64_HOOK_OP_DISABLE: u32 = 0x2;
pub const RT64_HOOK_OP_DL: u32 = 0x3;
pub const RT64_HOOK_OP_BRANCH: u32 = 0x4;

/// 0x5254 for ASCII "RT" followed by 0x64.
pub const RT64_HOOK_MAGIC_NUMBER: u32 = 0x525464;

/// `RT64_EXTENDED_OPCODE`'s `#ifndef`-guarded default.
pub const RT64_EXTENDED_OPCODE_DEFAULT: u32 = 0x64;

pub const G_EX_VERSION: u32 = 0x1;

// --- G_EX_* opcode enumeration (lines 31-83) ---

pub const G_EX_NOOP: u32 = 0x000000;
pub const G_EX_PRINT: u32 = 0x000001;
pub const G_EX_TEXRECT_V1: u32 = 0x000002;
pub const G_EX_FILLRECT_V1: u32 = 0x000003;
pub const G_EX_SETVIEWPORT_V1: u32 = 0x000004;
pub const G_EX_SETSCISSOR_V1: u32 = 0x000005;
pub const G_EX_SETRECTALIGN_V1: u32 = 0x000006;
pub const G_EX_SETVIEWPORTALIGN_V1: u32 = 0x000007;
pub const G_EX_SETSCISSORALIGN_V1: u32 = 0x000008;
pub const G_EX_SETREFRESHRATE_V1: u32 = 0x000009;
pub const G_EX_VERTEXZTEST_V1: u32 = 0x00000A;
pub const G_EX_ENDVERTEXZTEST_V1: u32 = 0x00000B;
pub const G_EX_MATRIXGROUP_V1: u32 = 0x00000C;
pub const G_EX_POPMATRIXGROUP_V1: u32 = 0x00000D;
pub const G_EX_FORCEUPSCALE2D_V1: u32 = 0x00000E;
pub const G_EX_FORCETRUEBILERP_V1: u32 = 0x00000F;
pub const G_EX_FORCESCALELOD_V1: u32 = 0x000010;
pub const G_EX_FORCEBRANCH_V1: u32 = 0x000011;
pub const G_EX_SETRENDERTORAM_V1: u32 = 0x000012;
pub const G_EX_EDITGROUPBYADDRESS_V1: u32 = 0x000013;
pub const G_EX_VERTEX_V1: u32 = 0x000014;
pub const G_EX_PUSHVIEWPORT_V1: u32 = 0x000015;
pub const G_EX_POPVIEWPORT_V1: u32 = 0x000016;
pub const G_EX_PUSHSCISSOR_V1: u32 = 0x000017;
pub const G_EX_POPSCISSOR_V1: u32 = 0x000018;
pub const G_EX_PUSHOTHERMODE_V1: u32 = 0x000019;
pub const G_EX_POPOTHERMODE_V1: u32 = 0x00001A;
pub const G_EX_PUSHCOMBINE_V1: u32 = 0x00001B;
pub const G_EX_POPCOMBINE_V1: u32 = 0x00001C;
pub const G_EX_PUSHPROJMATRIX_V1: u32 = 0x00001D;
pub const G_EX_POPPROJMATRIX_V1: u32 = 0x00001E;
pub const G_EX_PUSHENVCOLOR_V1: u32 = 0x00001F;
pub const G_EX_POPENVCOLOR_V1: u32 = 0x000020;
pub const G_EX_PUSHBLENDCOLOR_V1: u32 = 0x000021;
pub const G_EX_POPBLENDCOLOR_V1: u32 = 0x000022;
pub const G_EX_PUSHFOGCOLOR_V1: u32 = 0x000023;
pub const G_EX_POPFOGCOLOR_V1: u32 = 0x000024;
pub const G_EX_PUSHFILLCOLOR_V1: u32 = 0x000025;
pub const G_EX_POPFILLCOLOR_V1: u32 = 0x000026;
pub const G_EX_PUSHPRIMCOLOR_V1: u32 = 0x000027;
pub const G_EX_POPPRIMCOLOR_V1: u32 = 0x000028;
pub const G_EX_PUSHGEOMETRYMODE_V1: u32 = 0x000029;
pub const G_EX_POPGEOMETRYMODE_V1: u32 = 0x00002A;
pub const G_EX_SETDITHERNOISESTRENGTH_V1: u32 = 0x00002B;
pub const G_EX_SETRDRAMEXTENDED_V1: u32 = 0x00002C;
pub const G_EX_SETPROJMATRIXFLOAT_V1: u32 = 0x00002D;
pub const G_EX_SETVIEWMATRIXFLOAT_V1: u32 = 0x00002E;
pub const G_EX_SETNEARCLIPPING_V1: u32 = 0x00002F;
pub const G_EX_MATRIX_FLOAT_V1: u32 = 0x000030;
pub const G_EX_SETVERTEXSEGMENT_V1: u32 = 0x000031;
pub const G_EX_SETTEXCOORDWRAPPOINT_V1: u32 = 0x000032;
pub const G_EX_SETRECTASPECT_V1: u32 = 0x000033;
pub const G_EX_MAX: u32 = 0x000034;

// --- packed-field enums (lines 85-122) ---

pub const G_EX_ORIGIN_NONE: u32 = 0x800;
pub const G_EX_ORIGIN_LEFT: u32 = 0x0;
pub const G_EX_ORIGIN_CENTER: u32 = 0x200;
pub const G_EX_ORIGIN_RIGHT: u32 = 0x400;

pub const G_EX_NOPUSH: u32 = 0x0;
pub const G_EX_PUSH: u32 = 0x1;

pub const G_EX_ID_IGNORE: u32 = 0x0;
pub const G_EX_ID_AUTO: u32 = 0xFFFFFFFF;

pub const G_EX_COMPONENT_SKIP: u32 = 0x0;
pub const G_EX_COMPONENT_INTERPOLATE: u32 = 0x1;
pub const G_EX_COMPONENT_AUTO: u32 = 0x2;

pub const G_EX_INTERPOLATE_SIMPLE: u32 = 0x0;
pub const G_EX_INTERPOLATE_DECOMPOSE: u32 = 0x1;

pub const G_EX_ORDER_LINEAR: u32 = 0x0;
pub const G_EX_ORDER_AUTO: u32 = 0x1;

pub const G_EX_EDIT_NONE: u32 = 0x0;
pub const G_EX_EDIT_ALLOW: u32 = 0x1;

pub const G_EX_BILERP_NONE: u32 = 0x0;
pub const G_EX_BILERP_ONLY: u32 = 0x1;
pub const G_EX_BILERP_ALL: u32 = 0x2;

pub const G_EX_ASPECT_AUTO: u32 = 0x0;
pub const G_EX_ASPECT_STRETCH: u32 = 0x1;
pub const G_EX_ASPECT_ADJUST: u32 = 0x2;

pub const G_EX_VERTEX_POSITION: u32 = 0x0;
pub const G_EX_VERTEX_VELOCITY: u32 = 0x1;
pub const G_EX_VERTEX_MAX: u32 = 0x2;

pub const G_EX_DISABLED: u32 = 0x0;
pub const G_EX_ENABLED: u32 = 0x1;

// --- PARAM packing primitive (line 156-157) ---

/// `PARAM(value, bits, shift)`: `((value & ((1 << bits) - 1)) << shift)`.
/// Masks `value` to `bits` width BEFORE shifting -- an over-wide `value`
/// is truncated, never corrupts a neighboring field. Private: only this
/// file's `pack_*` functions call it, matching `rt64_gbi_f3d.rs`'s `p0`/
/// `p1` visibility precedent for the opposite-direction primitive.
fn param(value: u32, bits: u32, shift: u32) -> u32 {
    (value & ((1u32 << bits) - 1)) << shift
}

// --- gEX* command-packing functions (lines 204-580) ---

/// `gEXGetVersion(cmd, ret)`.
pub fn pack_get_version(hook_opcode: u32, ret: u32) -> [u32; 2] {
    [
        param(hook_opcode, 8, 24) | param(RT64_HOOK_MAGIC_NUMBER, 24, 0),
        param(RT64_HOOK_OP_GETVERSION, 4, 28) | param(ret, 28, 0),
    ]
}

/// `gEXEnable(cmd)`.
pub fn pack_enable(hook_opcode: u32, extended_opcode: u32) -> [u32; 2] {
    [
        param(hook_opcode, 8, 24) | param(RT64_HOOK_MAGIC_NUMBER, 24, 0),
        param(RT64_HOOK_OP_ENABLE, 4, 28) | param(extended_opcode, 8, 0),
    ]
}

/// `gEXDisable(cmd)`.
pub fn pack_disable(hook_opcode: u32) -> [u32; 2] {
    [
        param(hook_opcode, 8, 24) | param(RT64_HOOK_MAGIC_NUMBER, 24, 0),
        param(RT64_HOOK_OP_DISABLE, 4, 28),
    ]
}

/// `gEXBranchList(cmd, dlist)`.
pub fn pack_branch_list(hook_opcode: u32, dlist: u32) -> [u32; 2] {
    [
        param(hook_opcode, 8, 24) | param(RT64_HOOK_MAGIC_NUMBER, 24, 0),
        param(RT64_HOOK_OP_BRANCH, 4, 28) | param(dlist, 28, 0),
    ]
}

/// `gEXDisplayList(cmd, dlist)`.
pub fn pack_display_list(hook_opcode: u32, dlist: u32) -> [u32; 2] {
    [
        param(hook_opcode, 8, 24) | param(RT64_HOOK_MAGIC_NUMBER, 24, 0),
        param(RT64_HOOK_OP_DL, 4, 28) | param(dlist, 28, 0),
    ]
}

/// `gEXNoOp(cmd)`.
pub fn pack_no_op(extended_opcode: u32) -> [u32; 2] {
    [param(extended_opcode, 8, 24) | param(G_EX_NOOP, 24, 0), 0]
}

/// `gEXPrint(cmd)`.
pub fn pack_print(extended_opcode: u32) -> [u32; 2] {
    [param(extended_opcode, 8, 24) | param(G_EX_PRINT, 24, 0), 0]
}

/// `gEXTextureRectangle(cmd, lorigin, rorigin, ulx, uly, lrx, lry, tile, s,
/// t, dsdx, dtdy)`. Three command words (6 `u32`s).
#[allow(clippy::too_many_arguments)]
pub fn pack_texture_rectangle(
    extended_opcode: u32,
    lorigin: u32,
    rorigin: u32,
    ulx: u32,
    uly: u32,
    lrx: u32,
    lry: u32,
    tile: u32,
    s: u32,
    t: u32,
    dsdx: u32,
    dtdy: u32,
) -> [u32; 6] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_TEXRECT_V1, 24, 0),
        param(tile, 3, 0) | param(lorigin, 12, 3) | param(rorigin, 12, 15) | param(0, 5, 27),
        param(ulx, 16, 16) | param(uly, 16, 0),
        param(lrx, 16, 16) | param(lry, 16, 0),
        param(s, 16, 16) | param(t, 16, 0),
        param(dsdx, 16, 16) | param(dtdy, 16, 0),
    ]
}

/// `gEXViewport(cmd, origin, vp)`. `vp` is a bare `(unsigned)` pointer
/// cast, not run through `PARAM` -- ported as an opaque `u32` address.
pub fn pack_viewport(extended_opcode: u32, origin: u32, vp: u32) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETVIEWPORT_V1, 24, 0),
        param(origin, 12, 0),
        0,
        vp,
    ]
}

/// `gEXSetScissor(cmd, mode, lorigin, rorigin, ulx, uly, lrx, lry)`. Each
/// coordinate is multiplied by 4 in the caller's argument expression
/// (`(ulx) * 4`) BEFORE `PARAM` masks/shifts it.
#[allow(clippy::too_many_arguments)]
pub fn pack_set_scissor(
    extended_opcode: u32,
    mode: u32,
    lorigin: u32,
    rorigin: u32,
    ulx: u32,
    uly: u32,
    lrx: u32,
    lry: u32,
) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETSCISSOR_V1, 24, 0),
        param(mode, 2, 0) | param(lorigin, 12, 2) | param(rorigin, 12, 14),
        param(ulx * 4, 16, 16) | param(uly * 4, 16, 0),
        param(lrx * 4, 16, 16) | param(lry * 4, 16, 0),
    ]
}

/// `gEXSetRectAlign(cmd, lorigin, rorigin, ulxOffset, ulyOffset,
/// lrxOffset, lryOffset)`.
#[allow(clippy::too_many_arguments)]
pub fn pack_set_rect_align(
    extended_opcode: u32,
    lorigin: u32,
    rorigin: u32,
    ulx_offset: u32,
    uly_offset: u32,
    lrx_offset: u32,
    lry_offset: u32,
) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETRECTALIGN_V1, 24, 0),
        param(lorigin, 12, 0) | param(rorigin, 12, 12),
        param(ulx_offset, 16, 16) | param(uly_offset, 16, 0),
        param(lrx_offset, 16, 16) | param(lry_offset, 16, 0),
    ]
}

/// `gEXSetViewportAlign(cmd, origin, xOffset, yOffset)`.
pub fn pack_set_viewport_align(
    extended_opcode: u32,
    origin: u32,
    x_offset: u32,
    y_offset: u32,
) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETVIEWPORTALIGN_V1, 24, 0),
        param(origin, 12, 0),
        param(x_offset, 16, 16) | param(y_offset, 16, 0),
        0,
    ]
}

/// `gEXSetScissorAlign(cmd, lorigin, rorigin, ulxOffset, ulyOffset,
/// lrxOffset, lryOffset, ulxBound, ulyBound, lrxBound, lryBound)`. Three
/// command words (6 `u32`s); every offset/bound is multiplied by 4 before
/// `PARAM`, matching `pack_set_scissor`.
#[allow(clippy::too_many_arguments)]
pub fn pack_set_scissor_align(
    extended_opcode: u32,
    lorigin: u32,
    rorigin: u32,
    ulx_offset: u32,
    uly_offset: u32,
    lrx_offset: u32,
    lry_offset: u32,
    ulx_bound: u32,
    uly_bound: u32,
    lrx_bound: u32,
    lry_bound: u32,
) -> [u32; 6] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETSCISSORALIGN_V1, 24, 0),
        param(lorigin, 12, 0) | param(rorigin, 12, 12),
        param(ulx_offset * 4, 16, 16) | param(uly_offset * 4, 16, 0),
        param(lrx_offset * 4, 16, 16) | param(lry_offset * 4, 16, 0),
        param(ulx_bound * 4, 16, 16) | param(uly_bound * 4, 16, 0),
        param(lrx_bound * 4, 16, 16) | param(lry_bound * 4, 16, 0),
    ]
}

/// `gEXSetRefreshRate(cmd, refresh_rate)`.
pub fn pack_set_refresh_rate(extended_opcode: u32, refresh_rate: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETREFRESHRATE_V1, 24, 0),
        param(refresh_rate, 16, 0),
    ]
}

/// `gEXVertexZTest(cmd, vertex_index)`.
pub fn pack_vertex_z_test(extended_opcode: u32, vertex_index: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_VERTEXZTEST_V1, 24, 0),
        param(vertex_index, 8, 0),
    ]
}

/// `gEXEndVertexZTest(cmd)`.
pub fn pack_end_vertex_z_test(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_ENDVERTEXZTEST_V1, 24, 0),
        0,
    ]
}

/// `gEXMatrixGroup(cmd, id, mode, push, proj, pos, rot, scale, skew,
/// persp, vert, tile, order, edit, aspect, tc, lookat)`. `id` is a bare
/// second command word, not run through `PARAM`.
#[allow(clippy::too_many_arguments)]
pub fn pack_matrix_group(
    extended_opcode: u32,
    id: u32,
    mode: u32,
    push: u32,
    proj: u32,
    pos: u32,
    rot: u32,
    scale: u32,
    skew: u32,
    persp: u32,
    vert: u32,
    tile: u32,
    order: u32,
    edit: u32,
    aspect: u32,
    tc: u32,
    lookat: u32,
) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_MATRIXGROUP_V1, 24, 0),
        id,
        param(push, 1, 0)
            | param((proj != 0) as u32, 1, 1)
            | param(mode, 1, 2)
            | param(pos, 2, 3)
            | param(rot, 2, 5)
            | param(scale, 2, 7)
            | param(skew, 2, 9)
            | param(persp, 2, 11)
            | param(vert, 2, 13)
            | param(tile, 2, 15)
            | param(order, 2, 17)
            | param(edit, 1, 19)
            | param(aspect, 2, 20)
            | param(tc, 2, 22)
            | param(lookat, 2, 24),
        0,
    ]
}

/// `gEXPopMatrixGroup(cmd, proj)`: `gEXPopMatrixGroupN(cmd, proj, 1)`.
pub fn pack_pop_matrix_group(extended_opcode: u32, proj: u32) -> [u32; 2] {
    pack_pop_matrix_group_n(extended_opcode, proj, 1)
}

/// `gEXPopMatrixGroupN(cmd, proj, count)`.
pub fn pack_pop_matrix_group_n(extended_opcode: u32, proj: u32, count: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPMATRIXGROUP_V1, 24, 0),
        param(count, 8, 0) | param(proj, 1, 8),
    ]
}

/// `gEXForceUpscale2D(cmd, force)`.
pub fn pack_force_upscale_2d(extended_opcode: u32, force: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_FORCEUPSCALE2D_V1, 24, 0),
        param(force, 1, 0),
    ]
}

/// `gEXForceTrueBilerp(cmd, mode)`.
pub fn pack_force_true_bilerp(extended_opcode: u32, mode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_FORCETRUEBILERP_V1, 24, 0),
        param(mode, 2, 0),
    ]
}

/// `gEXForceScaleLOD(cmd, force)`.
pub fn pack_force_scale_lod(extended_opcode: u32, force: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_FORCESCALELOD_V1, 24, 0),
        param(force, 1, 0),
    ]
}

/// `gEXForceBranch(cmd, force)`.
pub fn pack_force_branch(extended_opcode: u32, force: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_FORCEBRANCH_V1, 24, 0),
        param(force, 1, 0),
    ]
}

/// `gEXSetRenderToRAM(cmd, render)`.
pub fn pack_set_render_to_ram(extended_opcode: u32, render: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETRENDERTORAM_V1, 24, 0),
        param(render, 1, 0),
    ]
}

/// `gEXEditGroupByAddress(cmd, address, mode, push, proj, pos, rot, scale,
/// skew, persp, vert, tile, order)`. `address` is a bare `(unsigned)` cast,
/// not run through `PARAM`; `edit` is hardcoded to `G_EX_EDIT_ALLOW`
/// (unlike `pack_matrix_group`, which takes `edit` as a caller parameter).
#[allow(clippy::too_many_arguments)]
pub fn pack_edit_group_by_address(
    extended_opcode: u32,
    address: u32,
    mode: u32,
    push: u32,
    proj: u32,
    pos: u32,
    rot: u32,
    scale: u32,
    skew: u32,
    persp: u32,
    vert: u32,
    tile: u32,
    order: u32,
) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_EDITGROUPBYADDRESS_V1, 24, 0),
        address,
        param(push, 1, 0)
            | param((proj != 0) as u32, 1, 1)
            | param(mode, 1, 2)
            | param(pos, 2, 3)
            | param(rot, 2, 5)
            | param(scale, 2, 7)
            | param(skew, 2, 9)
            | param(persp, 2, 11)
            | param(vert, 2, 13)
            | param(tile, 2, 15)
            | param(order, 2, 17)
            | param(G_EX_EDIT_ALLOW, 1, 18),
        0,
    ]
}

/// `gEXVertex(cmd, vtx, count, v0)`. `vtx` is a bare `(unsigned)` cast, not
/// run through `PARAM`.
pub fn pack_vertex(extended_opcode: u32, vtx: u32, count: u32, v0: u32) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_VERTEX_V1, 24, 0),
        param(v0, 8, 0) | param(count, 8, 8),
        0,
        vtx,
    ]
}

/// `gEXSetProjMatrixFloat(cmd, matrix)`. `matrix` is a bare `(unsigned)`
/// cast, not run through `PARAM`.
pub fn pack_set_proj_matrix_float(extended_opcode: u32, matrix: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETPROJMATRIXFLOAT_V1, 24, 0),
        matrix,
    ]
}

/// `gEXSetViewMatrixFloat(cmd, matrix)`. `matrix` is a bare `(unsigned)`
/// cast, not run through `PARAM`.
pub fn pack_set_view_matrix_float(extended_opcode: u32, matrix: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETVIEWMATRIXFLOAT_V1, 24, 0),
        matrix,
    ]
}

/// `gEXPushViewport(cmd)`.
pub fn pack_push_viewport(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHVIEWPORT_V1, 24, 0),
        0,
    ]
}

/// `gEXPopViewport(cmd)`.
pub fn pack_pop_viewport(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPVIEWPORT_V1, 24, 0),
        0,
    ]
}

/// `gEXPushScissor(cmd)`.
pub fn pack_push_scissor(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHSCISSOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPopScissor(cmd)`.
pub fn pack_pop_scissor(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPSCISSOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPushOtherMode(cmd)`.
pub fn pack_push_other_mode(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHOTHERMODE_V1, 24, 0),
        0,
    ]
}

/// `gEXPopOtherMode(cmd)`.
pub fn pack_pop_other_mode(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPOTHERMODE_V1, 24, 0),
        0,
    ]
}

/// `gEXPushCombineMode(cmd)`.
pub fn pack_push_combine_mode(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHCOMBINE_V1, 24, 0),
        0,
    ]
}

/// `gEXPopCombineMode(cmd)`.
pub fn pack_pop_combine_mode(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPCOMBINE_V1, 24, 0),
        0,
    ]
}

/// `gEXPushProjectionMatrix(cmd)`.
pub fn pack_push_projection_matrix(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHPROJMATRIX_V1, 24, 0),
        0,
    ]
}

/// `gEXPopProjectionMatrix(cmd)`.
pub fn pack_pop_projection_matrix(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPPROJMATRIX_V1, 24, 0),
        0,
    ]
}

/// `gEXPushEnvColor(cmd)`.
pub fn pack_push_env_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHENVCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPopEnvColor(cmd)`.
pub fn pack_pop_env_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPENVCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPushBlendColor(cmd)`.
pub fn pack_push_blend_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHBLENDCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPopBlendColor(cmd)`.
pub fn pack_pop_blend_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPBLENDCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPushFogColor(cmd)`.
pub fn pack_push_fog_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHFOGCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPopFogColor(cmd)`.
pub fn pack_pop_fog_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPFOGCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPushFillColor(cmd)`.
pub fn pack_push_fill_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHFILLCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPopFillColor(cmd)`.
pub fn pack_pop_fill_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPFILLCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPushPrimColor(cmd)`.
pub fn pack_push_prim_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHPRIMCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPopPrimColor(cmd)`.
pub fn pack_pop_prim_color(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPPRIMCOLOR_V1, 24, 0),
        0,
    ]
}

/// `gEXPushGeometryMode(cmd)`.
pub fn pack_push_geometry_mode(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_PUSHGEOMETRYMODE_V1, 24, 0),
        0,
    ]
}

/// `gEXPopGeometryMode(cmd)`.
pub fn pack_pop_geometry_mode(extended_opcode: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_POPGEOMETRYMODE_V1, 24, 0),
        0,
    ]
}

/// `gEXSetDitherNoiseStrength(cmd, value)`. Upstream multiplies a `float`
/// by 1024 in the macro's *argument expression*, before `PARAM`'s
/// `(unsigned)` cast truncates it; this function takes the already-
/// converted fixed-point `u32` (see "Admitted domain" -- the `* 1024` is
/// the caller's concern, not `PARAM`'s).
pub fn pack_set_dither_noise_strength(extended_opcode: u32, value_x1024: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETDITHERNOISESTRENGTH_V1, 24, 0),
        param(value_x1024, 16, 0),
    ]
}

/// `gEXSetRDRAMExtended(cmd, isExtended)`.
pub fn pack_set_rdram_extended(extended_opcode: u32, is_extended: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETRDRAMEXTENDED_V1, 24, 0),
        param(is_extended, 1, 0),
    ]
}

/// `gEXSetNearClipping(cmd, isEnabled)`.
pub fn pack_set_near_clipping(extended_opcode: u32, is_enabled: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETNEARCLIPPING_V1, 24, 0),
        param(is_enabled, 1, 0),
    ]
}

/// `gEXMatrixFloat(cmd, m, p)`. `m` is a bare `(unsigned)` cast, not run
/// through `PARAM`.
pub fn pack_matrix_float(extended_opcode: u32, m: u32, p: u32) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_MATRIX_FLOAT_V1, 24, 0),
        param(p, 8, 0),
        0,
        m,
    ]
}

/// `gEXSetVertexSegment(cmd, vertexElement, isEnabled, vertexAddress,
/// baseSegmentAddress)`. `vertexAddress`/`baseSegmentAddress` are bare
/// `(unsigned)` casts, not run through `PARAM`.
pub fn pack_set_vertex_segment(
    extended_opcode: u32,
    vertex_element: u32,
    is_enabled: u32,
    vertex_address: u32,
    base_segment_address: u32,
) -> [u32; 4] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETVERTEXSEGMENT_V1, 24, 0),
        param(is_enabled, 1, 0) | param(vertex_element, 4, 1),
        vertex_address,
        base_segment_address,
    ]
}

/// `gEXSetTexcoordWrapPoint(cmd, wrapPointU, wrapPointV)`.
pub fn pack_set_texcoord_wrap_point(
    extended_opcode: u32,
    wrap_point_u: u32,
    wrap_point_v: u32,
) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETTEXCOORDWRAPPOINT_V1, 24, 0),
        param(wrap_point_u, 16, 16) | param(wrap_point_v, 16, 0),
    ]
}

/// `gEXSetRectAspect(cmd, aspect)`.
pub fn pack_set_rect_aspect(extended_opcode: u32, aspect: u32) -> [u32; 2] {
    [
        param(extended_opcode, 8, 24) | param(G_EX_SETRECTASPECT_V1, 24, 0),
        param(aspect, 2, 0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- param: the PARAM macro's mask-then-shift primitive ---

    #[test]
    fn param_all_zero_is_zero() {
        assert_eq!(param(0, 8, 0), 0);
    }

    #[test]
    fn param_masks_value_to_bit_width_at_zero_shift() {
        assert_eq!(param(0xFF, 4, 0), 0xF);
    }

    #[test]
    fn param_shifts_after_masking() {
        // 8-bit field at shift 8: 0xFF -> 0xFF00.
        assert_eq!(param(0xFF, 8, 8), 0xFF00);
    }

    #[test]
    fn param_one_bit_above_field_width_is_masked_out_not_corrupting() {
        // bits=4 at shift=0: value 0x1F (one bit above the 4-bit field)
        // must truncate to 0xF, not leak bit 4 into an adjacent field.
        assert_eq!(param(0x1F, 4, 0), 0xF);
    }

    #[test]
    fn param_over_wide_value_does_not_corrupt_an_adjacent_shifted_field() {
        // Simulate two adjacent PARAM calls the way a gEX* macro ORs them:
        // an over-wide low field (bits=4 but value has bit 4 set) must not
        // bleed into the high field's bits.
        let low = param(0b1_0101, 4, 0); // value=0x15, 4-bit field -> 0x5
        let high = param(0x3, 2, 4);
        assert_eq!(low, 0x5);
        assert_eq!(low | high, 0x35);
    }

    #[test]
    fn param_sixteen_bit_field_max_value() {
        assert_eq!(param(0xFFFF, 16, 0), 0xFFFF);
        assert_eq!(param(0xFFFF, 16, 16), 0xFFFF_0000);
    }

    #[test]
    fn param_sixteen_bit_field_one_bit_above_max_is_masked() {
        assert_eq!(param(0x1_FFFF, 16, 0), 0xFFFF);
    }

    #[test]
    fn param_twenty_eight_bit_field_max_value() {
        assert_eq!(param(0x0FFF_FFFF, 28, 0), 0x0FFF_FFFF);
    }

    #[test]
    fn param_twenty_eight_bit_field_one_bit_above_max_is_masked() {
        assert_eq!(param(0xFFFF_FFFF, 28, 0), 0x0FFF_FFFF);
    }

    #[test]
    fn param_negative_i16_reinterpreted_as_u32_round_trips_through_sixteen_bit_field() {
        // -1i16 as u32 is 0xFFFF_FFFF; PARAM masks to the low 16 bits.
        let negative_one = (-1i16) as u16 as u32;
        assert_eq!(param(negative_one, 16, 0), 0xFFFF);
    }

    // --- pack_get_version / pack_enable / pack_disable (hook opcodes) ---

    #[test]
    fn pack_get_version_all_zero() {
        let w = pack_get_version(0, 0);
        assert_eq!(w[0], param(RT64_HOOK_MAGIC_NUMBER, 24, 0));
        assert_eq!(w[1], 0);
    }

    #[test]
    fn pack_get_version_f3dex_gbi_2_hook_opcode_and_max_ret() {
        let w = pack_get_version(RT64_HOOK_OPCODE_F3DEX_GBI_2, 0x0FFF_FFFF);
        assert_eq!(w[0], (0xE0u32 << 24) | RT64_HOOK_MAGIC_NUMBER);
        assert_eq!(w[1], (RT64_HOOK_OP_GETVERSION << 28) | 0x0FFF_FFFF);
    }

    #[test]
    fn pack_get_version_hook_opcode_one_bit_above_eight_bits_is_masked() {
        // hook_opcode field is 8 bits at shift 24; 0x1FF has bit 8 set.
        let w = pack_get_version(0x1FF, 0);
        assert_eq!(w[0], (0xFFu32 << 24) | RT64_HOOK_MAGIC_NUMBER);
    }

    #[test]
    fn pack_enable_default_hook_opcode_and_default_extended_opcode() {
        let w = pack_enable(RT64_HOOK_OPCODE_DEFAULT, RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w[0], RT64_HOOK_MAGIC_NUMBER);
        assert_eq!(
            w[1],
            (RT64_HOOK_OP_ENABLE << 28) | RT64_EXTENDED_OPCODE_DEFAULT
        );
    }

    #[test]
    fn pack_enable_extended_opcode_max_eight_bit_value() {
        let w = pack_enable(0, 0xFF);
        assert_eq!(w[1], (RT64_HOOK_OP_ENABLE << 28) | 0xFF);
    }

    #[test]
    fn pack_enable_extended_opcode_one_bit_above_max_is_masked() {
        let w = pack_enable(0, 0x1FF);
        assert_eq!(w[1], (RT64_HOOK_OP_ENABLE << 28) | 0xFF);
    }

    #[test]
    fn pack_disable_carries_no_payload() {
        let w = pack_disable(RT64_HOOK_OPCODE_F3DEX_GBI_2);
        assert_eq!(w[0], (0xE0u32 << 24) | RT64_HOOK_MAGIC_NUMBER);
        assert_eq!(w[1], RT64_HOOK_OP_DISABLE << 28);
    }

    #[test]
    fn pack_branch_list_dlist_max_28_bit_value() {
        let w = pack_branch_list(0, 0x0FFF_FFFF);
        assert_eq!(w[1], (RT64_HOOK_OP_BRANCH << 28) | 0x0FFF_FFFF);
    }

    #[test]
    fn pack_branch_list_dlist_one_bit_above_28_bits_is_masked() {
        let w = pack_branch_list(0, 0xFFFF_FFFF);
        assert_eq!(w[1], (RT64_HOOK_OP_BRANCH << 28) | 0x0FFF_FFFF);
    }

    #[test]
    fn pack_display_list_dlist_max_28_bit_value() {
        let w = pack_display_list(0, 0x0FFF_FFFF);
        assert_eq!(w[1], (RT64_HOOK_OP_DL << 28) | 0x0FFF_FFFF);
    }

    // --- pack_no_op / pack_print (extended opcodes, no payload) ---

    #[test]
    fn pack_no_op_all_zero() {
        let w = pack_no_op(0);
        assert_eq!(w, [param(G_EX_NOOP, 24, 0), 0]);
    }

    #[test]
    fn pack_no_op_extended_opcode_default_value() {
        let w = pack_no_op(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w[0], (0x64u32 << 24) | G_EX_NOOP);
        assert_eq!(w[1], 0);
    }

    #[test]
    fn pack_print_opcode_field() {
        let w = pack_print(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w[0], (0x64u32 << 24) | G_EX_PRINT);
        assert_eq!(w[1], 0);
    }

    // --- pack_texture_rectangle ---

    #[test]
    fn pack_texture_rectangle_all_zero() {
        let w = pack_texture_rectangle(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w, [G_EX_TEXRECT_V1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn pack_texture_rectangle_each_field_at_max_value() {
        let w = pack_texture_rectangle(
            RT64_EXTENDED_OPCODE_DEFAULT,
            0xFFF,  // lorigin, 12 bits
            0xFFF,  // rorigin, 12 bits
            0xFFFF, // ulx, 16 bits
            0xFFFF, // uly, 16 bits
            0xFFFF, // lrx, 16 bits
            0xFFFF, // lry, 16 bits
            0x7,    // tile, 3 bits
            0xFFFF, // s, 16 bits
            0xFFFF, // t, 16 bits
            0xFFFF, // dsdx, 16 bits
            0xFFFF, // dtdy, 16 bits
        );
        assert_eq!(w[0], (0x64u32 << 24) | G_EX_TEXRECT_V1);
        assert_eq!(w[1], 0x7 | (0xFFF << 3) | (0xFFF << 15));
        assert_eq!(w[2], 0xFFFF_FFFF);
        assert_eq!(w[3], 0xFFFF_FFFF);
        assert_eq!(w[4], 0xFFFF_FFFF);
        assert_eq!(w[5], 0xFFFF_FFFF);
    }

    #[test]
    fn pack_texture_rectangle_tile_one_bit_above_three_bits_is_masked() {
        let w = pack_texture_rectangle(0, 0, 0, 0, 0, 0, 0, 0x8, 0, 0, 0, 0);
        assert_eq!(w[1], 0);
    }

    #[test]
    fn pack_texture_rectangle_lorigin_one_bit_above_twelve_bits_is_masked() {
        let w = pack_texture_rectangle(0, 0x1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w[1], 0);
    }

    // --- pack_viewport ---

    #[test]
    fn pack_viewport_all_zero() {
        let w = pack_viewport(0, 0, 0);
        assert_eq!(w, [G_EX_SETVIEWPORT_V1, 0, 0, 0]);
    }

    #[test]
    fn pack_viewport_origin_field_and_raw_vp_address() {
        let w = pack_viewport(
            RT64_EXTENDED_OPCODE_DEFAULT,
            G_EX_ORIGIN_CENTER,
            0xDEAD_BEEF,
        );
        assert_eq!(w[0], (0x64u32 << 24) | G_EX_SETVIEWPORT_V1);
        assert_eq!(w[1], G_EX_ORIGIN_CENTER);
        assert_eq!(w[2], 0);
        assert_eq!(w[3], 0xDEAD_BEEF);
    }

    #[test]
    fn pack_viewport_origin_one_bit_above_twelve_bits_is_masked() {
        let w = pack_viewport(0, 0x1000, 0);
        assert_eq!(w[1], 0);
    }

    #[test]
    fn pack_viewport_vp_is_not_masked_full_32_bit_passthrough() {
        // vp is a bare (unsigned) cast, never PARAM-masked -- full 32 bits
        // must survive untouched, unlike every PARAM-wrapped field above.
        let w = pack_viewport(0, 0, 0xFFFF_FFFF);
        assert_eq!(w[3], 0xFFFF_FFFF);
    }

    // --- pack_set_scissor ---

    #[test]
    fn pack_set_scissor_all_zero() {
        let w = pack_set_scissor(0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w, [G_EX_SETSCISSOR_V1, 0, 0, 0]);
    }

    #[test]
    fn pack_set_scissor_coordinates_scaled_by_four_before_masking() {
        let w = pack_set_scissor(0, 0, 0, 0, 100, 200, 300, 400);
        assert_eq!(w[2], (400u32 << 16) | 800u32);
        assert_eq!(w[3], (1200u32 << 16) | 1600u32);
    }

    #[test]
    fn pack_set_scissor_mode_and_origins_max_values() {
        let w = pack_set_scissor(0, 0x3, 0xFFF, 0xFFF, 0, 0, 0, 0);
        assert_eq!(w[1], 0x3 | (0xFFF << 2) | (0xFFF << 14));
    }

    #[test]
    fn pack_set_scissor_mode_one_bit_above_two_bits_is_masked() {
        let w = pack_set_scissor(0, 0x4, 0, 0, 0, 0, 0, 0);
        assert_eq!(w[1], 0);
    }

    #[test]
    fn pack_set_scissor_negative_coordinate_reinterpreted_as_u32() {
        // A negative ulx (off-screen scissor edge) reinterpreted per the
        // "Admitted domain" note: (-1i16 as u32) * 4 wraps in u32 space
        // exactly as C's unsigned multiplication would, then PARAM masks
        // to 16 bits.
        let neg_one = (-1i16) as u16 as u32;
        let w = pack_set_scissor(0, 0, 0, 0, neg_one, 0, 0, 0);
        let expected_high = (neg_one.wrapping_mul(4)) & 0xFFFF;
        assert_eq!(w[2] >> 16, expected_high);
    }

    // --- pack_set_rect_align ---

    #[test]
    fn pack_set_rect_align_all_zero() {
        let w = pack_set_rect_align(0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w, [G_EX_SETRECTALIGN_V1, 0, 0, 0]);
    }

    #[test]
    fn pack_set_rect_align_origins_max_twelve_bit_values() {
        let w = pack_set_rect_align(0, 0xFFF, 0xFFF, 0, 0, 0, 0);
        assert_eq!(w[1], 0xFFF | (0xFFF << 12));
    }

    #[test]
    fn pack_set_rect_align_lorigin_one_bit_above_twelve_bits_is_masked() {
        let w = pack_set_rect_align(0, 0x1000, 0, 0, 0, 0, 0);
        assert_eq!(w[1], 0);
    }

    #[test]
    fn pack_set_rect_align_offsets_not_scaled_unlike_scissor() {
        // Unlike gEXSetScissor, gEXSetRectAlign does NOT multiply offsets
        // by 4 -- they are packed directly.
        let w = pack_set_rect_align(0, 0, 0, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF);
        assert_eq!(w[2], 0xFFFF_FFFF);
        assert_eq!(w[3], 0xFFFF_FFFF);
    }

    // --- pack_set_viewport_align ---

    #[test]
    fn pack_set_viewport_align_all_zero() {
        let w = pack_set_viewport_align(0, 0, 0, 0);
        assert_eq!(w, [G_EX_SETVIEWPORTALIGN_V1, 0, 0, 0]);
    }

    #[test]
    fn pack_set_viewport_align_offsets_max_sixteen_bit_values() {
        let w = pack_set_viewport_align(0, 0, 0xFFFF, 0xFFFF);
        assert_eq!(w[2], 0xFFFF_FFFF);
        assert_eq!(w[3], 0);
    }

    // --- pack_set_scissor_align ---

    #[test]
    fn pack_set_scissor_align_all_zero() {
        let w = pack_set_scissor_align(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w, [G_EX_SETSCISSORALIGN_V1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn pack_set_scissor_align_offsets_and_bounds_scaled_by_four() {
        let w = pack_set_scissor_align(0, 0, 0, 10, 20, 30, 40, 50, 60, 70, 80);
        assert_eq!(w[2], (40u32 << 16) | 80u32);
        assert_eq!(w[3], (120u32 << 16) | 160u32);
        assert_eq!(w[4], (200u32 << 16) | 240u32);
        assert_eq!(w[5], (280u32 << 16) | 320u32);
    }

    // --- pack_set_refresh_rate ---

    #[test]
    fn pack_set_refresh_rate_max_sixteen_bit_value() {
        let w = pack_set_refresh_rate(0, 0xFFFF);
        assert_eq!(w[1], 0xFFFF);
    }

    #[test]
    fn pack_set_refresh_rate_one_bit_above_sixteen_bits_is_masked() {
        let w = pack_set_refresh_rate(0, 0x1_FFFF);
        assert_eq!(w[1], 0xFFFF);
    }

    // --- pack_vertex_z_test / pack_end_vertex_z_test ---

    #[test]
    fn pack_vertex_z_test_max_eight_bit_value() {
        let w = pack_vertex_z_test(0, 0xFF);
        assert_eq!(w[1], 0xFF);
    }

    #[test]
    fn pack_vertex_z_test_one_bit_above_eight_bits_is_masked() {
        let w = pack_vertex_z_test(0, 0x1FF);
        assert_eq!(w[1], 0xFF);
    }

    #[test]
    fn pack_end_vertex_z_test_carries_no_payload() {
        let w = pack_end_vertex_z_test(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w[0], (0x64u32 << 24) | G_EX_ENDVERTEXZTEST_V1);
        assert_eq!(w[1], 0);
    }

    // --- pack_matrix_group ---

    #[test]
    fn pack_matrix_group_all_zero() {
        let w = pack_matrix_group(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w, [G_EX_MATRIXGROUP_V1, 0, 0, 0]);
    }

    #[test]
    fn pack_matrix_group_id_is_raw_second_word_full_32_bits() {
        // id is not PARAM-masked -- a bare second command word.
        let w = pack_matrix_group(0, 0xFFFF_FFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w[1], 0xFFFF_FFFF);
    }

    #[test]
    fn pack_matrix_group_id_ignore_and_id_auto_constants() {
        let w_ignore = pack_matrix_group(
            0,
            G_EX_ID_IGNORE,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        );
        assert_eq!(w_ignore[1], 0);
        let w_auto =
            pack_matrix_group(0, G_EX_ID_AUTO, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w_auto[1], 0xFFFF_FFFF);
    }

    #[test]
    fn pack_matrix_group_proj_is_normalized_to_a_single_bit_via_not_equal_zero() {
        // PARAM((proj) != 0, 1, 1): any nonzero proj packs to exactly bit 1
        // set, not the raw value shifted -- pins the C `!=` boolean
        // normalization, distinct from a plain PARAM(proj, 1, 1).
        let w_two = pack_matrix_group(0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        let w_one = pack_matrix_group(0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w_two[2], 0b10);
        assert_eq!(w_one[2], 0b10);
    }

    #[test]
    fn pack_matrix_group_every_flag_field_at_max_packs_to_all_ones_flags_word() {
        let w = pack_matrix_group(
            0, 0, 1,    // mode
            1,    // push
            1,    // proj (normalized)
            0b11, // pos
            0b11, // rot
            0b11, // scale
            0b11, // skew
            0b11, // persp
            0b11, // vert
            0b11, // tile
            0b11, // order
            1,    // edit
            0b11, // aspect
            0b11, // tc
            0b11, // lookat
        );
        // Highest field (lookat) is 2 bits at shift 24, occupying bits
        // 24-25 -- the packed flags word only ever fills bits [0..26), not
        // the full 32 bits.
        assert_eq!(w[2], 0x03FF_FFFF);
    }

    #[test]
    fn pack_matrix_group_lookat_field_one_bit_above_two_bits_is_masked() {
        // lookat is the highest field, at shift 24, 2 bits wide -- a value
        // of 0b100 (bit 26) must not overflow into bit 26/27 (out of the
        // defined flags word).
        let w = pack_matrix_group(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b100);
        assert_eq!(w[2], 0);
    }

    // --- pack_pop_matrix_group / pack_pop_matrix_group_n ---

    #[test]
    fn pack_pop_matrix_group_defaults_count_to_one() {
        let w = pack_pop_matrix_group(RT64_EXTENDED_OPCODE_DEFAULT, 1);
        assert_eq!(w[1], 1 | (1 << 8));
    }

    #[test]
    fn pack_pop_matrix_group_n_count_max_eight_bit_value() {
        let w = pack_pop_matrix_group_n(0, 0, 0xFF);
        assert_eq!(w[1], 0xFF);
    }

    #[test]
    fn pack_pop_matrix_group_n_count_one_bit_above_eight_bits_is_masked() {
        let w = pack_pop_matrix_group_n(0, 0, 0x1FF);
        assert_eq!(w[1], 0xFF);
    }

    #[test]
    fn pack_pop_matrix_group_n_proj_bit_at_position_eight() {
        let w = pack_pop_matrix_group_n(0, 1, 0);
        assert_eq!(w[1], 1 << 8);
    }

    #[test]
    fn pack_pop_matrix_group_delegates_exactly_to_pop_matrix_group_n_with_count_one() {
        let direct = pack_pop_matrix_group(0x64, 1);
        let via_n = pack_pop_matrix_group_n(0x64, 1, 1);
        assert_eq!(direct, via_n);
    }

    // --- single-bit/two-bit toggle packers ---

    #[test]
    fn pack_force_upscale_2d_bit_zero_toggle() {
        assert_eq!(pack_force_upscale_2d(0, 1)[1], 1);
        assert_eq!(pack_force_upscale_2d(0, 0)[1], 0);
    }

    #[test]
    fn pack_force_upscale_2d_one_bit_above_single_bit_field_is_masked() {
        assert_eq!(pack_force_upscale_2d(0, 0b10)[1], 0);
    }

    #[test]
    fn pack_force_true_bilerp_two_bit_field_all_bilerp_enum_values() {
        assert_eq!(pack_force_true_bilerp(0, G_EX_BILERP_NONE)[1], 0);
        assert_eq!(pack_force_true_bilerp(0, G_EX_BILERP_ONLY)[1], 1);
        assert_eq!(pack_force_true_bilerp(0, G_EX_BILERP_ALL)[1], 2);
    }

    #[test]
    fn pack_force_true_bilerp_one_bit_above_two_bit_field_is_masked() {
        assert_eq!(pack_force_true_bilerp(0, 0b100)[1], 0);
    }

    #[test]
    fn pack_force_scale_lod_bit_zero_toggle() {
        assert_eq!(pack_force_scale_lod(0, 1)[1], 1);
    }

    #[test]
    fn pack_force_branch_bit_zero_toggle() {
        assert_eq!(pack_force_branch(0, 1)[1], 1);
    }

    #[test]
    fn pack_set_render_to_ram_bit_zero_toggle() {
        assert_eq!(pack_set_render_to_ram(0, 1)[1], 1);
    }

    // --- pack_edit_group_by_address ---

    #[test]
    fn pack_edit_group_by_address_all_zero() {
        let w = pack_edit_group_by_address(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w, [G_EX_EDITGROUPBYADDRESS_V1, 0, 1 << 18, 0]);
    }

    #[test]
    fn pack_edit_group_by_address_address_is_raw_not_masked() {
        let w = pack_edit_group_by_address(0, 0xFFFF_FFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(w[1], 0xFFFF_FFFF);
    }

    #[test]
    fn pack_edit_group_by_address_edit_flag_is_hardcoded_allow_not_a_parameter() {
        // Unlike pack_matrix_group's caller-supplied `edit`, this macro
        // always ORs in G_EX_EDIT_ALLOW at bit 18 regardless of any input.
        let w = pack_edit_group_by_address(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!((w[2] >> 18) & 1, G_EX_EDIT_ALLOW);
    }

    #[test]
    fn pack_edit_group_by_address_proj_normalized_to_single_bit() {
        let w = pack_edit_group_by_address(0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!((w[2] >> 1) & 1, 1);
    }

    #[test]
    fn pack_edit_group_by_address_every_flag_field_at_max() {
        let w = pack_edit_group_by_address(
            0, 0, 1, // mode
            1, // push
            1, // proj
            0b11, 0b11, 0b11, 0b11, 0b11, 0b11, 0b11, 0b11,
        );
        // bits 0..=18 all set (push..edit=ALLOW), matching every field at
        // its widest value plus the hardcoded edit-allow bit.
        assert_eq!(w[2], 0x0007_FFFF);
    }

    // --- pack_vertex ---

    #[test]
    fn pack_vertex_all_zero() {
        let w = pack_vertex(0, 0, 0, 0);
        assert_eq!(w, [G_EX_VERTEX_V1, 0, 0, 0]);
    }

    #[test]
    fn pack_vertex_count_and_v0_max_eight_bit_values() {
        let w = pack_vertex(0, 0, 0xFF, 0xFF);
        assert_eq!(w[1], 0xFF | (0xFF << 8));
    }

    #[test]
    fn pack_vertex_count_one_bit_above_eight_bits_is_masked() {
        let w = pack_vertex(0, 0, 0x1FF, 0);
        assert_eq!(w[1], 0xFF << 8);
    }

    #[test]
    fn pack_vertex_vtx_is_raw_full_32_bit_address() {
        let w = pack_vertex(0, 0xDEAD_BEEF, 0, 0);
        assert_eq!(w[3], 0xDEAD_BEEF);
    }

    // --- pack_set_proj_matrix_float / pack_set_view_matrix_float ---

    #[test]
    fn pack_set_proj_matrix_float_matrix_is_raw_full_32_bit_address() {
        let w = pack_set_proj_matrix_float(RT64_EXTENDED_OPCODE_DEFAULT, 0xFFFF_FFFF);
        assert_eq!(w[0], (0x64u32 << 24) | G_EX_SETPROJMATRIXFLOAT_V1);
        assert_eq!(w[1], 0xFFFF_FFFF);
    }

    #[test]
    fn pack_set_view_matrix_float_matrix_is_raw_full_32_bit_address() {
        let w = pack_set_view_matrix_float(0, 0xDEAD_BEEF);
        assert_eq!(w[1], 0xDEAD_BEEF);
    }

    // --- trivial no-payload push/pop packers ---

    #[test]
    fn pack_push_viewport_opcode_only() {
        let w = pack_push_viewport(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHVIEWPORT_V1, 0]);
    }

    #[test]
    fn pack_pop_viewport_opcode_only() {
        let w = pack_pop_viewport(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPVIEWPORT_V1, 0]);
    }

    #[test]
    fn pack_push_scissor_opcode_only() {
        let w = pack_push_scissor(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHSCISSOR_V1, 0]);
    }

    #[test]
    fn pack_pop_scissor_opcode_only() {
        let w = pack_pop_scissor(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPSCISSOR_V1, 0]);
    }

    #[test]
    fn pack_push_other_mode_opcode_only() {
        let w = pack_push_other_mode(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHOTHERMODE_V1, 0]);
    }

    #[test]
    fn pack_pop_other_mode_opcode_only() {
        let w = pack_pop_other_mode(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPOTHERMODE_V1, 0]);
    }

    #[test]
    fn pack_push_combine_mode_opcode_only() {
        let w = pack_push_combine_mode(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHCOMBINE_V1, 0]);
    }

    #[test]
    fn pack_pop_combine_mode_opcode_only() {
        let w = pack_pop_combine_mode(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPCOMBINE_V1, 0]);
    }

    #[test]
    fn pack_push_projection_matrix_opcode_only() {
        let w = pack_push_projection_matrix(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHPROJMATRIX_V1, 0]);
    }

    #[test]
    fn pack_pop_projection_matrix_opcode_only() {
        let w = pack_pop_projection_matrix(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPPROJMATRIX_V1, 0]);
    }

    #[test]
    fn pack_push_env_color_opcode_only() {
        let w = pack_push_env_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHENVCOLOR_V1, 0]);
    }

    #[test]
    fn pack_pop_env_color_opcode_only() {
        let w = pack_pop_env_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPENVCOLOR_V1, 0]);
    }

    #[test]
    fn pack_push_blend_color_opcode_only() {
        let w = pack_push_blend_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHBLENDCOLOR_V1, 0]);
    }

    #[test]
    fn pack_pop_blend_color_opcode_only() {
        let w = pack_pop_blend_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPBLENDCOLOR_V1, 0]);
    }

    #[test]
    fn pack_push_fog_color_opcode_only() {
        let w = pack_push_fog_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHFOGCOLOR_V1, 0]);
    }

    #[test]
    fn pack_pop_fog_color_opcode_only() {
        let w = pack_pop_fog_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPFOGCOLOR_V1, 0]);
    }

    #[test]
    fn pack_push_fill_color_opcode_only() {
        let w = pack_push_fill_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHFILLCOLOR_V1, 0]);
    }

    #[test]
    fn pack_pop_fill_color_opcode_only() {
        let w = pack_pop_fill_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPFILLCOLOR_V1, 0]);
    }

    #[test]
    fn pack_push_prim_color_opcode_only() {
        let w = pack_push_prim_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHPRIMCOLOR_V1, 0]);
    }

    #[test]
    fn pack_pop_prim_color_opcode_only() {
        let w = pack_pop_prim_color(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPPRIMCOLOR_V1, 0]);
    }

    #[test]
    fn pack_push_geometry_mode_opcode_only() {
        let w = pack_push_geometry_mode(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_PUSHGEOMETRYMODE_V1, 0]);
    }

    #[test]
    fn pack_pop_geometry_mode_opcode_only() {
        let w = pack_pop_geometry_mode(RT64_EXTENDED_OPCODE_DEFAULT);
        assert_eq!(w, [(0x64u32 << 24) | G_EX_POPGEOMETRYMODE_V1, 0]);
    }

    // --- pack_set_dither_noise_strength ---

    #[test]
    fn pack_set_dither_noise_strength_scales_by_1024_like_the_macro_argument() {
        // The caller must pre-multiply by 1024 (see "Admitted domain");
        // this test reproduces that call-site step explicitly.
        let value: f64 = 2.5;
        let scaled = (value * 1024.0) as u32;
        let w = pack_set_dither_noise_strength(0, scaled);
        assert_eq!(w[1], scaled);
    }

    #[test]
    fn pack_set_dither_noise_strength_max_sixteen_bit_value() {
        let w = pack_set_dither_noise_strength(0, 0xFFFF);
        assert_eq!(w[1], 0xFFFF);
    }

    #[test]
    fn pack_set_dither_noise_strength_one_bit_above_sixteen_bits_is_masked() {
        let w = pack_set_dither_noise_strength(0, 0x1_FFFF);
        assert_eq!(w[1], 0xFFFF);
    }

    // --- pack_set_rdram_extended / pack_set_near_clipping ---

    #[test]
    fn pack_set_rdram_extended_bit_zero_toggle() {
        assert_eq!(pack_set_rdram_extended(0, 1)[1], 1);
        assert_eq!(pack_set_rdram_extended(0, 0)[1], 0);
    }

    #[test]
    fn pack_set_near_clipping_bit_zero_toggle() {
        assert_eq!(pack_set_near_clipping(0, 1)[1], 1);
    }

    // --- pack_matrix_float ---

    #[test]
    fn pack_matrix_float_p_max_eight_bit_value() {
        let w = pack_matrix_float(0, 0, 0xFF);
        assert_eq!(w[1], 0xFF);
    }

    #[test]
    fn pack_matrix_float_p_one_bit_above_eight_bits_is_masked() {
        let w = pack_matrix_float(0, 0, 0x1FF);
        assert_eq!(w[1], 0xFF);
    }

    #[test]
    fn pack_matrix_float_m_is_raw_full_32_bit_address() {
        let w = pack_matrix_float(0, 0xDEAD_BEEF, 0);
        assert_eq!(w[3], 0xDEAD_BEEF);
    }

    // --- pack_set_vertex_segment ---

    #[test]
    fn pack_set_vertex_segment_all_zero() {
        let w = pack_set_vertex_segment(0, 0, 0, 0, 0);
        assert_eq!(w, [G_EX_SETVERTEXSEGMENT_V1, 0, 0, 0]);
    }

    #[test]
    fn pack_set_vertex_segment_element_and_enabled_fields() {
        let w = pack_set_vertex_segment(0, 0xF, 1, 0, 0);
        assert_eq!(w[1], 1 | (0xF << 1));
    }

    #[test]
    fn pack_set_vertex_segment_element_one_bit_above_four_bits_is_masked() {
        let w = pack_set_vertex_segment(0, 0x1F, 0, 0, 0);
        assert_eq!(w[1], 0x1E); // 0x1F & 0xF = 0xF, shifted by 1 = 0x1E
    }

    #[test]
    fn pack_set_vertex_segment_addresses_are_raw_full_32_bit_passthrough() {
        let w = pack_set_vertex_segment(0, 0, 0, 0xAAAA_AAAA, 0x5555_5555);
        assert_eq!(w[2], 0xAAAA_AAAA);
        assert_eq!(w[3], 0x5555_5555);
    }

    // --- pack_set_texcoord_wrap_point ---

    #[test]
    fn pack_set_texcoord_wrap_point_both_fields_max_sixteen_bit_values() {
        let w = pack_set_texcoord_wrap_point(0, 0xFFFF, 0xFFFF);
        assert_eq!(w[1], 0xFFFF_FFFF);
    }

    #[test]
    fn pack_set_texcoord_wrap_point_u_one_bit_above_sixteen_bits_is_masked() {
        let w = pack_set_texcoord_wrap_point(0, 0x1_FFFF, 0);
        assert_eq!(w[1], 0xFFFF_0000);
    }

    // --- pack_set_rect_aspect ---

    #[test]
    fn pack_set_rect_aspect_two_bit_field_all_aspect_enum_values() {
        assert_eq!(pack_set_rect_aspect(0, G_EX_ASPECT_AUTO)[1], 0);
        assert_eq!(pack_set_rect_aspect(0, G_EX_ASPECT_STRETCH)[1], 1);
        assert_eq!(pack_set_rect_aspect(0, G_EX_ASPECT_ADJUST)[1], 2);
    }

    #[test]
    fn pack_set_rect_aspect_one_bit_above_two_bit_field_is_masked() {
        assert_eq!(pack_set_rect_aspect(0, 0b100)[1], 0);
    }

    // --- opcode constant sanity: every G_EX_* opcode is distinct and
    // fits in the 24-bit opcode field the way every gEX* macro packs it ---

    #[test]
    fn every_g_ex_opcode_fits_in_the_twenty_four_bit_opcode_field() {
        let opcodes = [
            G_EX_NOOP,
            G_EX_PRINT,
            G_EX_TEXRECT_V1,
            G_EX_FILLRECT_V1,
            G_EX_SETVIEWPORT_V1,
            G_EX_SETSCISSOR_V1,
            G_EX_SETRECTALIGN_V1,
            G_EX_SETVIEWPORTALIGN_V1,
            G_EX_SETSCISSORALIGN_V1,
            G_EX_SETREFRESHRATE_V1,
            G_EX_VERTEXZTEST_V1,
            G_EX_ENDVERTEXZTEST_V1,
            G_EX_MATRIXGROUP_V1,
            G_EX_POPMATRIXGROUP_V1,
            G_EX_FORCEUPSCALE2D_V1,
            G_EX_FORCETRUEBILERP_V1,
            G_EX_FORCESCALELOD_V1,
            G_EX_FORCEBRANCH_V1,
            G_EX_SETRENDERTORAM_V1,
            G_EX_EDITGROUPBYADDRESS_V1,
            G_EX_VERTEX_V1,
            G_EX_PUSHVIEWPORT_V1,
            G_EX_POPVIEWPORT_V1,
            G_EX_PUSHSCISSOR_V1,
            G_EX_POPSCISSOR_V1,
            G_EX_PUSHOTHERMODE_V1,
            G_EX_POPOTHERMODE_V1,
            G_EX_PUSHCOMBINE_V1,
            G_EX_POPCOMBINE_V1,
            G_EX_PUSHPROJMATRIX_V1,
            G_EX_POPPROJMATRIX_V1,
            G_EX_PUSHENVCOLOR_V1,
            G_EX_POPENVCOLOR_V1,
            G_EX_PUSHBLENDCOLOR_V1,
            G_EX_POPBLENDCOLOR_V1,
            G_EX_PUSHFOGCOLOR_V1,
            G_EX_POPFOGCOLOR_V1,
            G_EX_PUSHFILLCOLOR_V1,
            G_EX_POPFILLCOLOR_V1,
            G_EX_PUSHPRIMCOLOR_V1,
            G_EX_POPPRIMCOLOR_V1,
            G_EX_PUSHGEOMETRYMODE_V1,
            G_EX_POPGEOMETRYMODE_V1,
            G_EX_SETDITHERNOISESTRENGTH_V1,
            G_EX_SETRDRAMEXTENDED_V1,
            G_EX_SETPROJMATRIXFLOAT_V1,
            G_EX_SETVIEWMATRIXFLOAT_V1,
            G_EX_SETNEARCLIPPING_V1,
            G_EX_MATRIX_FLOAT_V1,
            G_EX_SETVERTEXSEGMENT_V1,
            G_EX_SETTEXCOORDWRAPPOINT_V1,
            G_EX_SETRECTASPECT_V1,
        ];
        for op in opcodes {
            assert!(op < (1u32 << 24));
        }
        assert_eq!(opcodes.len() as u32, G_EX_MAX);
        // Consecutive, gap-free 0..G_EX_MAX, matching the header's literal
        // sequential enumeration.
        let mut sorted = opcodes;
        sorted.sort_unstable();
        for (i, op) in sorted.iter().enumerate() {
            assert_eq!(*op, i as u32);
        }
    }
}
