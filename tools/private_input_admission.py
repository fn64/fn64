#!/usr/bin/env python3
"""Admit private fn64 inputs without copying content or identities into git."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


MANIFEST_SCHEMA = "fn64.private-input-admission.v3"
READINESS_SCHEMA = "fn64.private-input-readiness.v3"
PURPOSES = {"extended_gbi", "full_rom", "combined"}
WIRE_FAMILIES = {
    "f3dex2_extended_gbi_v1",
    "f3dex2",
    "fast3d_f3dex",
    "s2dex_s2dex2",
    "full_rom_mixed",
}
EXTENDED_CASES = {
    "hook-control",
    "disabled-negative-control",
    "activation",
    "widescreen",
    "interpolation",
    "vertex-z",
}
PLATFORMS = {"macos_arm64", "linux_x86_64", "windows_x86_64"}
CONTROLLERS = {
    "standard_controller", "controller_pak", "rumble_pak", "transfer_pak",
    "voice_recognition_unit",
}
SAVES = {
    "no_cartridge_save", "eeprom_4_kbit", "eeprom_16_kbit",
    "sram_32_kib", "flash_ram_128_kib",
}
RENDERERS = {
    "reference_lle_accuracy", "rt64_lle_accuracy", "rt64_post_vi_capture",
    "rt64_replacement_packs",
}
ROLE_PROVENANCE = {
    "microcode_text": {"user_owned_rom_derived"},
    "microcode_data": {"user_owned_rom_derived"},
    "rom": {"user_owned_rom"},
    "recompiled": {"user_generated_from_owned_rom"},
}
ROOT_FIELDS = {"schema", "purpose", "intent", "release_matrix", "artifacts"}
PROGRAM_EVIDENCE_LANES = {
    "no_program_fixture", "identified_native_archive",
    "typed_observed_function", "typed_block_program",
}
INTENT_FIELDS = {
    "wire_family", "report_scenario", "recognition", "extended_gbi_cases",
    "program_evidence_lane",
}
RELEASE_FIELDS = {"platform", "controllers", "save", "renderers", "repeat_bar"}
ARTIFACT_FIELDS = {"microcode_text", "microcode_data", "rom", "recompiled"}
FILE_FIELDS = {"path", "length", "sha256", "provenance", "git_identity"}
READINESS_FIELDS = {
    "schema", "status", "purpose", "wire_family", "report_scenario",
    "artifact_roles_admitted", "extended_gbi_fixture", "full_rom_inputs",
    "release_matrix_policy", "repeat_bar", "required_extended_cases",
    "platform", "controllers", "save", "renderers", "program_evidence_lane",
}
MAX_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
SCENARIO_RE = re.compile(r"[a-z0-9][a-z0-9._-]{0,127}\Z")


class AdmissionError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AdmissionError(message)


def load_json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise AdmissionError(f"cannot read {path}: {error}") from error
    require(isinstance(value, dict), f"{path}: root must be an object")
    return value


def nonempty(value: object, where: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{where} must be nonempty")
    return value


def unique_strings(value: object, where: str) -> list[str]:
    require(isinstance(value, list), f"{where} must be an array")
    require(all(isinstance(item, str) and item for item in value), f"{where} entries must be nonempty strings")
    require(len(value) == len(set(value)), f"{where} contains duplicates")
    return value


def validate_sha256(value: object, where: str) -> str:
    require(isinstance(value, str) and len(value) == 64, f"{where} must be a SHA-256")
    require(all(character in "0123456789abcdef" for character in value), f"{where} must be lowercase hexadecimal")
    return value


def path_components(path: Path) -> list[Path]:
    components: list[Path] = []
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        components.append(current)
    return components


def reject_symlink_components(path: Path, include_leaf: bool) -> None:
    components = path_components(path)
    if not include_leaf and components:
        components.pop()
    for component in components:
        if component.exists() or component.is_symlink():
            require(not component.is_symlink(), f"symlink path component is forbidden: {component}")


def git_excluded(path: Path, root: Path) -> bool:
    try:
        relative = path.relative_to(root)
    except ValueError:
        return True
    tracked = subprocess.run(
        ["git", "ls-files", "--error-unmatch", "--", str(relative)],
        cwd=root,
        capture_output=True,
    ).returncode == 0
    if tracked:
        return False
    return subprocess.run(
        ["git", "check-ignore", "-q", "--no-index", "--", str(relative)],
        cwd=root,
        capture_output=True,
    ).returncode == 0


def validate_local_regular_file(path_text: object, where: str, root: Path) -> Path:
    path = Path(nonempty(path_text, f"{where}.path"))
    require(path.is_absolute(), f"{where}.path must be absolute")
    require(".." not in path.parts, f"{where}.path must not contain '..'")
    reject_symlink_components(path, include_leaf=True)
    try:
        mode = os.lstat(path).st_mode
    except OSError as error:
        raise AdmissionError(f"{where}.path cannot be inspected: {error}") from error
    require(stat.S_ISREG(mode), f"{where}.path must be a regular file")
    require(git_excluded(path, root), f"{where}.path is inside the repository and not gitignored")
    return path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while block := file.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def validate_artifact(role: str, descriptor: object, root: Path) -> Path:
    require(isinstance(descriptor, dict) and set(descriptor) == FILE_FIELDS, f"artifacts.{role}: invalid fields")
    require(descriptor["git_identity"] == "excluded", f"artifacts.{role}.git_identity must be 'excluded'")
    require(descriptor["provenance"] in ROLE_PROVENANCE[role], f"artifacts.{role}.provenance is invalid")
    length = descriptor["length"]
    require(isinstance(length, int) and not isinstance(length, bool) and 0 < length <= MAX_ARTIFACT_BYTES, f"artifacts.{role}.length is invalid")
    if role == "microcode_text":
        require(length == 4096, "artifacts.microcode_text.length must be the exact 4 KiB IMEM image")
    expected_hash = validate_sha256(descriptor["sha256"], f"artifacts.{role}.sha256")
    path = validate_local_regular_file(descriptor["path"], f"artifacts.{role}", root)
    actual_length = path.stat().st_size
    require(actual_length == length, f"artifacts.{role} length drift: expected {length}, observed {actual_length}")
    actual_hash = sha256_file(path)
    require(actual_hash == expected_hash, f"artifacts.{role} SHA-256 drift")
    return path


def validate_manifest(manifest: dict, manifest_path: Path, root: Path) -> tuple[dict, dict[str, Path]]:
    require(set(manifest) == ROOT_FIELDS, "manifest has unknown or missing root fields")
    require(manifest["schema"] == MANIFEST_SCHEMA, f"schema must be {MANIFEST_SCHEMA!r}")
    validate_local_regular_file(str(manifest_path), "manifest", root)
    purpose = manifest["purpose"]
    require(purpose in PURPOSES, f"purpose must be one of {sorted(PURPOSES)}")

    intent = manifest["intent"]
    require(isinstance(intent, dict) and set(intent) == INTENT_FIELDS, "intent fields are invalid")
    wire_family = nonempty(intent["wire_family"], "intent.wire_family")
    require(wire_family in WIRE_FAMILIES, f"unsupported wire family {wire_family!r}")
    scenario = nonempty(intent["report_scenario"], "intent.report_scenario")
    require(
        SCENARIO_RE.fullmatch(scenario) is not None
        and re.fullmatch(r"[0-9a-f]{64}", scenario) is None,
        "intent.report_scenario is invalid",
    )
    require(intent["recognition"] == "runtime_must_confirm_rt64_known_pair", "intent.recognition must preserve the runtime recognition gate")
    program_lane = nonempty(intent["program_evidence_lane"], "intent.program_evidence_lane")
    if program_lane == "typed_function":
        raise AdmissionError(
            "intent.program_evidence_lane='typed_function' is not release-admissible: "
            "the lane name does not assert the generated entry-observation schema; install "
            "that schema and select 'typed_observed_function'"
        )
    if program_lane == "unidentified_native":
        raise AdmissionError(
            "intent.program_evidence_lane='unidentified_native' is not release-admissible: "
            "bind the exact linked archive identity and select 'identified_native_archive'"
        )
    require(
        program_lane in PROGRAM_EVIDENCE_LANES,
        f"intent.program_evidence_lane must be one of {sorted(PROGRAM_EVIDENCE_LANES)}",
    )
    cases = set(unique_strings(intent["extended_gbi_cases"], "intent.extended_gbi_cases"))
    if purpose in {"extended_gbi", "combined"}:
        require(wire_family == "f3dex2_extended_gbi_v1", "Extended GBI requires f3dex2_extended_gbi_v1")
        require(cases == EXTENDED_CASES, f"Extended GBI case denominator drifted: missing={sorted(EXTENDED_CASES - cases)}, extra={sorted(cases - EXTENDED_CASES)}")
    else:
        require(not cases, "full_rom-only admission must not claim Extended GBI cases")

    release = manifest["release_matrix"]
    require(isinstance(release, dict) and set(release) == RELEASE_FIELDS, "release_matrix fields are invalid")
    require(release["platform"] in PLATFORMS, "release_matrix.platform is invalid")
    controllers = set(unique_strings(release["controllers"], "release_matrix.controllers"))
    require(bool(controllers) and controllers <= CONTROLLERS, "release_matrix.controllers is invalid")
    require(release["save"] in SAVES, "release_matrix.save is invalid")
    renderers = set(unique_strings(release["renderers"], "release_matrix.renderers"))
    require(bool(renderers) and renderers <= RENDERERS, "release_matrix.renderers is invalid")
    if "reference_lle_accuracy" in renderers:
        require(renderers == {"reference_lle_accuracy"}, "reference LLE must stand alone")
    else:
        require("rt64_lle_accuracy" in renderers, "RT64 renderer coverage requires rt64_lle_accuracy")
    if purpose in {"extended_gbi", "combined"}:
        require({"rt64_lle_accuracy", "rt64_post_vi_capture"} <= renderers, "Extended GBI requires RT64 LLE and post-VI capture coverage")
    require(release["repeat_bar"] == 10, "release_matrix.repeat_bar must be exactly 10")

    artifacts = manifest["artifacts"]
    require(isinstance(artifacts, dict) and set(artifacts) == ARTIFACT_FIELDS, "artifacts fields are invalid")
    admitted: dict[str, Path] = {}
    for required_role in ("microcode_text", "microcode_data"):
        require(artifacts[required_role] is not None, f"artifacts.{required_role} is required")
    for role in ARTIFACT_FIELDS:
        descriptor = artifacts[role]
        if descriptor is not None:
            admitted[role] = validate_artifact(role, descriptor, root)
    if purpose in {"full_rom", "combined"}:
        require({"rom", "recompiled"} <= set(admitted), f"{purpose} admission requires ROM and recompiled artifacts")
        require(
            program_lane in {
                "identified_native_archive", "typed_observed_function",
                "typed_block_program",
            },
            f"{purpose} admission requires an authoritative executable lane: "
            "'identified_native_archive', 'typed_observed_function', or "
            "'typed_block_program'",
        )
    else:
        require(
            program_lane == "no_program_fixture",
            "extended_gbi-only admission must select 'no_program_fixture'; executable full-ROM "
            "lane claims require purpose 'full_rom' or 'combined'",
        )

    readiness = {
        "schema": READINESS_SCHEMA,
        "status": "ready",
        "purpose": purpose,
        "wire_family": wire_family,
        "report_scenario": scenario,
        "program_evidence_lane": program_lane,
        "artifact_roles_admitted": sorted(admitted),
        "extended_gbi_fixture": "ready_for_runtime_recognition" if purpose in {"extended_gbi", "combined"} else "not_requested",
        "full_rom_inputs": "ready" if {"rom", "recompiled"} <= set(admitted) else "not_supplied",
        "release_matrix_policy": "ready_for_ten_run_evidence",
        "repeat_bar": 10,
        "required_extended_cases": sorted(cases),
        "platform": release["platform"],
        "controllers": sorted(controllers),
        "save": release["save"],
        "renderers": sorted(renderers),
    }
    return readiness, admitted


def validate_readiness(report: dict) -> None:
    require(set(report) == READINESS_FIELDS, "readiness report has unknown or missing fields")
    require(report["schema"] == READINESS_SCHEMA, f"readiness schema must be {READINESS_SCHEMA!r}")
    require(report["status"] == "ready", "readiness status must be ready")
    require(report["purpose"] in PURPOSES, "readiness purpose is invalid")
    require(report["wire_family"] in WIRE_FAMILIES, "readiness wire family is invalid")
    scenario = nonempty(report["report_scenario"], "readiness report_scenario")
    require(SCENARIO_RE.fullmatch(scenario) is not None and re.fullmatch(r"[0-9a-f]{64}", scenario) is None, "readiness report_scenario is invalid")
    roles = set(unique_strings(report["artifact_roles_admitted"], "readiness artifact_roles_admitted"))
    program_lane = nonempty(report["program_evidence_lane"], "readiness program_evidence_lane")
    require(program_lane in PROGRAM_EVIDENCE_LANES, "readiness program-evidence lane is invalid")
    require({"microcode_text", "microcode_data"} <= roles <= ARTIFACT_FIELDS, "readiness artifact roles are invalid")
    require(report["extended_gbi_fixture"] in {"ready_for_runtime_recognition", "not_requested"}, "readiness Extended GBI state is invalid")
    require(report["full_rom_inputs"] in {"ready", "not_supplied"}, "readiness full-ROM state is invalid")
    require(report["release_matrix_policy"] == "ready_for_ten_run_evidence", "readiness release-matrix state is invalid")
    require(report["repeat_bar"] == 10, "readiness repeat bar is invalid")
    require(report["platform"] in PLATFORMS, "readiness platform is invalid")
    controllers = set(unique_strings(report["controllers"], "readiness controllers"))
    require(bool(controllers) and controllers <= CONTROLLERS, "readiness controllers are invalid")
    require(report["save"] in SAVES, "readiness save is invalid")
    renderers = set(unique_strings(report["renderers"], "readiness renderers"))
    require(bool(renderers) and renderers <= RENDERERS, "readiness renderers are invalid")
    if "reference_lle_accuracy" in renderers:
        require(renderers == {"reference_lle_accuracy"}, "readiness reference LLE must stand alone")
    else:
        require("rt64_lle_accuracy" in renderers, "readiness RT64 policy lacks rt64_lle_accuracy")
    cases = set(unique_strings(report["required_extended_cases"], "readiness required_extended_cases"))
    require(cases in (set(), EXTENDED_CASES), "readiness Extended GBI denominator is invalid")
    if report["purpose"] in {"extended_gbi", "combined"}:
        require(report["extended_gbi_fixture"] == "ready_for_runtime_recognition" and cases == EXTENDED_CASES, "readiness Extended GBI state is inconsistent")
        require({"rt64_lle_accuracy", "rt64_post_vi_capture"} <= renderers, "readiness Extended GBI renderer policy is incomplete")
    else:
        require(report["extended_gbi_fixture"] == "not_requested" and not cases, "readiness full-ROM-only report claims Extended GBI")
    if report["purpose"] in {"full_rom", "combined"}:
        require(report["full_rom_inputs"] == "ready" and {"rom", "recompiled"} <= roles, "readiness full-ROM inputs are incomplete")
        require(
            program_lane in {
                "identified_native_archive", "typed_observed_function",
                "typed_block_program",
            },
            "readiness full-ROM program-evidence lane is not authoritative",
        )
    else:
        require(program_lane == "no_program_fixture", "readiness fixture program lane is inconsistent")
    forbidden = {"path", "sha256", "hash", "length", "bytes", "filename", "manifest"}
    for key in report:
        require(not any(token in key.lower() for token in forbidden), f"readiness field {key!r} leaks private identity")


def validate_output_path(path: Path, root: Path) -> None:
    require(path.is_absolute(), "--report must be absolute")
    require(".." not in path.parts, "--report must not contain '..'")
    reject_symlink_components(path, include_leaf=False)
    require(not path.exists() and not path.is_symlink(), f"refusing to overwrite readiness report {path}")
    require(git_excluded(path, root), "readiness report path is inside the repository and not gitignored")


def write_report(path: Path, report: dict, root: Path) -> None:
    validate_readiness(report)
    validate_output_path(path, root)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    require(not temporary.exists() and not temporary.is_symlink(), f"temporary report path already exists: {temporary}")
    temporary.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def descriptor(path: Path, provenance: str) -> dict:
    return {
        "path": str(path),
        "length": path.stat().st_size,
        "sha256": sha256_file(path),
        "provenance": provenance,
        "git_identity": "excluded",
    }


def synthetic_manifest(directory: Path) -> tuple[Path, dict]:
    text = directory / "synthetic-text.bin"
    data = directory / "synthetic-data.bin"
    text.write_bytes(bytes((index * 17 + 3) & 0xFF for index in range(4096)))
    data.write_bytes(bytes((index * 29 + 5) & 0xFF for index in range(256)))
    manifest = {
        "schema": MANIFEST_SCHEMA,
        "purpose": "extended_gbi",
        "intent": {
            "wire_family": "f3dex2_extended_gbi_v1",
            "report_scenario": "synthetic-private-admission-selftest",
            "recognition": "runtime_must_confirm_rt64_known_pair",
            "extended_gbi_cases": sorted(EXTENDED_CASES),
            "program_evidence_lane": "no_program_fixture",
        },
        "release_matrix": {
            "platform": "macos_arm64",
            "controllers": ["standard_controller"],
            "save": "no_cartridge_save",
            "renderers": ["rt64_lle_accuracy", "rt64_post_vi_capture"],
            "repeat_bar": 10,
        },
        "artifacts": {
            "microcode_text": descriptor(text, "user_owned_rom_derived"),
            "microcode_data": descriptor(data, "user_owned_rom_derived"),
            "rom": None,
            "recompiled": None,
        },
    }
    manifest_path = directory / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest_path, manifest


def expect_rejected(action, label: str) -> None:
    try:
        action()
    except AdmissionError:
        return
    raise AdmissionError(f"selftest: {label} was accepted")


def selftest(root: Path) -> None:
    base = Path("/private/tmp") if Path("/private/tmp").is_dir() else Path(tempfile.gettempdir()).resolve()
    with tempfile.TemporaryDirectory(prefix="fn64-private-admission-", dir=base) as raw_directory:
        directory = Path(raw_directory)
        manifest_path, manifest = synthetic_manifest(directory)
        readiness, _ = validate_manifest(manifest, manifest_path, root)
        validate_readiness(readiness)
        serialized = json.dumps(readiness, sort_keys=True)
        for private_value in (
            manifest["artifacts"]["microcode_text"]["path"],
            manifest["artifacts"]["microcode_text"]["sha256"],
            str(manifest["artifacts"]["microcode_text"]["length"]),
        ):
            require(private_value not in serialized, "selftest: readiness report leaked private identity")

        text_path = Path(manifest["artifacts"]["microcode_text"]["path"])
        original = text_path.read_bytes()
        text_path.write_bytes(original[:-1] + bytes([original[-1] ^ 0xFF]))
        expect_rejected(lambda: validate_manifest(manifest, manifest_path, root), "hash drift")
        text_path.write_bytes(original)

        wrong_length = json.loads(json.dumps(manifest))
        wrong_length["artifacts"]["microcode_data"]["length"] += 1
        expect_rejected(lambda: validate_manifest(wrong_length, manifest_path, root), "length drift")

        shrunk = json.loads(json.dumps(manifest))
        shrunk["intent"]["extended_gbi_cases"].pop()
        expect_rejected(lambda: validate_manifest(shrunk, manifest_path, root), "shrunk Extended GBI case denominator")

        combined = json.loads(json.dumps(manifest))
        combined["purpose"] = "combined"
        expect_rejected(lambda: validate_manifest(combined, manifest_path, root), "combined admission without ROM/recompiled artifacts")

        rom = directory / "synthetic-rom.bin"
        recompiled = directory / "synthetic-recompiled.bin"
        rom.write_bytes(bytes(range(64)))
        recompiled.write_bytes(bytes(reversed(range(64))))
        full_rom = json.loads(json.dumps(manifest))
        full_rom["purpose"] = "full_rom"
        full_rom["intent"]["wire_family"] = "full_rom_mixed"
        full_rom["intent"]["extended_gbi_cases"] = []
        full_rom["intent"]["program_evidence_lane"] = "typed_block_program"
        full_rom["release_matrix"]["renderers"] = ["reference_lle_accuracy"]
        full_rom["artifacts"]["rom"] = descriptor(rom, "user_owned_rom")
        full_rom["artifacts"]["recompiled"] = descriptor(
            recompiled, "user_generated_from_owned_rom"
        )
        full_readiness, _ = validate_manifest(full_rom, manifest_path, root)
        validate_readiness(full_readiness)

        observed_function = json.loads(json.dumps(full_rom))
        observed_function["intent"]["program_evidence_lane"] = "typed_observed_function"
        observed_function_readiness, _ = validate_manifest(
            observed_function, manifest_path, root
        )
        validate_readiness(observed_function_readiness)

        typed_function = json.loads(json.dumps(full_rom))
        typed_function["intent"]["program_evidence_lane"] = "typed_function"
        expect_rejected(
            lambda: validate_manifest(typed_function, manifest_path, root),
            "typed whole-function full-ROM lane",
        )

        unidentified_native = json.loads(json.dumps(full_rom))
        unidentified_native["intent"]["program_evidence_lane"] = "unidentified_native"
        expect_rejected(
            lambda: validate_manifest(unidentified_native, manifest_path, root),
            "unidentified native full-ROM lane",
        )

        no_program_full_rom = json.loads(json.dumps(full_rom))
        no_program_full_rom["intent"]["program_evidence_lane"] = "no_program_fixture"
        expect_rejected(
            lambda: validate_manifest(no_program_full_rom, manifest_path, root),
            "no-program full-ROM lane",
        )

        directory_descriptor = json.loads(json.dumps(manifest))
        directory_descriptor["artifacts"]["microcode_data"]["path"] = str(directory)
        expect_rejected(lambda: validate_manifest(directory_descriptor, manifest_path, root), "special/non-regular file")

        tracked = json.loads(json.dumps(manifest))
        readme = root / "README.md"
        tracked["artifacts"]["microcode_data"] = {
            "path": str(readme), "length": readme.stat().st_size,
            "sha256": sha256_file(readme), "provenance": "user_owned_rom_derived",
            "git_identity": "excluded",
        }
        expect_rejected(lambda: validate_manifest(tracked, manifest_path, root), "tracked repository input")

        link = directory / "text-link.bin"
        try:
            link.symlink_to(text_path)
        except OSError:
            pass
        else:
            symlinked = json.loads(json.dumps(manifest))
            symlinked["artifacts"]["microcode_text"]["path"] = str(link)
            expect_rejected(lambda: validate_manifest(symlinked, manifest_path, root), "symlink input")
            manifest_link = directory / "manifest-link.json"
            manifest_link.symlink_to(manifest_path)
            expect_rejected(lambda: validate_manifest(manifest, manifest_link, root), "symlink manifest")

        fifo = directory / "synthetic.fifo"
        try:
            os.mkfifo(fifo)
        except (AttributeError, OSError):
            pass
        else:
            fifo_manifest = json.loads(json.dumps(manifest))
            fifo_manifest["artifacts"]["microcode_data"]["path"] = str(fifo)
            expect_rejected(lambda: validate_manifest(fifo_manifest, manifest_path, root), "FIFO input")

        report_path = directory / "readiness.json"
        write_report(report_path, readiness, root)
        validate_readiness(load_json(report_path))
        expect_rejected(lambda: write_report(report_path, readiness, root), "readiness overwrite")


def main() -> int:
    parser = argparse.ArgumentParser()
    actions = parser.add_mutually_exclusive_group(required=True)
    actions.add_argument("--check", action="store_true", help="run the synthetic policy selftest")
    actions.add_argument("--selftest", action="store_true")
    actions.add_argument("--manifest", type=Path)
    actions.add_argument("--verify-readiness", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        if args.check or args.selftest:
            require(args.report is None, "--report requires --manifest")
            selftest(root)
            print("private-input-admission: selftest passed (synthetic non-game bytes only)")
            return 0
        if args.verify_readiness:
            require(args.report is None, "--report cannot accompany --verify-readiness")
            validate_readiness(load_json(args.verify_readiness))
            print(f"private-input-admission: valid content-free readiness report {args.verify_readiness}")
            return 0
        require(args.manifest is not None and args.report is not None, "--manifest requires --report")
        require(args.manifest.is_absolute(), "--manifest must be absolute")
        validate_local_regular_file(str(args.manifest), "manifest", root)
        readiness, _ = validate_manifest(load_json(args.manifest), args.manifest, root)
        write_report(args.report, readiness, root)
        print(f"private-input-admission: ready; content-free report written to {args.report}")
        return 0
    except (AdmissionError, OSError) as error:
        print(f"private-input-admission: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
