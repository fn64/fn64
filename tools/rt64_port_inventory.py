#!/usr/bin/env python3
"""Build and validate fn64's dual-pin RT64-to-Rust source/task denominator.

Only path names, digests, include edges, and non-exhaustive navigation hints
leave the admitted MIT RT64 checkouts.  Implementation text is never emitted.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parent.parent
AUTHORITY_PATH = ROOT / "docs/rt64-port-authority.json"
DEFAULT_JSON = ROOT / "docs/rt64-port-inventory.json"
DEFAULT_DOC = ROOT / "docs/RT64-PORT-INVENTORY.md"
SCHEMA = "fn64.rt64-port-inventory.v2"
EXPECTED_SOURCE_SET_SHA256 = "86704d407a71722233e71938b8517d647b38b6d2ff71d1702cc7c5e5c0232c8b"
SOURCE_SELECTIONS = ("oracle", "port")
SOURCE_PREFIXES = (
    "include",
    "src/apple",
    "src/common",
    "src/gbi",
    "src/gui",
    "src/hle",
    "src/imgui",
    "src/preset",
    "src/render",
    "src/rhi",
    "src/shaders",
    "src/shared",
)
SUFFIXES = {".cpp", ".h", ".hpp", ".mm", ".hlsl", ".hlsli"}
EXCLUDED_PREFIXES = ("src/contrib/", "src/tools/")
EXCLUSION_RECORDS = [
    {
        "path": "src/tools/texture_hasher",
        "reason": "Separate GPL-derived Rice-generation tool; its GLIDEN64-LICENSE lineage is not Rust-port authority.",
    },
    {
        "path": "src/contrib/mupen64plus-core",
        "reason": "GPL runtime implementation excluded from the fn64 target and from port authority.",
    },
    {
        "path": "m2c",
        "reason": "Excluded conversion tool under AGENTS.md; it is neither invoked nor inspected by this inventory.",
    },
]
INCLUDE = re.compile(r'^\s*#\s*include\s+"([^"\\]+)"', re.MULTILINE)
DECLARATION_HINT = re.compile(
    r"\b(class|struct|enum(?:\s+class)?)\s+(?:alignas\s*\([^)]*\)\s*)?([A-Za-z_]\w*)"
)
FUNCTION_DEFINITION_HINT = re.compile(
    r"(?:^|[;}])\s*(?:[\w:<>,~*&\[\]\s]+?)\b"
    r"([A-Za-z_]\w*(?:::[A-Za-z_]\w*)?)\s*\([^;{}]*\)"
    r"\s*(?:const\s*)?(?:noexcept\s*)?(?:\{|:\s*[^;{]+\{)",
    re.MULTILINE,
)
NON_FUNCTION_HINTS = {
    "alignas", "catch", "constexpr", "defined", "for", "if", "numthreads",
    "return", "sizeof", "switch", "while",
}
MILESTONES = {"M1", "M3", "M4", "M5", "M6", "M8", "M10", "M11", "M12"}
PORT_STATES = {"ported", "not-started", "refused", "authority-gated"}
# Declared, citation-carrying refusals: sources a landed batch assessment
# examined and settled as never-to-be-ported.
#
# `refused` is the one `port_state` that cannot be inferred from digests. A
# digest proves a port happened; nothing in a byte stream can prove a human
# read a file and concluded there is nothing in it worth owning. So this state
# is *declared* -- and declared inputs are what this tool is built to distrust.
# The distrust is discharged, not waived, by making every entry carry a
# citation that the tool mechanically resolves against this repository:
#
#   `commit`   the full 40-hex SHA-1 of the landed assessment. `verify_refusals`
#              resolves it with `git cat-file -t` in ROOT and requires a
#              `commit` object whose message names this batch. A fabricated or
#              rewritten-away SHA fails closed.
#   `evidence` a repository-relative Rust module or document that records the
#              reasoning. `verify_refusals` requires the file to exist. A
#              refusal pointing at a deleted or invented file fails closed.
#   `reason`   the one-line human judgement, for a reader who has the citation
#              open in the other window.
#
# The `require()` in `validate_refusals` refuses a refusal with no citation
# exactly as `port_state_for` refuses a `ported` claim with no digest: both
# states must name the artifact a reader can check. What this deliberately
# does NOT do is widen to a partial or behavioral claim -- `refused` says only
# "an assessment settled this file", never "this file is done".
#
# Scope is closed at the six landed batch assessments. Every entry below is
# one of the 17 `src/common`, 28 `src/render`, 18 `src/hle`, or 10
# `src/gui`/`src/imgui` sources those assessments refused (6 from the
# geometry batch, 10 from the VI-registers batch, 2 from the
# workload-geometry batch, all 10 the GUI batch refused); nothing else may be
# added without a new landed assessment.
COMMON_ASSESSMENT_COMMIT = "f4850c0032fbb7b266bcb80d2c0cfa0178f31d85"
COMMON_ASSESSMENT_SUBJECT = "cite the RT64 configuration digests settings.rs already implements"
COMMON_ASSESSMENT_EVIDENCE = "crates/fn64-render/src/settings.rs"
RENDER_ASSESSMENT_COMMIT = "2e915940693019ae4fee9fcae93976d18d401371"
RENDER_ASSESSMENT_SUBJECT = "port RT64's tile-bounds lerp, refuse 28 of 29 render files"
RENDER_ASSESSMENT_EVIDENCE = "crates/fn64-render-wgpu/src/rt64_render_pipeline_types.rs"
HLE_ASSESSMENT_COMMIT = "d2980310ab227978c193426fdfee816a79ca2603"
HLE_ASSESSMENT_SUBJECT = "port six HLE geometry sources, refuse six, find a dither trap"
HLE_ASSESSMENT_EVIDENCE = "crates/fn64-render-wgpu/src/rt64_hle_geometry.rs"
VI_REGISTERS_ASSESSMENT_COMMIT = "49f6760b64ffc5926913132727d3c5b5834e98bf"
VI_REGISTERS_ASSESSMENT_SUBJECT = "render-wgpu: compare RT64's VI registers against fn64, refuse ten of twelve"
VI_REGISTERS_ASSESSMENT_EVIDENCE = "crates/fn64-render-wgpu/src/rt64_vi_registers.rs"
WORKLOAD_GEOMETRY_ASSESSMENT_COMMIT = "2b4253cb17b6a923345c9b194332a39d4a5f7780"
WORKLOAD_GEOMETRY_ASSESSMENT_SUBJECT = "render-wgpu: port the workload cluster's config arithmetic, refuse the rest"
WORKLOAD_GEOMETRY_ASSESSMENT_EVIDENCE = "crates/fn64-render-wgpu/src/rt64_workload_geometry.rs"
GUI_ASSESSMENT_COMMIT = "be52ea716f319113985155c6ce097fa6ba813e30"
GUI_ASSESSMENT_SUBJECT = "docs: land the src/gui assessment as citable evidence"
GUI_ASSESSMENT_EVIDENCE = "docs/RT64-GUI-ASSESSMENT.md"
SCOPING_EVIDENCE = "docs/RT64-M6-M7-SCOPING.md"


def _common_refusal(reason: str, evidence: str = COMMON_ASSESSMENT_EVIDENCE) -> dict:
    return {"commit": COMMON_ASSESSMENT_COMMIT, "evidence": evidence, "reason": reason}


def _render_refusal(reason: str, evidence: str = RENDER_ASSESSMENT_EVIDENCE) -> dict:
    return {"commit": RENDER_ASSESSMENT_COMMIT, "evidence": evidence, "reason": reason}


def _hle_refusal(reason: str, evidence: str = HLE_ASSESSMENT_EVIDENCE) -> dict:
    return {"commit": HLE_ASSESSMENT_COMMIT, "evidence": evidence, "reason": reason}


def _vi_registers_refusal(reason: str, evidence: str = VI_REGISTERS_ASSESSMENT_EVIDENCE) -> dict:
    return {"commit": VI_REGISTERS_ASSESSMENT_COMMIT, "evidence": evidence, "reason": reason}


def _workload_geometry_refusal(reason: str, evidence: str = WORKLOAD_GEOMETRY_ASSESSMENT_EVIDENCE) -> dict:
    return {"commit": WORKLOAD_GEOMETRY_ASSESSMENT_COMMIT, "evidence": evidence, "reason": reason}


def _gui_refusal(reason: str, evidence: str = GUI_ASSESSMENT_EVIDENCE) -> dict:
    return {"commit": GUI_ASSESSMENT_COMMIT, "evidence": evidence, "reason": reason}


PORT_REFUSALS: dict[str, dict[str, str]] = {
    # -- src/common, 17 of the 21 files the batch assessment refused. The other
    # four (rt64_emulator_configuration.{cpp,h}, rt64_enhancement_configuration
    # .{cpp,h}) were refused as *already fully owned* and are therefore
    # `ported` via crates/fn64-render/src/settings.rs, not `refused`.
    "src/common/rt64_dynamic_libraries.cpp": _common_refusal(
        "Host dynamic-loader shim (dlopen/LoadLibrary); no CPU-side behavior to own."
    ),
    "src/common/rt64_dynamic_libraries.h": _common_refusal(
        "Declarations for the host dynamic-loader shim; no arithmetic."
    ),
    "src/common/rt64_filesystem.h": _common_refusal(
        "Abstract filesystem interface; pure virtual declarations, no behavior."
    ),
    "src/common/rt64_filesystem_directory.h": _common_refusal(
        "std::filesystem directory-walk adapter; host I/O plumbing."
    ),
    "src/common/rt64_filesystem_zip.cpp": _common_refusal(
        "miniz and zstd FFI archive reader over a mapped file; a vendored-library binding, "
        "not portable behavior."
    ),
    "src/common/rt64_filesystem_zip.h": _common_refusal(
        "Declarations over the miniz/zstd FFI archive reader."
    ),
    "src/common/rt64_hlslpp.h": _common_refusal(
        "Third-party hlslpp include/pragma shim; 13 lines, no fn64-owned content."
    ),
    "src/common/rt64_load_types.cpp": _common_refusal(
        "nlohmann to_json/from_json for the LoadTile and LoadTexture PODs; 38 of 60 lines "
        "are one-to-one field copies and there is no arithmetic to own."
    ),
    "src/common/rt64_load_types.h": _common_refusal(
        "POD field declarations for the above, over a vendored nlohmann json typedef."
    ),
    "src/common/rt64_mapped_file.cpp": _common_refusal(
        "mmap / CreateFileMappingW host memory mapping; platform syscalls, not behavior."
    ),
    "src/common/rt64_mapped_file.h": _common_refusal(
        "Declarations for the host memory-mapping wrapper."
    ),
    "src/common/rt64_plume.h": _common_refusal(
        "Five-line include of RT64's plume RHI, which the port plan refuses to transliterate.",
        SCOPING_EVIDENCE,
    ),
    "src/common/rt64_sommelier.h": _common_refusal(
        "Wine/Sommelier host detection; compiles to nothing on macOS."
    ),
    "src/common/rt64_thread.cpp": _common_refusal(
        "pthread_setname_np / SetThreadDescription thread naming; host threading only."
    ),
    "src/common/rt64_thread.h": _common_refusal(
        "Declarations for the host thread-naming helper."
    ),
    "src/common/rt64_user_paths.cpp": _common_refusal(
        "Per-platform user/config directory discovery; host path policy, not RT64 behavior."
    ),
    "src/common/rt64_user_paths.h": _common_refusal(
        "Declarations for per-platform user-path discovery."
    ),
    # -- src/render, the 28 files the batch assessment refused (of the 29 it
    # examined; the 29th, rt64_tile_processor.cpp, is `ported`).
    "src/render/rt64_descriptor_sets.h": _render_refusal(
        "677 lines of 21 RenderDescriptorSetBase subclasses; every non-declaration "
        "line is a builder.add* call and there is zero arithmetic, enumerated not sampled."
    ),
    "src/render/rt64_framebuffer_renderer.h": _render_refusal(
        "Framebuffer-renderer declarations over plume RHI handles; its .cpp arithmetic "
        "is separately owned by rt64_render_target_geometry.rs."
    ),
    "src/render/rt64_framebuffer_renderer_call.h": _render_refusal(
        "Renderer-call bitfields; refused rather than ported because this program makes "
        "no repr(C)/size/alignment/ABI claim."
    ),
    "src/render/rt64_geometry_mode.cpp": _render_refusal(
        "The briefed bit-decoding hypothesis fails: the body is `this->v = v;`, with no "
        "bit decoding and no G_ZBUFFER/G_CULL constant anywhere in the file."
    ),
    "src/render/rt64_geometry_mode.h": _render_refusal(
        "Companion declaration to the above; no constant and no arithmetic."
    ),
    "src/render/rt64_look_at_processor.h": _render_refusal(
        "Bare declarations; the processor's interpolation body is already ported by "
        "crates/fn64-render-wgpu/src/rt64_interpolation_helpers.rs."
    ),
    "src/render/rt64_native_target.h": _render_refusal(
        "RHI-resource declarations; the geometry arithmetic in its .cpp is already ported "
        "by crates/fn64-render-wgpu/src/rt64_framebuffer_geometry.rs."
    ),
    "src/render/rt64_optimus.cpp": _render_refusal(
        "Fourteen lines whose entire body is `#ifdef _WIN32`-guarded, exporting the single "
        "`NvOptimusEnablement` DWORD; compiles to nothing on macOS."
    ),
    "src/render/rt64_projection_processor.h": _render_refusal(
        "Bare declarations; the processor's body is already ported by "
        "crates/fn64-render-wgpu/src/rt64_interpolation_helpers.rs."
    ),
    "src/render/rt64_raster_shader.h": _render_refusal(
        "Declarations over plume RHI pipeline objects; its .cpp is authority-gated.",
        SCOPING_EVIDENCE,
    ),
    "src/render/rt64_raster_shader_cache.h": _render_refusal(
        "Thread-pool and RHI cache declarations; the port plan refuses plume's thread topology.",
        SCOPING_EVIDENCE,
    ),
    "src/render/rt64_render_target.h": _render_refusal(
        "RHI-handle declarations; its .cpp geometry is already ported by "
        "crates/fn64-render-wgpu/src/rt64_render_target_geometry.rs."
    ),
    "src/render/rt64_render_target_manager.cpp": _render_refusal(
        "Its only non-plumbing content is hash() == XXH3_64bits(this, sizeof(RenderTargetKey)), "
        "which hashes struct padding, so which keys collide depends on ABI layout. Refused "
        "rather than decided; a future card wanting target-key identity needs an explicit ABI decision."
    ),
    "src/render/rt64_render_target_manager.h": _render_refusal(
        "Declarations for the above, including the RenderTargetKey whose byte layout the "
        "refused hash depends on."
    ),
    "src/render/rt64_render_worker.cpp": _render_refusal(
        "Plume command-queue/list/fence plumbing with no arithmetic; the port plan refuses to "
        "transliterate the plume RHI, and wgpu is already fn64's RHI.",
        SCOPING_EVIDENCE,
    ),
    "src/render/rt64_render_worker.h": _render_refusal(
        "Declarations for the plume command-queue worker.",
        SCOPING_EVIDENCE,
    ),
    "src/render/rt64_rsp_processor.cpp": _render_refusal(
        "Reduces to dispatch bookkeeping that crates/fn64-render-wgpu/src/rt64_rsp_process.rs "
        "already refused in writing, calling GROUP_SIZE 64 \"a dispatch tile width\"."
    ),
    "src/render/rt64_rsp_processor.h": _render_refusal(
        "Declarations for the above dispatch bookkeeping."
    ),
    "src/render/rt64_sampler_library.h": _render_refusal(
        "Plume sampler-object declarations; no arithmetic."
    ),
    "src/render/rt64_shader_library.h": _render_refusal(
        "Plume shader-object declarations; its .cpp is authority-gated."
    ),
    "src/render/rt64_texture.h": _render_refusal(
        "RHI texture-handle declarations; the texture-cache behavior is separately owned by "
        "crates/fn64-render-wgpu/src/rt64_texture_map_lru.rs."
    ),
    "src/render/rt64_tile_processor.h": _render_refusal(
        "Refused in full and deliberately not digest-cited: bare member and method declarations "
        "plus a ProcessParams pointer bundle, whose only non-pointer contents are two call-site "
        "float defaults."
    ),
    "src/render/rt64_transform_processor.cpp": _render_refusal(
        "Its math is hlslpp::inverse/transpose plus a RigidBody::lerp that is separately owned."
    ),
    "src/render/rt64_transform_processor.h": _render_refusal(
        "Declarations for the above."
    ),
    "src/render/rt64_upscaler.h": _render_refusal(
        "Refused in full as an explicit non-goal of the post-process card that ported "
        "rt64_upscaler.cpp; see crates/fn64-render-wgpu/src/rt64_postprocess.rs.",
        "crates/fn64-render-wgpu/src/rt64_postprocess.rs",
    ),
    "src/render/rt64_vertex_processor.cpp": _render_refusal(
        "Reduces to dispatch bookkeeping that crates/fn64-render-wgpu/src/rt64_rsp_process.rs "
        "already refused in writing -- vertexStart/vertexCount \"exist only to index and bound "
        "the dispatch\"."
    ),
    "src/render/rt64_vertex_processor.h": _render_refusal(
        "Declarations for the above dispatch bookkeeping."
    ),
    "src/render/rt64_vi_renderer.h": _render_refusal(
        "Plume RHI declarations; its .cpp is authority-gated."
    ),
    # -- src/hle, 6 of the 12 files the batch assessment examined (the other
    # six -- rt64_draw_call.{cpp,h}, rt64_framebuffer_pair.cpp,
    # rt64_framebuffer_changes.cpp, rt64_projection.cpp, and a slice of
    # rt64_rdp_tmem.cpp -- landed as `ported` in rt64_hle_geometry.rs).
    "src/hle/rt64_framebuffer_pair.h": _hle_refusal(
        "rt64_frame_compatibility.rs already owns the six scalar fields "
        "(colorImage.{address,fmt,siz,width}, depthImage.{address,formatChanged}) its "
        "predicates read; the rest is a FlushReason tag, a layout-only bitfield, and "
        "RHI-adjacent containers with no arithmetic.",
        "crates/fn64-render-wgpu/src/rt64_frame_compatibility.rs",
    ),
    "src/hle/rt64_projection.h": _hle_refusal(
        "Projection's members are std::vector<GameCall>, LightManager, "
        "std::vector<interop::PointLight> and FixedRect -- the HLE object graph, not "
        "behavior; only its Type enum is needed to express the already-ported usesViewport."
    ),
    "src/hle/rt64_framebuffer_changes.h": _hle_refusal(
        "Two struct declarations whose members are std::unique_ptr<RenderTexture>, "
        "std::unique_ptr<...DescriptorSet> and std::map -- RHI-bound resource handles "
        "with no arithmetic."
    ),
    "src/hle/rt64_game_call.h": _hle_refusal(
        "A five-member aggregate (callDesc: DrawCall, shaderDesc: ShaderDescription, and "
        "three anonymous sub-structs -- meshDesc, debuggerDesc, lerpDesc) plus a "
        "#if SCRIPT_ENABLED callback pointer; ShaderDescription is already owned by "
        "crate::rt64_shader_description and the rest is field declarations, no behavior."
    ),
    "src/hle/rt64_transform_group.h": _hle_refusal(
        "Thirteen default-initialized fields, every default an already-owned G_EX_* "
        "constant in rt64_extended_gbi.rs (G_EX_ID_AUTO, G_EX_COMPONENT_AUTO, "
        "G_EX_COMPONENT_SKIP, G_EX_ORDER_AUTO, G_EX_ASPECT_AUTO, G_EX_EDIT_NONE); no "
        "arithmetic, no predicate, no derived constant.",
        "crates/fn64-render-wgpu/src/rt64_extended_gbi.rs",
    ),
    "src/hle/rt64_microcode.h": _hle_refusal(
        "Its entire content is struct Microcode { uint32_t half1; uint32_t half2; }; "
        "field declaration order is not pinnable in safe Rust, and this card makes no "
        "repr(C)/size/alignment/ABI claim, so a Rust struct here would assert nothing "
        "testable."
    ),
    # -- src/hle, 10 of the last 12 files the batch assessment examined (the
    # other two, rt64_vi.h and rt64_application.cpp, landed partially `ported`
    # in rt64_vi_registers.rs; rt64_vi.cpp stays authority-gated). This closes
    # out src/hle.
    "src/hle/rt64_application_window.cpp": _vi_registers_refusal(
        "425/425 refused: every one of its 66 arithmetic candidates is Win32 "
        "(AdjustWindowRectEx, GetMonitorInfo, EnumDisplaySettings), SDL2 or XRandR. Its "
        "only real arithmetic is a refreshRate * 2 for interlaced modes and a "
        "(refreshRate % 10) == 9 truncation hack carrying its own // FIXME -- host-display "
        "compensation with no guest meaning."
    ),
    "src/hle/rt64_state.h": _vi_registers_refusal(
        "25 includes, an External object graph of 20 raw pointers (21 counting the "
        "#if RT_ENABLED rtConfig), and 40 method declarations (including the constructor "
        "and destructor) -- zero bodies."
    ),
    "src/hle/rt64_application.h": _vi_registers_refusal(
        "Zero function bodies anywhere in the file (Core::decodeVI and every Application "
        "method are declared only); config defaults plus enums plus an SDL/Win32 "
        "ApplicationWindow::Listener override interface."
    ),
    "src/hle/rt64_shared_queue_resources.h": _vi_registers_refusal(
        "Six unconditional inline bodies (a seventh, setRtConfig, is #if RT_ENABLED-only): "
        "five are pure field assignment under std::scoped_lock<std::mutex>, but the sixth, "
        "updateMultisampling, takes no lock and instead calls "
        "renderTargetManager.destroyAll()/setMultisampling() -- RHI dispatch, not guarded "
        "field assignment."
    ),
    "src/hle/rt64_application_window.h": _vi_registers_refusal(
        "SDL/Win32 handle struct plus a pure-virtual Listener; no bodies."
    ),
    "src/hle/rt64_present_queue.h": _vi_registers_refusal(
        "Object graph: a raw std::thread*, five mutexes, three condition variables, four "
        "atomics, and a swapChainFramebuffers vector; every method is declared only."
    ),
    "src/hle/rt64_command_warning.h": _vi_registers_refusal(
        "A 3-value IndexType tag and a union of three index payloads (load/tile/call); no "
        "opcode classification, no severity logic."
    ),
    "src/hle/rt64_command_warning.cpp": _vi_registers_refusal(
        "One vsnprintf varargs formatter, CommandWarning::format."
    ),
    "src/hle/rt64_rdp_tmem.h": _vi_registers_refusal(
        "Declares TextureManager: two std::set<uint64_t> members and five method "
        "declarations, all five bodies already adjudicated in writing by "
        "rt64_hle_geometry.rs's uploadEmpty/uploadTMEM/uploadTexture/removeHashes/"
        "dumpTexture refusals (:585-593).",
        HLE_ASSESSMENT_EVIDENCE,
    ),
    "src/hle/rt64_game_configuration.h": _vi_registers_refusal(
        "Five default-initialized tunables (sunLightIntensity, sunLightDistance, "
        "estimateSunLight, rspLightAsDiffuse, rspLightIntensity); no logic."
    ),
    # -- src/hle, the last 2 of the batch: deliberately not digest-cited so
    # the scanner cannot falsely credit them (they carry no arithmetic to
    # port, but citing their SHA-256 would still register as `ported`).
    "src/hle/rt64_interpreter.h": _workload_geometry_refusal(
        "Zero bodies: five function declarations (constructor, setup, loadUCodeGBI, "
        "processRDPLists, processDisplayLists) over six data members (state, gbiManager, "
        "hleGBI, extendedOpCode, extendedFunction, and the anonymous-struct-typed UCode)."
    ),
    "src/hle/rt64_present.h": _workload_geometry_refusal(
        "Two aggregate structs, DebuggerFramebuffer and Present -- no member functions, "
        "field layout only."
    ),
    # -- src/gui and src/imgui, all 10 files the GUI batch assessment examined.
    # `rt64_camera_controller.h` and `rt64_file_dialog.h` were held back by the
    # batch that recorded the other eight, because `self_test`'s `not-started`
    # mutation fixtures then read live rows out of the committed inventory and
    # would have raised `StopIteration` once this set emptied. Those probes now
    # synthesize their own rows (`with_synthetic_not_started`), so the fixture
    # no longer depends on the port being unfinished and both files are
    # recorded here on their own re-verified reasoning.
    "src/gui/rt64_debugger_inspector.cpp": _gui_refusal(
        "532 ImGui:: call sites over 1,507 substantive lines. Its math calls -- "
        "pseudoRandom, barycentricCoordinates, nearPlaneFromProj, farPlaneFromProj, "
        "fovFromProj -- are all defined in common/rt64_math.cpp, not here; "
        "barycentricCoordinates is already ported at rt64_math.rs:225."
    ),
    "src/gui/rt64_debugger_inspector.h": _gui_refusal(
        "An 11-field struct plus 8 method declarations (including the constructor). "
        "Field layout only; Workload, VI, DrawCallKey and RenderWindow are all from "
        "uncited files."
    ),
    "src/gui/rt64_inspector.cpp": _gui_refusal(
        "Vulkan and D3D12 backend plumbing -- VkDescriptorPool, VkRenderPass, "
        "vkDestroyRenderPass, ImGui_ImplVulkan_Init, imgui_impl_dx12, imgui_impl_win32. "
        "fn64 targets wgpu; none of these APIs exist in the render crates."
    ),
    "src/gui/rt64_inspector.h": _gui_refusal(
        "Declarations for the above, over Plume/Vulkan/SDL2 handle types."
    ),
    "src/gui/rt64_file_dialog.cpp": _gui_refusal(
        "A thin wrapper over the nfd (Native File Dialog) library: NFD_Init, "
        "NFD_PickFolderN, NFD_OpenDialogN, NFD_SaveDialogN, NFD_FreePathN. Zero "
        "arithmetic in the file."
    ),
    "src/gui/rt64_file_dialog.h": _gui_refusal(
        "Declarations for the nfd wrapper above: FileDialog is one static "
        "std::atomic<bool> plus five bodiless static declarations (initialize, finish, "
        "getDirectoryPath, getOpenFilename, getSaveFilename). The file's only body is "
        "FileFilter's constructor, whose two branches assign the same two string fields "
        "and differ solely in calling win32::Utf8ToUtf16 from the excluded "
        "src/contrib/utf8conv on _WIN32. No arithmetic anywhere in the file."
    ),
    "src/gui/rt64_camera_controller.cpp": _gui_refusal(
        "All four methods (moveCursor, movePerspective, rotatePerspective, "
        "lookAtPerspective) take DebuggerCamera& and return void; their entire "
        "observable effect is mutating that 8-member struct, declared at the uncited "
        "hle/rt64_workload.h:193-202 and already refused by name at "
        "rt64_workload_geometry.rs:241. Only moveCursor is ImGui-gated -- "
        "lookAtPerspective (lines 65-75) has zero ImGui references, and neither "
        "movePerspective nor rotatePerspective reference an ImGui symbol -- so the "
        "uncited-mutated-type argument is what actually carries the refusal, not "
        "ImGui control flow."
    ),
    "src/gui/rt64_camera_controller.h": _gui_refusal(
        "Zero bodies: CameraController is one data member (hlslpp::int2 lastCursorPos) "
        "plus five declarations -- the constructor and the four methods whose "
        "definitions rt64_camera_controller.cpp already refuses. All four take "
        "DebuggerCamera&, the 8-member struct at hle/rt64_workload.h:193-202 that "
        "rt64_workload_geometry.rs refuses by name as field layout. Declaration shape "
        "only; nothing here to own that the .cpp refusal does not already settle."
    ),
    "src/imgui/imgui_impl_sdl2_custom.cpp": _gui_refusal(
        "Vendored Dear ImGui SDL2 platform backend, carrying upstream's own header and "
        "changelog. 126 SDL_ references. The workspace has no SDL2 dependency at all -- "
        "grep for sdl2 across every Cargo.toml returns nothing."
    ),
    "src/imgui/imgui_impl_sdl2_custom.h": _gui_refusal(
        "Nine IMGUI_IMPL_API function declarations over three forward-declared SDL "
        "types and one typedef; no enum and no body anywhere in the file."
    ),
}
REFUSAL_KEYS = {"commit", "evidence", "reason"}
FULL_SHA1 = re.compile(r"[0-9a-f]{40}")
# Rust-port crates only: fn64-render-rt64 is a C++ FFI shim/authority-gate
# guard crate, never itself a Rust reimplementation of RT64 source, so its
# files (including its own guard/self-tests over the C++ overlay) are never
# scanned as port evidence.
PORT_CRATE_EXCLUDED_DIR_PARTS = ("fn64-render-rt64",)
SHA256_LITERAL = re.compile(r"\b[0-9a-f]{64}\b")
TASK_KEYS = {
    "id", "outcome", "authority", "owner_lane", "recommended_profile",
    "writable_paths", "non_goals", "baseline_command", "exit_gate",
    "evidence_state", "claim_status",
}
LOCAL_PATH = re.compile(r"(?:/Users/|/home/|[A-Za-z]:\\\\)")
AUDITED_M8_PATHS = frozenset({
    "include/rt64_extended_gbi.h",
    "src/common/rt64_common.cpp",
    "src/common/rt64_common.h",
    "src/common/rt64_emulator_configuration.cpp",
    "src/common/rt64_emulator_configuration.h",
    "src/common/rt64_enhancement_configuration.cpp",
    "src/common/rt64_enhancement_configuration.h",
    "src/common/rt64_filesystem.h",
    "src/common/rt64_filesystem_directory.h",
    "src/common/rt64_filesystem_zip.cpp",
    "src/common/rt64_filesystem_zip.h",
    "src/common/rt64_hlslpp.h",
    "src/common/rt64_load_types.cpp",
    "src/common/rt64_load_types.h",
    "src/common/rt64_mapped_file.cpp",
    "src/common/rt64_mapped_file.h",
    "src/common/rt64_math.cpp",
    "src/common/rt64_math.h",
    "src/common/rt64_plume.h",
    "src/common/rt64_replacement_database.cpp",
    "src/common/rt64_replacement_database.h",
    "src/common/rt64_sommelier.h",
    "src/common/rt64_thread.cpp",
    "src/common/rt64_thread.h",
    "src/common/rt64_tmem_hasher.h",
    "src/common/rt64_user_configuration.cpp",
    "src/common/rt64_user_configuration.h",
    "src/common/rt64_user_paths.cpp",
    "src/common/rt64_user_paths.h",
    "src/gbi/rt64_gbi_extended.cpp",
    "src/gbi/rt64_gbi_extended.h",
    "src/gui/rt64_camera_controller.cpp",
    "src/gui/rt64_camera_controller.h",
    "src/gui/rt64_debugger_inspector.cpp",
    "src/gui/rt64_debugger_inspector.h",
    "src/gui/rt64_file_dialog.cpp",
    "src/gui/rt64_file_dialog.h",
    "src/gui/rt64_inspector.cpp",
    "src/gui/rt64_inspector.h",
    "src/hle/rt64_application.cpp",
    "src/hle/rt64_application.h",
    "src/hle/rt64_color_converter.cpp",
    "src/hle/rt64_color_converter.h",
    "src/hle/rt64_command_warning.cpp",
    "src/hle/rt64_command_warning.h",
    "src/hle/rt64_draw_call.cpp",
    "src/hle/rt64_draw_call.h",
    "src/hle/rt64_game_call.h",
    "src/hle/rt64_game_configuration.h",
    "src/hle/rt64_game_frame.cpp",
    "src/hle/rt64_game_frame.h",
    "src/hle/rt64_light_manager.cpp",
    "src/hle/rt64_light_manager.h",
    "src/hle/rt64_present_queue.cpp",
    "src/hle/rt64_present_queue.h",
    "src/hle/rt64_projection.cpp",
    "src/hle/rt64_projection.h",
    "src/hle/rt64_rigid_body.cpp",
    "src/hle/rt64_rigid_body.h",
    "src/hle/rt64_shared_queue_resources.h",
    "src/hle/rt64_transform_group.h",
    "src/hle/rt64_vi.cpp",
    "src/hle/rt64_vi.h",
    "src/imgui/imgui_impl_sdl2_custom.cpp",
    "src/imgui/imgui_impl_sdl2_custom.h",
    "src/preset/rt64_preset.cpp",
    "src/preset/rt64_preset.h",
    "src/preset/rt64_preset_draw_call.cpp",
    "src/preset/rt64_preset_draw_call.h",
    "src/preset/rt64_preset_inspector.h",
    "src/preset/rt64_preset_light.cpp",
    "src/preset/rt64_preset_light.h",
    "src/preset/rt64_preset_material.cpp",
    "src/preset/rt64_preset_material.h",
    "src/preset/rt64_preset_scene.cpp",
    "src/preset/rt64_preset_scene.h",
    "src/render/rt64_geometry_mode.cpp",
    "src/render/rt64_geometry_mode.h",
    "src/render/rt64_look_at_processor.cpp",
    "src/render/rt64_look_at_processor.h",
    "src/render/rt64_projection_processor.cpp",
    "src/render/rt64_projection_processor.h",
    "src/render/rt64_sampler_library.h",
    "src/render/rt64_shader_common.cpp",
    "src/render/rt64_shader_common.h",
    "src/render/rt64_shader_library.h",
    "src/render/rt64_transform_processor.cpp",
    "src/render/rt64_transform_processor.h",
    "src/render/rt64_vertex_processor.cpp",
    "src/render/rt64_vertex_processor.h",
    "src/shaders/Color.hlsli",
    "src/shaders/ComposePS.hlsl",
    "src/shaders/Constants.hlsli",
    "src/shaders/DebugPS.hlsl",
    "src/shaders/Formats.hlsli",
    "src/shaders/FullScreenVS.hlsl",
    "src/shaders/IdleCS.hlsl",
    "src/shaders/Im3DCommon.hlsli",
    "src/shaders/Im3DPS.hlsl",
    "src/shaders/Im3DVS.hlsl",
    "src/shaders/Math.hlsli",
})


class InventoryError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise InventoryError(message)


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest(path: Path) -> str:
    return digest_bytes(path.read_bytes())


def git(directory: Path, *arguments: str) -> str:
    try:
        return subprocess.run(
            ["git", "-C", str(directory), *arguments],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise InventoryError(f"git {' '.join(arguments)} failed: {detail.strip()}") from error


def load_authority() -> dict:
    try:
        authority = json.loads(AUTHORITY_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise InventoryError(f"cannot load authority manifest: {error}") from error
    require(authority.get("schema_version") == 1, "unsupported RT64 authority schema")
    require(authority.get("repository") == "https://github.com/rt64/rt64", "unexpected authority repository")
    return authority


def source_identity(authority: dict, selection: str) -> dict:
    if selection == "oracle":
        item = authority["oracle"]
        source_id = item["source_id"]
    else:
        item = authority["port_source"]
        source_id = f"git:{item['commit']}"
    return {
        "commit": item["commit"],
        "source_id": source_id,
        "authority_status": item["status"],
    }


def validate_tree(tree: Path, authority: dict, selection: str) -> None:
    identity = source_identity(authority, selection)
    require(tree.is_dir(), f"{selection} RT64 checkout does not exist")
    require(git(tree, "rev-parse", "HEAD") == identity["commit"], f"{selection} checkout is at the wrong authority pin")
    require(
        not git(tree, "status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"),
        f"{selection} checkout is dirty",
    )
    require((tree / "LICENSE").is_file(), f"{selection} RT64 checkout lacks LICENSE")
    require(digest(tree / "LICENSE") == authority["oracle"]["license_sha256"], f"{selection} RT64 LICENSE digest mismatch")
    require(digest(tree / ".gitmodules") == authority["oracle"]["gitmodules_sha256"], f"{selection} RT64 .gitmodules digest mismatch")
    plume = next(item for item in authority["submodules"] if item["path"] == "src/contrib/plume")
    plume_tree = tree / plume["path"]
    require(plume_tree.is_dir(), f"{selection} Plume submodule is not initialized")
    require(git(plume_tree, "rev-parse", "HEAD") == plume[f"{selection}_revision"], f"{selection} Plume checkout is at the wrong pin")


def allowed_authority_exceptions(authority: dict) -> set[str]:
    return {
        gate["path"]
        for gate in authority["overlays"]["source_gates"]
        if gate["path"].startswith(EXCLUDED_PREFIXES)
    }


def authority_locator(authority: dict, selection: str, relative: str) -> str:
    if relative.startswith("src/contrib/plume/"):
        plume = next(item for item in authority["submodules"] if item["path"] == "src/contrib/plume")
        nested = relative.removeprefix("src/contrib/plume/")
        return f"git:{plume[f'{selection}_revision']}:{nested}"
    return f"git:{source_identity(authority, selection)['commit']}:{relative}"


def source_paths(tree: Path, authority: dict) -> list[str]:
    tracked = git(tree, "ls-files").splitlines()
    result = {
        path
        for path in tracked
        if path.startswith(tuple(prefix + "/" for prefix in SOURCE_PREFIXES))
        and PurePosixPath(path).suffix in SUFFIXES
    }
    exceptions = allowed_authority_exceptions(authority)
    for path in exceptions:
        require((tree / path).is_file(), f"authority-gated source is missing: {path}")
        require(PurePosixPath(path).suffix in SUFFIXES, f"authority-gated source has unsupported suffix: {path}")
        result.add(path)
    require(result, "no admitted RT64 host or shader source files found")
    return sorted(result)


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return re.sub(r"//[^\n]*", "", text)


def candidate_hints(text: str) -> list[dict[str, str]]:
    """Return non-exhaustive navigation hints, never a symbol denominator."""
    clean = strip_comments(text)
    observed: set[tuple[str, str]] = set()
    for _kind, name in DECLARATION_HINT.findall(clean):
        observed.add(("type-declaration", name))
    for name in FUNCTION_DEFINITION_HINT.findall(clean):
        bare = name.rsplit("::", 1)[-1]
        if bare not in NON_FUNCTION_HINTS:
            observed.add(("function-definition", name))
    return [
        {"kind": kind, "name": name}
        for kind, name in sorted(observed, key=lambda item: (item[1], item[0]))
    ]


def dependency_paths(tree: Path, relative: str, known: set[str]) -> list[str]:
    path = tree / relative
    text = path.read_text(encoding="utf-8", errors="replace")
    result: set[str] = set()
    for include in INCLUDE.findall(text):
        candidates = (path.parent / include, tree / include, tree / "src" / include)
        for candidate in candidates:
            try:
                resolved = candidate.resolve().relative_to(tree.resolve()).as_posix()
            except ValueError:
                continue
            if resolved in known:
                result.add(resolved)
                break
    return sorted(result)


def route_for(relative: str, gates: dict[str, dict]) -> tuple[str, str, str, str, bool]:
    """Return the audited primary milestone, workstream, lane, profile, and
    whether this path is authority-gated.

    Rules are deliberately closed: a new file that does not match a named
    family traps instead of falling through to feature parity. The boolean
    is purely a path-derived fact (never a completion claim); the final
    `port_state` is computed separately from mechanically detected port
    evidence, see `ported_as_for`/`port_state_for`.
    """
    if relative in gates:
        milestone = gates[relative]["port_milestone"]
        lane = "authority-evidence" if milestone != "M10" else "gpu-render"
        profile = "M/medium" if lane == "authority-evidence" else "P/high"
        return milestone, "authority-overlay", lane, profile, True

    lower = relative.lower()
    name = PurePosixPath(lower).name
    if relative == "include/rt64_extended_gbi.h" or "gbi_extended" in lower:
        return "M8", "feature-parity", "semantic-frontend", "I/high", False
    if any(token in lower for token in ("raytracing", "globalhit", "lights.hlsli")):
        return "M12", "ray-path-tracing", "gpu-render", "I/high", False
    if lower.startswith("src/gbi/") or any(token in lower for token in ("/rt64_rsp", "microcode", "rspmodify", "rspprocess", "rspsmooth", "rspvertextest", "rspworld")):
        return "M5", "gbi-deferred-rsp", "semantic-frontend", "I/high", False
    if lower.startswith("src/shared/") and any(token in name for token in ("f3d", "point_light", "rsp_")):
        return "M5", "gbi-deferred-rsp", "semantic-frontend", "I/high", False
    if lower.startswith("src/shaders/fb") or any(token in lower for token in ("framebuffer", "/rt64_rdp", "raster", "texture", "tile_processor", "native_target", "render_target", "videointerface", "video_interface", "vi_renderer", "renderparams", "postblend", "rtcopy", "depth.hlsli", "random.hlsli", "bluenoise.hlsli", "background.hlsli", "library.hlsli")):
        return "M4", "rdp-framebuffer", "gpu-render", "I/high", False
    if lower.startswith("src/shared/") and any(token in name for token in ("blender", "color_combiner", "fb_", "other_mode", "rdp_", "render_params", "render_indices", "render_flags", "gpu_tile", "interleaved")):
        return "M4", "rdp-framebuffer", "gpu-render", "I/high", False
    if any(token in lower for token in ("state", "workload", "present.h", "interpreter.h")):
        return "M3", "raw-dpc", "semantic-frontend", "F/xhigh", False
    if lower.startswith("src/shared/") and any(token in name for token in ("extra_params", "frame_params", "hlsl")):
        return "M1", "semantic-ir", "semantic-frontend", "F/xhigh", False
    if any(token in lower for token in ("timer", "buffer_uploader", "descriptor_sets", "render_worker", "shader_compiler")):
        return "M6", "performance-spine", "integration-performance", "I/high", False
    if lower.startswith(("src/apple/", "src/rhi/")) or any(token in lower for token in ("application_window", "dynamic_libraries", "optimus")):
        return "M10", "platform-cutover", "gpu-render", "P/high", False
    if any(token in lower for token in ("upscaler", "postprocess", "histogram", "bicubic", "boxfilter", "gaussian", "luminance")):
        return "M11", "modernization", "gpu-render", "I/high", False
    if relative in AUDITED_M8_PATHS:
        return "M8", "feature-parity", "semantic-frontend", "I/high", False
    raise InventoryError(f"unrouted admitted RT64 source: {relative}")


def refusal_for(relative: str) -> dict[str, str] | None:
    """The declared, citation-carrying refusal record for a source, or None.

    Unlike `ported`, this is not derivable from bytes: no digest can witness
    a human having read a file and settled that it holds nothing to own. The
    declaration is admitted only because `verify_refusals` resolves both of
    its citations against this repository before any inventory is built.
    """
    return PORT_REFUSALS.get(relative)


def verify_refusals(root: Path) -> None:
    """Resolve every declared refusal's citations, or fail closed.

    This is the `refused` state's analogue of the whole-file SHA-256 scan that
    backs `ported`: a refusal is admitted only if a reader can follow it. The
    assessing commit must exist in this repository as a `commit` object whose
    message carries the batch's own subject line, and the cited evidence file
    must exist. An asserted refusal with a fabricated, rewritten-away, or
    absent citation raises rather than being silently believed.
    """
    resolved_subjects: dict[str, str] = {}
    for relative, record in sorted(PORT_REFUSALS.items()):
        require(set(record) == REFUSAL_KEYS, f"{relative}: refusal record fields changed")
        for key in sorted(REFUSAL_KEYS):
            value = record[key]
            require(isinstance(value, str) and value.strip(), f"{relative}: refusal {key} is empty")
        commit = record["commit"]
        require(FULL_SHA1.fullmatch(commit) is not None, f"{relative}: refusal commit is not a full SHA-1")
        evidence = record["evidence"]
        require(not evidence.startswith(("/", "~/")), f"{relative}: refusal evidence is not repository-relative")
        require((root / evidence).is_file(), f"{relative}: refusal cites a file that does not exist: {evidence}")
        if commit not in resolved_subjects:
            try:
                kind = git(root, "cat-file", "-t", commit)
            except InventoryError:
                kind = None
            require(kind == "commit", f"{relative}: refusal commit is not a commit object: {commit}")
            resolved_subjects[commit] = git(root, "log", "-1", "--format=%s", commit)
        expected_subject = {
            COMMON_ASSESSMENT_COMMIT: COMMON_ASSESSMENT_SUBJECT,
            RENDER_ASSESSMENT_COMMIT: RENDER_ASSESSMENT_SUBJECT,
            HLE_ASSESSMENT_COMMIT: HLE_ASSESSMENT_SUBJECT,
            VI_REGISTERS_ASSESSMENT_COMMIT: VI_REGISTERS_ASSESSMENT_SUBJECT,
            WORKLOAD_GEOMETRY_ASSESSMENT_COMMIT: WORKLOAD_GEOMETRY_ASSESSMENT_SUBJECT,
            GUI_ASSESSMENT_COMMIT: GUI_ASSESSMENT_SUBJECT,
        }.get(commit)
        require(expected_subject is not None, f"{relative}: refusal cites an undeclared assessment: {commit}")
        require(
            expected_subject in resolved_subjects[commit],
            f"{relative}: refusal commit {commit[:8]} is not the declared assessment",
        )


def port_state_for(gated: bool, ported_as: list[str], refusal: dict[str, str] | None = None) -> str:
    """Derive `port_state` from mechanically detected port evidence, plus the
    one declared input this tool admits.

    **Digest evidence outranks every declaration and every path-derived
    fact.** A source is `ported` when at least one Rust module in a port-target
    crate cites its exact whole-file SHA-256 digest (`ported_as_for`); that
    citation is the strongest fact this tool can observe, so it wins over both
    the `refused` declaration and the `authority-gated` path fact. Failing a
    digest, the ranking is `authority-gated`, then `refused`, then
    `not-started`.

    `authority-gated` is a path-derived source-overlay constraint, and it is
    neither completion evidence nor a veto. It records that fn64 applies
    textual overlays to this file in the native C++ build
    (`crates/fn64-render-rt64/ffi/CMakeLists.txt`), where the pinned digest is
    a build-time tripwire on the *input* bytes so a silent upstream change
    cannot make a patch land in the wrong place. It says nothing about whether
    the file's behavior has been reimplemented in Rust -- the tool schedules a
    port card with a Rust destination for every gated file -- so it cannot
    raise a digest-less file to `ported`, and equally it cannot suppress a
    real, cited port back down to `authority-gated`. The gate fact is never
    lost by that ranking: it is emitted as its own `authority_gate` record,
    keyed off the path in `authority["overlays"]["source_gates"]` rather than
    off `port_state`, and `validate_inventory` requires that record present and
    digest-matched for every gated path whatever its state.

    `refused` is the single declared state: a landed batch assessment settled
    the file as never-to-be-ported, and `verify_refusals` has resolved that
    assessment's commit and evidence file before this runs. Everything else is
    `not-started`. This never widens to a partial/behavioral claim: `refused`
    asserts an assessment happened, never that behavior is covered.
    """
    if ported_as:
        return "ported"
    if gated:
        return "authority-gated"
    return "refused" if refusal else "not-started"


def rust_port_source_files(root: Path) -> list[Path]:
    """Every `.rs` file in a Rust-port-target crate, deterministically ordered.

    `fn64-render-rt64` is excluded: it is the C++ FFI shim and
    authority-gate integrity-guard crate, never a Rust reimplementation of
    RT64 source (its own tests assert C++/CMake overlay text still contains
    an RT64 source-gate digest, which is a different fact than "this Rust
    module ports that source").
    """
    crates_dir = root / "crates"
    if not crates_dir.is_dir():
        return []
    files = [
        path
        for path in crates_dir.rglob("*.rs")
        if not (PORT_CRATE_EXCLUDED_DIR_PARTS and set(path.parts) & set(PORT_CRATE_EXCLUDED_DIR_PARTS))
    ]
    return sorted(files)


def sha256_citation_index(root: Path) -> dict[str, list[str]]:
    """Map each SHA-256 hex digest literal to the sorted Rust module paths
    (repository-relative, POSIX) that contain it verbatim.

    A whole-file SHA-256 digest is the only citation shape this tool trusts
    as a fully mechanical, human-judgment-free port signal: every basename
    or line-range citation style in this repository was, on inspection, also
    used for cross-reference/call-site/inherited/explicitly-disclaimed
    mentions that a fully mechanical scan cannot safely tell apart from a
    genuine port. Under-claiming here (missing a real port that only cites a
    partial-file line range, e.g. `endian_swap.rs`/`fbcommon.rs`/
    `rsp_math.rs`) is the deliberately safe failure mode; see
    `docs/RT64-PORT-INVENTORY.md`'s generation note.
    """
    index: dict[str, list[str]] = {}
    for path in rust_port_source_files(root):
        text = path.read_text(encoding="utf-8", errors="replace")
        digests = set(SHA256_LITERAL.findall(text))
        if not digests:
            continue
        relative = path.resolve().relative_to(root.resolve()).as_posix()
        for digest in digests:
            index.setdefault(digest, []).append(relative)
    for digest, paths in index.items():
        paths.sort()
    return index


def ported_as_for(sources: dict, citation_index: dict[str, list[str]]) -> list[str]:
    """Rust module paths that cite `sources["oracle"]`/`sources["port"]`'s
    whole-file SHA-256 digest verbatim, sorted and de-duplicated."""
    modules: set[str] = set()
    for selection in SOURCE_SELECTIONS:
        snapshot_value = sources[selection]
        if snapshot_value is None:
            continue
        modules.update(citation_index.get(snapshot_value["sha256"], ()))
    return sorted(modules)


def card_id(relative: str, milestone: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", relative.lower()).strip("-")
    return f"rt64-port-{milestone.lower()}-{slug}"


def proposed_rust_destination(relative: str, milestone: str) -> str:
    """A sensible, unique, not-yet-created destination for a source with no
    mechanically detected port. Placed flat under the target crate's `src/`,
    matching the real layout of every already-ported module in this
    repository today (no milestone-keyed subdirectory of this shape exists;
    inventing one here would just be a second, equally speculative guess
    replacing the previous fabricated `src/features/`-style paths)."""
    path = PurePosixPath(relative)
    suffix = path.suffix.lstrip(".").lower()
    stem = re.sub(r"[^a-z0-9]+", "_", path.stem.lower()).strip("_")
    if path.suffix in {".hlsl", ".hlsli"}:
        return f"crates/fn64-render-wgpu/src/{stem}_{suffix}.wgsl"
    crate = "fn64-render-ir" if milestone == "M1" else "fn64-render-wgpu"
    return f"crates/{crate}/src/{stem}_{suffix}.rs"


def snapshot(tree: Path, relative: str, known: set[str]) -> dict:
    data = (tree / relative).read_bytes()
    text = data.decode("utf-8", errors="replace")
    return {
        "sha256": digest_bytes(data),
        "lines": len(text.splitlines()),
        "candidate_hints": candidate_hints(text),
        "dependencies": dependency_paths(tree, relative, known),
    }


def delta_kind(oracle: dict | None, port: dict | None) -> str:
    if oracle is None:
        return "added"
    if port is None:
        return "removed"
    return "unchanged" if oracle["sha256"] == port["sha256"] else "modified"


def source_set_digest(files: list[dict]) -> str:
    rows = [
        {
            "path": item["path"],
            "oracle_sha256": None if item["sources"]["oracle"] is None else item["sources"]["oracle"]["sha256"],
            "port_sha256": None if item["sources"]["port"] is None else item["sources"]["port"]["sha256"],
            "port_delta": item["port_delta"],
        }
        for item in files
    ]
    return digest_bytes((json.dumps(rows, separators=(",", ":"), sort_keys=True) + "\n").encode())


def build_inventory(oracle_tree: Path, port_tree: Path, authority: dict) -> dict:
    validate_tree(oracle_tree, authority, "oracle")
    validate_tree(port_tree, authority, "port")
    verify_refusals(ROOT)
    gates = {item["path"]: item for item in authority["overlays"]["source_gates"]}
    paths_by_source = {
        "oracle": source_paths(oracle_tree, authority),
        "port": source_paths(port_tree, authority),
    }
    known_by_source = {name: set(paths) for name, paths in paths_by_source.items()}
    all_paths = sorted(known_by_source["oracle"] | known_by_source["port"])
    files: list[dict] = []
    trees = {"oracle": oracle_tree, "port": port_tree}
    citation_index = sha256_citation_index(ROOT)
    for relative in all_paths:
        milestone, workstream, owner, profile, gated = route_for(relative, gates)
        sources = {
            name: snapshot(trees[name], relative, known_by_source[name]) if relative in known_by_source[name] else None
            for name in SOURCE_SELECTIONS
        }
        ported_as = ported_as_for(sources, citation_index)
        refusal = refusal_for(relative)
        port_state = port_state_for(gated, ported_as, refusal)
        writable_paths = ported_as if ported_as else [proposed_rust_destination(relative, milestone)]
        item = {
            "path": relative,
            "sources": sources,
            "port_delta": delta_kind(sources["oracle"], sources["port"]),
            "milestone": milestone,
            "workstream": workstream,
            "port_state": port_state,
            "ported_as": ported_as,
            "evidence_state": "source-digests-verified",
            "task_card": {
                "id": card_id(relative, milestone),
                "outcome": f"Port the admitted behavior represented by {relative} into an owned Rust module without widening behavior claims.",
                "authority": {
                    "port_source": authority_locator(authority, "port", relative),
                    "comparison_oracle": authority_locator(authority, "oracle", relative),
                    "plan": "docs/RENDER-WGPU-PORT-PLAN.md",
                },
                "owner_lane": owner,
                "recommended_profile": profile,
                "writable_paths": writable_paths,
                "non_goals": [
                    "Do not edit, vendor, or transliterate the RT64 C++ source.",
                    "Do not claim parity from source translation or inventory status.",
                ],
                "baseline_command": "python3 tools/rt64_port_inventory.py --check --oracle-dir <clean-oracle> --port-dir <clean-port-source>",
                "exit_gate": f"The {milestone} behavior fixture for {relative} passes its declared differential and required 10/20-run reliability bar.",
                "evidence_state": "not-run",
                "claim_status": "candidate-observation",
            },
        }
        if port_state == "refused":
            item["port_refusal"] = dict(refusal)
        if relative in gates:
            item["authority_gate"] = {
                "mechanisms": gates[relative]["mechanisms"],
                "oracle_sha256": gates[relative]["sha256"],
            }
            require(sources["oracle"] is not None, f"authority gate absent from oracle: {relative}")
            require(sources["oracle"]["sha256"] == gates[relative]["sha256"], f"authority source-gate digest mismatch: {relative}")
        files.append(item)
    require(set(gates) <= set(all_paths), f"authority gates missing from inventory: {sorted(set(gates) - set(all_paths))}")
    counts = {kind: sum(item["port_delta"] == kind for item in files) for kind in ("added", "removed", "modified", "unchanged")}
    value = {
        "schema": SCHEMA,
        "generated_by": "tools/rt64_port_inventory.py",
        "authority_manifest": "docs/rt64-port-authority.json",
        "sources": {
            "repository": authority["repository"],
            "oracle": source_identity(authority, "oracle"),
            "port": source_identity(authority, "port"),
            "primary_port_input": "port",
        },
        "scope": {
            "included_prefixes": list(SOURCE_PREFIXES),
            "authority_gated_exceptions": sorted(allowed_authority_exceptions(authority)),
            "excluded_prefixes": list(EXCLUDED_PREFIXES),
            "exclusions": EXCLUSION_RECORDS,
            "file_extensions": sorted(SUFFIXES),
            "note": "Project-owned RT64 host/shader source plus explicitly authority-gated overlay files; all other contrib/tools trees remain excluded.",
        },
        "port_delta_counts": counts,
        "source_set_sha256": "",
        "files": files,
    }
    value["source_set_sha256"] = source_set_digest(files)
    return value


def expected_scope(authority: dict) -> dict:
    return {
        "included_prefixes": list(SOURCE_PREFIXES),
        "authority_gated_exceptions": sorted(allowed_authority_exceptions(authority)),
        "excluded_prefixes": list(EXCLUDED_PREFIXES),
        "exclusions": EXCLUSION_RECORDS,
        "file_extensions": sorted(SUFFIXES),
        "note": "Project-owned RT64 host/shader source plus explicitly authority-gated overlay files; all other contrib/tools trees remain excluded.",
    }


def assert_no_local_paths(value: object, where: str = "root") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            assert_no_local_paths(child, f"{where}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            assert_no_local_paths(child, f"{where}[{index}]")
    elif isinstance(value, str):
        require(LOCAL_PATH.search(value) is None, f"machine-local path leaked at {where}")
        require(not value.startswith(("/", "~/")), f"absolute path leaked at {where}")


def validate_snapshot(snapshot_value: object, known: set[str], label: str) -> None:
    require(isinstance(snapshot_value, dict), f"{label}: source snapshot must be an object")
    require(set(snapshot_value) == {"sha256", "lines", "candidate_hints", "dependencies"}, f"{label}: source snapshot fields changed")
    require(re.fullmatch(r"[0-9a-f]{64}", snapshot_value["sha256"]) is not None, f"{label}: invalid source digest")
    require(snapshot_value["sha256"] != "0" * 64, f"{label}: zero source digest")
    require(isinstance(snapshot_value["lines"], int) and snapshot_value["lines"] >= 0, f"{label}: invalid line count")
    hints = snapshot_value["candidate_hints"]
    require(isinstance(hints, list), f"{label}: candidate hints must be a list")
    require(hints == sorted(hints, key=lambda item: (item["name"], item["kind"])), f"{label}: candidate hints are not sorted")
    require(len({(item["kind"], item["name"]) for item in hints}) == len(hints), f"{label}: duplicate candidate hint")
    for hint in hints:
        require(set(hint) == {"kind", "name"}, f"{label}: candidate hint fields changed")
        require(hint["kind"] in {"type-declaration", "function-definition"}, f"{label}: invalid candidate hint kind")
        if hint["kind"] == "function-definition":
            require(hint["name"].rsplit("::", 1)[-1] not in NON_FUNCTION_HINTS, f"{label}: false-positive candidate hint")
    dependencies = snapshot_value["dependencies"]
    require(dependencies == sorted(set(dependencies)), f"{label}: dependencies are not sorted and unique")
    require(set(dependencies) <= known, f"{label}: dependency is absent from admitted denominator")


def validate_inventory(value: dict, authority: dict) -> None:
    expected_root = {
        "schema", "generated_by", "authority_manifest", "sources", "scope",
        "port_delta_counts", "source_set_sha256", "files",
    }
    require(set(value) == expected_root, "inventory root fields changed")
    require(value["schema"] == SCHEMA, "inventory schema changed")
    require(value["generated_by"] == "tools/rt64_port_inventory.py", "unexpected inventory generator")
    require(value["authority_manifest"] == "docs/rt64-port-authority.json", "authority manifest path changed")
    require(value["scope"] == expected_scope(authority), "inventory scope or exclusion boundary changed")
    sources = value["sources"]
    require(set(sources) == {"repository", "oracle", "port", "primary_port_input"}, "source fields changed")
    require(sources["repository"] == authority["repository"], "inventory repository drift")
    require(sources["oracle"] == source_identity(authority, "oracle"), "oracle identity drift")
    require(sources["port"] == source_identity(authority, "port"), "port-source identity drift")
    require(sources["primary_port_input"] == "port", "semantic port input drift")
    files = value["files"]
    require(isinstance(files, list) and files, "inventory contains no source files")
    paths = [item["path"] for item in files]
    require(paths == sorted(paths), "inventory file paths are not deterministically sorted")
    require(len(paths) == len(set(paths)), "duplicate inventory path")
    known = set(paths)
    known_by_source = {
        selection: {
            item["path"] for item in files
            if isinstance(item.get("sources"), dict) and item["sources"].get(selection) is not None
        }
        for selection in SOURCE_SELECTIONS
    }
    gates = {item["path"]: item for item in authority["overlays"]["source_gates"]}
    require(set(gates) <= known, f"authority gates missing from inventory: {sorted(set(gates) - known)}")
    verify_refusals(ROOT)
    require(set(PORT_REFUSALS) <= known, f"declared refusals missing from inventory: {sorted(set(PORT_REFUSALS) - known)}")
    require(not (set(PORT_REFUSALS) & set(gates)), f"refusal collides with an authority gate: {sorted(set(PORT_REFUSALS) & set(gates))}")
    citation_index = sha256_citation_index(ROOT)
    port_source_files = {
        path.resolve().relative_to(ROOT.resolve()).as_posix() for path in rust_port_source_files(ROOT)
    }
    proposed_destinations: set[str] = set()
    counts = {kind: 0 for kind in ("added", "removed", "modified", "unchanged")}
    for item in files:
        path = item["path"]
        base_keys = {"path", "sources", "port_delta", "milestone", "workstream", "port_state", "ported_as", "evidence_state", "task_card"}
        # Still an exact closed key set, deliberately widened by exactly one
        # optional key. `port_refusal` and `authority_gate` are mutually
        # exclusive because the declared refusal set and the gate set are
        # required disjoint above ("refusal collides with an authority gate"),
        # so no entry may carry both. That disjointness is the load-bearing
        # fact, not the ranking inside `port_state_for`: a gated path now reads
        # `ported` when a Rust module cites its digest, yet still emits its
        # `authority_gate` record, so the two keys are decoupled from the
        # state.
        require(
            set(item) in (base_keys, base_keys | {"authority_gate"}, base_keys | {"port_refusal"}),
            f"{path}: file entry fields changed",
        )
        in_prefix = path.startswith(tuple(prefix + "/" for prefix in SOURCE_PREFIXES))
        require(in_prefix or path in allowed_authority_exceptions(authority), f"out-of-scope source path: {path}")
        require(not path.startswith(EXCLUDED_PREFIXES) or path in allowed_authority_exceptions(authority), f"excluded path in inventory: {path}")
        require(PurePosixPath(path).suffix in SUFFIXES, f"unexpected source suffix: {path}")
        source_values = item["sources"]
        require(set(source_values) == set(SOURCE_SELECTIONS), f"{path}: source snapshots changed")
        for selection in SOURCE_SELECTIONS:
            if source_values[selection] is not None:
                validate_snapshot(source_values[selection], known_by_source[selection], f"{path}:{selection}")
        expected_delta = delta_kind(source_values["oracle"], source_values["port"])
        require(item["port_delta"] == expected_delta, f"{path}: port delta classification drift")
        counts[expected_delta] += 1
        expected_milestone, expected_workstream, expected_owner, expected_profile, expected_gated = route_for(path, gates)
        card = item["task_card"]
        require(isinstance(card, dict) and set(card) == TASK_KEYS, f"{path}: task-card fields changed")
        require(
            (item["milestone"], item["workstream"], card["owner_lane"], card["recommended_profile"]) == (expected_milestone, expected_workstream, expected_owner, expected_profile),
            f"{path}: audited route drift",
        )
        require(item["milestone"] in MILESTONES, f"{path}: invalid milestone")
        ported_as = item["ported_as"]
        require(isinstance(ported_as, list) and all(isinstance(entry, str) for entry in ported_as), f"{path}: ported_as must be a list of strings")
        require(ported_as == sorted(set(ported_as)), f"{path}: ported_as is not sorted and de-duplicated")
        expected_ported_as = ported_as_for(source_values, citation_index)
        require(ported_as == expected_ported_as, f"{path}: ported_as drift from mechanical SHA-256 citation scan")
        for module in ported_as:
            require(module in port_source_files, f"{path}: ported_as cites a Rust file that does not exist: {module}")
            require(module.startswith("crates/") and "fn64-render-rt64/" not in module, f"{path}: ported_as cites a non-port-crate module: {module}")
        require(item["port_state"] in PORT_STATES, f"{path}: invalid port_state")
        expected_refusal = refusal_for(path)
        require(
            item["port_state"] == port_state_for(expected_gated, ported_as, expected_refusal),
            f"{path}: port_state is not derived from gated status, ported_as, and the declared refusal",
        )
        if item["port_state"] == "refused":
            # The refusal must carry its evidence, exactly as `ported` must
            # carry its digest. `verify_refusals` has already resolved the
            # cited commit and file; this pins the emitted record to it.
            require("port_refusal" in item, f"{path}: refused without a citation")
            require(item["port_refusal"] == expected_refusal, f"{path}: refusal record drift from the declared table")
        else:
            require("port_refusal" not in item, f"{path}: spurious refusal record")
        require(item["evidence_state"] == "source-digests-verified", f"{path}: source evidence state drift")
        require(card["id"] == card_id(path, item["milestone"]), f"{path}: task-card id drift")
        require(set(card["authority"]) == {"port_source", "comparison_oracle", "plan"}, f"{path}: task authority fields changed")
        require(card["authority"]["port_source"] == authority_locator(authority, "port", path), f"{path}: port authority drift")
        require(card["authority"]["comparison_oracle"] == authority_locator(authority, "oracle", path), f"{path}: oracle authority drift")
        require(card["authority"]["plan"] == "docs/RENDER-WGPU-PORT-PLAN.md", f"{path}: plan authority drift")
        require(isinstance(card["outcome"], str) and path in card["outcome"], f"{path}: task outcome is not source-bound")
        require(card["non_goals"] and all(isinstance(text, str) for text in card["non_goals"]), f"{path}: task non-goals missing")
        require(card["baseline_command"].startswith("python3 tools/rt64_port_inventory.py --check"), f"{path}: baseline command drift")
        require(isinstance(card["exit_gate"], str) and item["milestone"] in card["exit_gate"], f"{path}: exit gate is not milestone-bound")
        require(card["evidence_state"] == "not-run", f"{path}: task evidence state drift")
        require(card["claim_status"] == "candidate-observation", f"{path}: task claim status drift")
        writable = card["writable_paths"]
        expected_writable = ported_as if ported_as else [proposed_rust_destination(path, item["milestone"])]
        require(writable == expected_writable, f"{path}: Rust writable destination drift")
        require(writable, f"{path}: task has no writable destination")
        for destination in writable:
            require(destination.startswith("crates/fn64-render"), f"{path}: task does not target Rust renderer source")
        if not ported_as:
            destination = writable[0]
            require(destination not in proposed_destinations, f"duplicate proposed writable destination: {destination}")
            proposed_destinations.add(destination)
        if path in gates:
            require("authority_gate" in item, f"{path}: authority gate metadata missing")
            require(item["authority_gate"] == {"mechanisms": gates[path]["mechanisms"], "oracle_sha256": gates[path]["sha256"]}, f"{path}: authority gate drift")
            require(source_values["oracle"] is not None and source_values["oracle"]["sha256"] == gates[path]["sha256"], f"{path}: authority oracle digest mismatch")
        else:
            require("authority_gate" not in item, f"{path}: spurious authority gate")
    require(value["port_delta_counts"] == counts, "port delta counts drift")
    require(re.fullmatch(r"[0-9a-f]{64}", value["source_set_sha256"]) is not None, "invalid source-set digest")
    require(value["source_set_sha256"] == source_set_digest(files), "source-set digest mismatch")
    require(value["source_set_sha256"] == EXPECTED_SOURCE_SET_SHA256, "pinned source-set digest drift")
    assert_no_local_paths(value)


def markdown(inventory: dict) -> str:
    files = inventory["files"]
    totals: dict[str, tuple[int, int]] = {}
    for item in files:
        primary = item["sources"]["port"] or item["sources"]["oracle"]
        count, lines = totals.get(item["milestone"], (0, 0))
        totals[item["milestone"]] = count + 1, lines + primary["lines"]
    sources = inventory["sources"]
    delta = inventory["port_delta_counts"]
    state_counts = {state: 0 for state in sorted(PORT_STATES)}
    for item in files:
        state_counts[item["port_state"]] += 1
    output = [
        "# RT64 port inventory", "",
        "<!-- Generated by tools/rt64_port_inventory.py from two admitted clean checkouts and docs/rt64-port-authority.json. -->", "",
        "This is the dual-pin mechanical work denominator for the RT64-to-Rust program. It records source identities, port deltas, include edges, non-exhaustive navigation hints, mechanically detected port evidence, and dispatch-card contracts. It is not a behavior or parity claim.", "",
        "Regenerate or source-check it from explicit clean checkouts:", "",
        "```sh",
        "python3 tools/rt64_port_inventory.py --oracle-dir /absolute/path/to/clean/oracle --port-dir /absolute/path/to/clean/port-source",
        "python3 tools/rt64_port_inventory.py --check --oracle-dir /absolute/path/to/clean/oracle --port-dir /absolute/path/to/clean/port-source",
        "```", "",
        f"- Executable comparison oracle: [`{sources['oracle']['commit'][:7]}`]({sources['repository']}/commit/{sources['oracle']['commit']}) (`{sources['oracle']['authority_status']}`).",
        f"- Primary semantic port input: [`{sources['port']['commit'][:7]}`]({sources['repository']}/commit/{sources['port']['commit']}) (`{sources['port']['authority_status']}`).",
        f"- Denominator: {len(files)} project-owned or explicitly authority-gated host/shader files; `{sum((item['sources']['port'] or item['sources']['oracle'])['lines'] for item in files) / 1000:.3f}` KLOC at the primary port pin.",
        f"- Port delta: {delta['added']} added, {delta['removed']} removed, {delta['modified']} modified, {delta['unchanged']} unchanged source files.",
        f"- Port state: {state_counts['ported']} `ported`, {state_counts['not-started']} `not-started`, {state_counts['refused']} `refused`, {state_counts['authority-gated']} `authority-gated` (of {len(files)}).",
        f"- Source-set SHA-256: `{inventory['source_set_sha256']}`.",
        "- Excluded: all other `src/contrib/**` and `src/tools/**`. `src/tools/texture_hasher` and its GLIDEN64/Rice lineage, GPL `src/contrib/mupen64plus-core`, and m2c are never read as port authority.",
        "- Paths are repository-relative; the checked artifact rejects machine-local paths.", "",
        "`candidate_hints` in the JSON are deliberately non-exhaustive regex navigation aids, not a symbol denominator.", "",
        "`port_state` is mechanically derived from digests and paths, with one declared exception. `ported` outranks everything: it means at least one Rust module under `crates/**/*.rs` (excluding the `fn64-render-rt64` C++ FFI shim/guard crate) contains this source's exact whole-file SHA-256 digest verbatim, listed in `ported_as`. Failing a digest, the ranking is `authority-gated`, then `refused`, then `not-started`. `authority-gated` is a path-derived source-overlay constraint (never completion evidence, and never a veto either): it records that fn64 patches this file textually in the native C++ build, where the pinned digest is a build-time tripwire on the input bytes, so it can neither raise a digest-less file to `ported` nor suppress a genuinely cited port. A gated file keeps emitting its `authority_gate` record whatever its `port_state`. This is deliberately a conservative under-count: a Rust module that cites only a basename or a partial-file line range (not the whole-file digest) does not flip a source to `ported`, because this repository was found, on inspection, to also use that citation shape for cross-reference and explicitly-disclaimed non-port mentions that cannot be mechanically told apart from a genuine port. Every task remains a candidate observation until its card exit gate and reliability bar pass, regardless of `port_state`.", "",
        "`refused` is the single **declared** state, and the only one no digest can witness -- nothing in a byte stream proves a human read a file and settled that it holds nothing worth owning. It is admitted only because it carries its evidence: each refused entry emits a `port_refusal` record naming the assessing commit and a repository-relative evidence file, and the generator resolves both (the commit must exist as a `commit` object carrying that assessment's own subject line; the evidence file must exist) before any inventory is built. A refusal asserted with no citation, a fabricated or rewritten-away commit, or an absent evidence file fails closed, exactly as a `ported` claim with no digest does. Digest evidence still outranks the declaration: a file some Rust module actually cites reads `ported`, never `refused`. `refused` asserts only that a landed assessment settled the file as never-to-be-ported -- it is **not** a partial or behavioral claim, and it credits no line as covered. The declared set is closed at the six landed batch assessments of `src/common` (17 files), `src/render` (28 files), `src/hle` (18 files), and `src/gui`/`src/imgui` (10 files).", "",
        "## Milestone denominator", "", "| milestone | files | primary-port KLOC |", "|---|---:|---:|",
    ]
    for milestone in sorted(totals, key=lambda item: int(item[1:])):
        count, lines = totals[milestone]
        output.append(f"| `{milestone}` | {count} | `{lines / 1000:.3f}` |")
    output.extend(["", "## Source work cards", "", "Each row is one source-bound candidate card with its mechanically derived port state and writable destination(s). JSON carries its outcome, both authorities, exact destination(s), non-goals, baseline, exit gate, evidence state, and candidate-vs-claim status.", "", "| source | delta | port state | ported as / refusal citation | lines | hints | deps | milestone / workstream | source evidence | task evidence / claim | owner | card |", "|---|---|---|---|---:|---:|---:|---|---|---|---|---|"])
    for item in files:
        primary = item["sources"]["port"] or item["sources"]["oracle"]
        card = item["task_card"]
        ported_as = ", ".join(f"`{module}`" for module in item["ported_as"]) or "--"
        if "port_refusal" in item:
            # A refused row carries its citation in the table itself, so the
            # burndown reader can check it without opening the JSON.
            refusal = item["port_refusal"]
            ported_as = f"refused by `{refusal['commit'][:8]}`, see `{refusal['evidence']}`"
        output.append(
            f"| `{item['path']}` | `{item['port_delta']}` | `{item['port_state']}` | {ported_as} | {primary['lines']} | {len(primary['candidate_hints'])} | {len(primary['dependencies'])} | "
            f"`{item['milestone']}` / `{item['workstream']}` | `{item['evidence_state']}` | `{card['evidence_state']}` / `{card['claim_status']}` | "
            f"`{card['owner_lane']}` ({card['recommended_profile']}) | `{card['id']}` |"
        )
    output.extend(["", "`authority-gated` is a source-overlay constraint, never completion evidence -- and never a veto: digest evidence outranks it, so a gated file some Rust module actually cites reads `ported` while still carrying its `authority_gate` record. A `refused` row's `ported as` cell names the assessing commit and evidence file instead of a module, because a refusal is a documented human judgement and must be checkable; it credits no line as ported. Every task remains a candidate observation until its card exit gate and reliability bar pass.", ""])
    return "\n".join(output)


def canonical(value: dict) -> str:
    return json.dumps(value, indent=2, sort_keys=False) + "\n"


def expect_rejected(value: dict, authority: dict, needle: str) -> None:
    try:
        validate_inventory(value, authority)
    except InventoryError as error:
        require(needle in str(error), f"mutation failed for the wrong reason: {error}")
    else:
        raise InventoryError(f"mutation was accepted; expected {needle!r}")


# Self-test fixture paths. These are not RT64 sources and never appear in a
# generated inventory; they exist only so `self_test`'s `not-started` probes
# own their fixture instead of borrowing a live row. Each name is chosen to
# route through a real, named family in `route_for` (both hit the `workload`
# token -> M3/raw-dpc), so a synthesized entry is a genuinely representative
# `not-started` row rather than a routing special case, and `route_for`
# staying closed is still exercised by the unrouted-path probe above.
SELF_TEST_PROBE_PATHS = (
    "src/render/rt64_zzz_self_test_workload_probe_one.h",
    "src/render/rt64_zzz_self_test_workload_probe_two.h",
)


def synthetic_not_started(authority: dict, relative: str) -> dict:
    """Build a well-formed `not-started` inventory entry for `relative`.

    The `not-started` mutation probes used to pull live rows out of the
    committed inventory and require them to still be `not-started`. That made
    a guard's coverage a function of how far the port had progressed: as the
    real `not-started` set drains toward zero -- which is the whole point of
    the program -- those probes lose their fixture and the self-test fails
    because the project succeeded, not because a guard broke. So the probes
    construct what they need instead.

    Every derived field is produced by the same helpers `build_inventory`
    uses (`route_for`, `card_id`, `proposed_rust_destination`,
    `authority_locator`), so the fixture cannot drift away from the shape
    `validate_inventory` demands, and a future change to any of those rules
    updates the fixture with the production path rather than silently
    leaving a stale hand-written literal behind. The digest is derived from
    the path so it is stable, nonzero, and -- being no real file's content
    hash -- cited by no Rust module, which is exactly what makes the entry
    legitimately `not-started` under `ported_as_for`/`port_state_for`.
    """
    require(relative not in PORT_REFUSALS, f"self-test fixture path is a declared refusal: {relative}")
    gates = {item["path"]: item for item in authority["overlays"]["source_gates"]}
    require(relative not in gates, f"self-test fixture path is an authority gate: {relative}")
    milestone, workstream, owner, profile, gated = route_for(relative, gates)
    require(not gated, f"self-test fixture path is authority-gated: {relative}")
    snapshot_value = {
        "sha256": digest_bytes(f"fn64 self-test fixture: {relative}".encode()),
        "lines": 1,
        "candidate_hints": [],
        "dependencies": [],
    }
    require(port_state_for(gated, [], None) == "not-started", "synthetic fixture is not not-started")
    return {
        "path": relative,
        "sources": {name: copy.deepcopy(snapshot_value) for name in SOURCE_SELECTIONS},
        "port_delta": "unchanged",
        "milestone": milestone,
        "workstream": workstream,
        "port_state": "not-started",
        "ported_as": [],
        "evidence_state": "source-digests-verified",
        "task_card": {
            "id": card_id(relative, milestone),
            "outcome": f"Port the admitted behavior represented by {relative} into an owned Rust module without widening behavior claims.",
            "authority": {
                "port_source": authority_locator(authority, "port", relative),
                "comparison_oracle": authority_locator(authority, "oracle", relative),
                "plan": "docs/RENDER-WGPU-PORT-PLAN.md",
            },
            "owner_lane": owner,
            "recommended_profile": profile,
            "writable_paths": [proposed_rust_destination(relative, milestone)],
            "non_goals": [
                "Do not edit, vendor, or transliterate the RT64 C++ source.",
                "Do not claim parity from source translation or inventory status.",
            ],
            "baseline_command": "python3 tools/rt64_port_inventory.py --check --oracle-dir <clean-oracle> --port-dir <clean-port-source>",
            "exit_gate": f"The {milestone} behavior fixture for {relative} passes its declared differential and required 10/20-run reliability bar.",
            "evidence_state": "not-run",
            "claim_status": "candidate-observation",
        },
    }


def with_synthetic_not_started(base: dict, authority: dict, count: int) -> tuple[dict, list[dict]]:
    """A deep copy of `base` carrying `count` synthesized `not-started` rows.

    Returns the fixture and the inserted rows, so a probe can mutate a row it
    owns. Rows are inserted in sorted position because `validate_inventory`
    requires deterministic path order, and the whole-inventory
    `source_set_sha256` and `port_delta_counts` guards run only *after* the
    per-file loop -- so a per-file mutation still raises its own message
    first. The probes below assert on that message, so this is checked, not
    assumed: `expect_rejected` fails loudly if a fixture's own bookkeeping
    drift ever masks the mutation under test.
    """
    require(count <= len(SELF_TEST_PROBE_PATHS), "not enough declared self-test fixture paths")
    fixture = copy.deepcopy(base)
    known = {item["path"] for item in fixture["files"]}
    added = []
    for relative in SELF_TEST_PROBE_PATHS[:count]:
        require(relative not in known, f"self-test fixture path collides with a real source: {relative}")
        entry = synthetic_not_started(authority, relative)
        fixture["files"].append(entry)
        added.append(entry)
    fixture["files"].sort(key=lambda item: item["path"])
    fixture["port_delta_counts"]["unchanged"] += count
    return fixture, added


def expect_fixture_only_rejection(fixture: dict, authority: dict) -> None:
    """Assert an unmutated synthetic fixture is rejected *only* by the
    whole-inventory digest pin, never by a per-file guard.

    This is what makes the `not-started` probes trustworthy. Each of them
    asserts a specific per-file message; if a synthesized row were itself
    malformed, that row could raise the probe's expected message on its own
    and the probe would pass while testing nothing. Pinning the pre-mutation
    failure to `source-set digest mismatch` proves every per-file guard --
    routing, task card, writable destination, ported_as, port_state -- is
    already satisfied, so any per-file message a probe then observes can only
    have come from the mutation that probe applied.
    """
    expect_rejected(fixture, authority, "source-set digest mismatch")


def self_test() -> None:
    authority = load_authority()
    gates = {item["path"]: item for item in authority["overlays"]["source_gates"]}
    for unknown in ("src/common/new_unreviewed.cpp", "src/gui/new_unreviewed.h"):
        try:
            route_for(unknown, gates)
        except InventoryError as error:
            require("unrouted admitted" in str(error), "unknown M8 source was rejected for the wrong reason")
        else:
            raise InventoryError("unknown source silently fell through to M8")
    require(DEFAULT_JSON.is_file(), "committed inventory is required for mutation self-tests")
    base = json.loads(DEFAULT_JSON.read_text(encoding="utf-8"))
    validate_inventory(base, authority)
    mutated = copy.deepcopy(base)
    mutated["files"].reverse()
    expect_rejected(mutated, authority, "not deterministically sorted")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["sources"]["oracle"]["sha256"] = "0" * 64
    expect_rejected(mutated, authority, "zero source digest")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["sources"]["oracle"]["sha256"] = "f" * 64
    mutated["files"][0]["port_delta"] = "modified"
    mutated["port_delta_counts"]["unchanged"] -= 1
    mutated["port_delta_counts"]["modified"] += 1
    expect_rejected(mutated, authority, "source-set digest mismatch")
    mutated["source_set_sha256"] = source_set_digest(mutated["files"])
    expect_rejected(mutated, authority, "pinned source-set digest drift")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["sources"]["oracle"]["dependencies"] = ["src/shared/not-real.h"]
    expect_rejected(mutated, authority, "dependency is absent")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["task_card"]["writable_paths"] = ["/Users/example/private.rs"]
    expect_rejected(mutated, authority, "Rust writable destination drift")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["task_card"]["outcome"] += " /Users/example/private.rs"
    expect_rejected(mutated, authority, "machine-local path leaked")
    mutated = copy.deepcopy(base)
    gate = authority["overlays"]["source_gates"][0]["path"]
    mutated["files"] = [item for item in mutated["files"] if item["path"] != gate]
    expect_rejected(mutated, authority, "authority gates missing")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["task_card"].pop("exit_gate")
    expect_rejected(mutated, authority, "task-card fields changed")
    mutated = copy.deepcopy(base)
    routed = next(item for item in mutated["files"] if item["milestone"] == "M4" and "authority_gate" not in item)
    routed["milestone"] = "M8"
    expect_rejected(mutated, authority, "audited route drift")
    mutated = copy.deepcopy(base)
    ported = next(item for item in mutated["files"] if item["ported_as"])
    ported["ported_as"] = sorted(set(ported["ported_as"]) | {"crates/fn64-render-wgpu/src/zzz_not_a_real_module.rs"})
    expect_rejected(mutated, authority, "ported_as drift from mechanical SHA-256 citation scan")
    mutated = copy.deepcopy(base)
    ported = next(item for item in mutated["files"] if item["ported_as"])
    ported["port_state"] = "not-started"
    expect_rejected(mutated, authority, "port_state is not derived from gated status, ported_as, and the declared refusal")
    # Digest evidence outranks the authority gate. The gate pins the *input*
    # bytes fn64's native C++ build patches textually; it is not a prohibition
    # on porting, and the tool schedules a Rust destination for every gated
    # file. So a gated source a Rust module actually cites must read `ported`,
    # while a gated source with no citation stays `authority-gated`. Asserted
    # on `port_state_for` directly, because no real gated file currently
    # carries a digest citation and the committed inventory therefore cannot
    # supply the positive fixture -- the same reason the `not-started` probes
    # above synthesize theirs. Reverting the two branches breaks the first
    # assertion.
    require(
        port_state_for(True, ["crates/fn64-render-wgpu/src/rt64_math.rs"], None) == "ported",
        "a cited digest must outrank the authority gate",
    )
    require(
        port_state_for(True, [], None) == "authority-gated",
        "an uncited authority-gated source must stay authority-gated",
    )
    # And the gate must outrank a declared refusal it can never actually
    # collide with (they are required disjoint), pinning the middle of the
    # ranking rather than leaving it to accident.
    require(
        port_state_for(True, [], PORT_REFUSALS[min(PORT_REFUSALS)]) == "authority-gated",
        "the authority gate must outrank a declared refusal",
    )
    # End-to-end: a real gated row that claims `ported` with no citation is
    # still rejected, so the reordering did not make the gated state
    # self-certifying.
    mutated = copy.deepcopy(base)
    gated_row = next(item for item in mutated["files"] if item["port_state"] == "authority-gated")
    require(not gated_row["ported_as"], "gated probe fixture must start with no port evidence")
    gated_row["port_state"] = "ported"
    expect_rejected(mutated, authority, "port_state is not derived from gated status, ported_as, and the declared refusal")
    # A `not-started` row may not fabricate port evidence: claiming a Rust
    # module that does not cite this source's digest must fail the mechanical
    # scan. The fixture is synthesized rather than borrowed from the committed
    # inventory, so this guard keeps its coverage after the real `not-started`
    # set drains to zero.
    mutated, (not_started,) = with_synthetic_not_started(base, authority, 1)
    expect_fixture_only_rejection(mutated, authority)
    not_started["ported_as"] = ["crates/fn64-render-wgpu/src/rt64_math.rs"]
    expect_rejected(mutated, authority, "ported_as drift from mechanical SHA-256 citation scan")
    # A refusal must carry its evidence, exactly as `ported` must carry its
    # digest. Each mutation below is one way a reader could be lied to.
    mutated = copy.deepcopy(base)
    refused = next(item for item in mutated["files"] if item["port_state"] == "refused")
    refused.pop("port_refusal")
    expect_rejected(mutated, authority, "refused without a citation")
    mutated = copy.deepcopy(base)
    refused = next(item for item in mutated["files"] if item["port_state"] == "refused")
    refused["port_refusal"] = dict(refused["port_refusal"], commit="0" * 40)
    expect_rejected(mutated, authority, "refusal record drift from the declared table")
    # An *undeclared* refusal -- `refused` asserted for a path that carries no
    # entry in `PORT_REFUSALS` -- must be refused, or the one declared state
    # would be self-certifying. Synthesized fixture, same reason as above.
    mutated, (undeclared,) = with_synthetic_not_started(base, authority, 1)
    expect_fixture_only_rejection(mutated, authority)
    undeclared["port_state"] = "refused"
    expect_rejected(mutated, authority, "port_state is not derived from gated status, ported_as, and the declared refusal")
    mutated = copy.deepcopy(base)
    refused = next(item for item in mutated["files"] if item["port_state"] == "refused")
    refused["port_state"] = "not-started"
    expect_rejected(mutated, authority, "port_state is not derived from gated status, ported_as, and the declared refusal")
    mutated = copy.deepcopy(base)
    plain = next(item for item in mutated["files"] if item["port_state"] == "ported")
    plain["port_refusal"] = {"commit": "0" * 40, "evidence": "docs/RT64-PORT-AUTHORITY.md", "reason": "asserted"}
    expect_rejected(mutated, authority, "spurious refusal record")
    # And a third key alongside both optional ones is still rejected outright,
    # so widening the closed key set by one did not open it.
    mutated = copy.deepcopy(base)
    mutated["files"][0]["port_note"] = "asserted"
    expect_rejected(mutated, authority, "file entry fields changed")
    # And the declaration itself must resolve: a refusal with no citation, an
    # unresolvable commit, or a missing evidence file must fail closed.
    for broken, needle in (
        ({"commit": RENDER_ASSESSMENT_COMMIT, "evidence": RENDER_ASSESSMENT_EVIDENCE}, "refusal record fields changed"),
        ({"commit": RENDER_ASSESSMENT_COMMIT, "evidence": RENDER_ASSESSMENT_EVIDENCE, "reason": "  "}, "refusal reason is empty"),
        ({"commit": "0" * 40, "evidence": RENDER_ASSESSMENT_EVIDENCE, "reason": "asserted"}, "refusal commit is not a commit object"),
        ({"commit": "not-a-sha", "evidence": RENDER_ASSESSMENT_EVIDENCE, "reason": "asserted"}, "refusal commit is not a full SHA-1"),
        ({"commit": RENDER_ASSESSMENT_COMMIT, "evidence": "docs/does-not-exist.md", "reason": "asserted"}, "refusal cites a file that does not exist"),
        ({"commit": RENDER_ASSESSMENT_COMMIT, "evidence": "/Users/example/notes.md", "reason": "asserted"}, "refusal evidence is not repository-relative"),
    ):
        probe = "src/render/zzz_probe_only.h"
        PORT_REFUSALS[probe] = broken
        try:
            verify_refusals(ROOT)
        except InventoryError as error:
            require(needle in str(error), f"uncited refusal rejected for the wrong reason: {error}")
        else:
            raise InventoryError(f"uncited refusal was accepted; expected {needle!r}")
        finally:
            del PORT_REFUSALS[probe]
    # Two un-ported sources may not be pointed at the same proposed Rust
    # destination: for a `not-started` row the writable path is derived, so
    # borrowing another row's destination is drift. This needs two rows that
    # both lack port evidence, which is precisely the fixture the real
    # inventory stops being able to supply once the port finishes; both are
    # synthesized.
    mutated, (first, second) = with_synthetic_not_started(base, authority, 2)
    expect_fixture_only_rejection(mutated, authority)
    require(
        first["task_card"]["writable_paths"] != second["task_card"]["writable_paths"],
        "self-test fixture rows must start with distinct writable destinations",
    )
    first["task_card"]["writable_paths"] = list(second["task_card"]["writable_paths"])
    expect_rejected(mutated, authority, "Rust writable destination drift")

    with tempfile.TemporaryDirectory() as temporary:
        tree = Path(temporary) / "rt64"
        tree.mkdir()
        subprocess.run(["git", "init", "-q", str(tree)], check=True)
        (tree / "LICENSE").write_text("fixture", encoding="utf-8")
        (tree / ".gitmodules").write_text("fixture", encoding="utf-8")
        (tree / "src/contrib/plume").mkdir(parents=True)
        subprocess.run(["git", "-C", str(tree), "add", "."], check=True)
        subprocess.run(["git", "-C", str(tree), "-c", "user.name=fn64", "-c", "user.email=fn64@example.invalid", "commit", "-qm", "fixture"], check=True)
        try:
            validate_tree(tree, authority, "oracle")
        except InventoryError as error:
            require("wrong authority pin" in str(error), "wrong-pin mutation was not rejected")
        else:
            raise InventoryError("wrong-pin mutation was accepted")
        dirty_authority = copy.deepcopy(authority)
        fixture_head = git(tree, "rev-parse", "HEAD")
        dirty_authority["oracle"]["commit"] = fixture_head
        dirty_authority["oracle"]["source_id"] = f"git:{fixture_head}"
        dirty_authority["oracle"]["license_sha256"] = digest(tree / "LICENSE")
        dirty_authority["oracle"]["gitmodules_sha256"] = digest(tree / ".gitmodules")
        (tree / "LICENSE").write_text("dirty", encoding="utf-8")
        try:
            validate_tree(tree, dirty_authority, "oracle")
        except InventoryError as error:
            require("checkout is dirty" in str(error), "dirty-tree mutation was not rejected")
        else:
            raise InventoryError("dirty-tree mutation was accepted")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oracle-dir", type=Path, help="explicit clean executable-oracle checkout")
    parser.add_argument("--port-dir", type=Path, help="explicit clean accepted port-source checkout")
    parser.add_argument("--output", type=Path, default=DEFAULT_JSON, help="JSON inventory output path")
    parser.add_argument("--markdown-output", type=Path, default=DEFAULT_DOC, help="generated Markdown report path")
    parser.add_argument("--check", action="store_true", help="structurally check outputs; with both source dirs also rederive every byte")
    parser.add_argument("--self-test", action="store_true", help="run fail-closed mutation tests")
    arguments = parser.parse_args()
    try:
        if arguments.self_test:
            self_test()
            print("rt64-port-inventory: mutation self-tests clean")
            return 0
        authority = load_authority()
        supplied = (arguments.oracle_dir is not None, arguments.port_dir is not None)
        require(supplied[0] == supplied[1], "--oracle-dir and --port-dir must be supplied together")
        if supplied[0]:
            value = build_inventory(arguments.oracle_dir.resolve(), arguments.port_dir.resolve(), authority)
            validate_inventory(value, authority)
            expected_json = canonical(value)
            expected_doc = markdown(value)
            if arguments.check:
                require(arguments.output.is_file(), f"inventory is missing: {arguments.output}")
                require(arguments.output.read_text(encoding="utf-8") == expected_json, "inventory is stale; regenerate from both admitted checkouts")
                require(arguments.markdown_output.is_file(), f"generated report is missing: {arguments.markdown_output}")
                require(arguments.markdown_output.read_text(encoding="utf-8") == expected_doc, "generated report is stale; regenerate from both admitted checkouts")
            else:
                arguments.output.write_text(expected_json, encoding="utf-8")
                arguments.markdown_output.write_text(expected_doc, encoding="utf-8")
        else:
            require(arguments.check, "both source directories are required to generate; this tool never guesses machine-local checkouts")
            require(arguments.output.is_file(), f"inventory is missing: {arguments.output}")
            value = json.loads(arguments.output.read_text(encoding="utf-8"))
            validate_inventory(value, authority)
            require(arguments.markdown_output.is_file(), f"generated report is missing: {arguments.markdown_output}")
            require(arguments.markdown_output.read_text(encoding="utf-8") == markdown(value), "generated report is stale; regenerate from both admitted checkouts")
    except (InventoryError, OSError, json.JSONDecodeError, KeyError, TypeError) as error:
        print(f"rt64-port-inventory: {error}", file=sys.stderr)
        return 1
    print("rt64-port-inventory: clean")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
