#!/usr/bin/env python3
"""Build one bounded, answer-derived mechanism-opportunity ranking."""

from __future__ import annotations

import argparse
import bisect
import hashlib
import json
import os
import re
import stat
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any, NoReturn

REPORT_MAX = 64 * 1024 * 1024
OUTPUT_MAX = 16 * 1024 * 1024
ROW_MAX = 2_000_000
CLUSTER_MAX = 100_000
TOP_MAX = 1_000
ALGORITHM_V1 = "fn64.known-function-attribution.v1"
ALGORITHM_V2 = "fn64.known-function-attribution.v2"
# Current producers emit V2. V1 remains an exact, read-only input so strict
# A/B can compare a pre-migration baseline with a V2 follow-up.
ALGORITHM = ALGORITHM_V2
OUTPUT_SCHEMA = "fn64.mechanism-opportunity-ranking.v1"
AB_OUTPUT_SCHEMA = "fn64.known-function-attribution-ab.v2"
HEX = re.compile(r"[0-9a-f]{64}\Z")
TOKEN = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
U32_MAX = (1 << 32) - 1


class RankingError(ValueError):
    pass


def fail(message: str) -> NoReturn:
    raise RankingError(message)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: Any) -> bytes:
    return (json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


def exact(value: Any, keys: list[str], where: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{where}: expected object")
    actual = set(value)
    if actual != set(keys) or len(value) != len(keys):
        fail(f"{where}: fields differ: expected={keys} actual={sorted(actual)}")
    ordered = [(key, value[key]) for key in keys]
    value.clear()
    value.update(ordered)
    return value


def array(value: Any, where: str) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{where}: expected array")
    return value


def text(value: Any, where: str, *, token: bool = False) -> str:
    if not isinstance(value, str) or not value or not value.isascii():
        fail(f"{where}: expected nonempty ASCII string")
    if token and not TOKEN.fullmatch(value):
        fail(f"{where}: invalid token")
    return value


def digest(value: Any, where: str) -> str:
    value = text(value, where)
    if not HEX.fullmatch(value):
        fail(f"{where}: expected lowercase SHA-256")
    return value


def uint(value: Any, where: str, maximum: int = U32_MAX) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0 or value > maximum:
        fail(f"{where}: expected unsigned integer <= {maximum}")
    return value


def boolean(value: Any, where: str) -> bool:
    if not isinstance(value, bool):
        fail(f"{where}: expected boolean")
    return value


def stable_regular(path_text: str, expected_sha: str) -> tuple[Path, bytes]:
    path = Path(path_text)
    if not path.is_absolute() or path.resolve() != path:
        fail("report must be a canonical absolute path")
    try:
        before = path.stat()
    except OSError as error:
        fail(f"reading report metadata: {error}")
    if not stat.S_ISREG(before.st_mode) or path.is_symlink() or before.st_size > REPORT_MAX:
        fail("report must be a regular non-symlink file within the 64 MiB bound")
    try:
        data = path.read_bytes()
    except OSError as error:
        fail(f"reading report: {error}")
    after = path.stat()
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
    ):
        fail("report changed while read")
    if sha(data) != expected_sha:
        fail("report SHA-256 mismatch")
    return path, data


def no_duplicate_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"JSON contains duplicate field {key!r}")
        result[key] = value
    return result


def validate_section(value: Any, index: int) -> dict[str, Any]:
    row = exact(value, ["raw_ordinal", "name", "execution_domain", "rom_start", "vram_start", "size"], f"section[{index}]")
    uint(row["raw_ordinal"], f"section[{index}] ordinal", (1 << 64) - 1)
    text(row["name"], f"section[{index}] name")
    text(row["execution_domain"], f"section[{index}] domain")
    for key in ("rom_start", "vram_start", "size"):
        uint(row[key], f"section[{index}] {key}")
    if row["rom_start"] + row["size"] > U32_MAX + 1 or row["vram_start"] + row["size"] > U32_MAX + 1:
        fail(f"section[{index}]: extent overflow")
    return row


def validate_status(value: Any, where: str) -> tuple[str, str | None]:
    if not isinstance(value, dict) or not value:
        fail(f"{where}: malformed status")
    status = value.get("status")
    if status == "missed":
        exact(value, ["status", "primary_reason"], where)
        reason = text(value["primary_reason"], f"{where} reason")
        if reason not in {"no_mapping", "ambiguous_mapping", "exact_candidate_not_promoted", "proven_code_no_entry", "candidate_code_no_entry", "mapped_unreached", "no_relation"}:
            fail(f"{where}: unsupported miss reason")
        return status, reason
    exact(value, ["status"], where)
    if status not in ("candidate_matched", "not_discoverable_marker"):
        fail(f"{where}: unsupported status")
    return status, None


def validate_addressed(value: Any, where: str) -> dict[str, Any]:
    value = exact(value, ["rom_space", "rom", "vram"], where)
    if text(value["rom_space"], f"{where} rom_space") not in ("Physical", "Virtual"):
        fail(f"{where}: invalid ROM space")
    uint(value["rom"], f"{where} rom")
    uint(value["vram"], f"{where} vram")
    return value


def validate_observations(value: Any, where: str) -> dict[str, Any]:
    value = exact(value, ["mappings", "claims", "conclusion_states", "word_classes", "owners", "incoming_relations", "candidate_detectors"], where)
    mappings = array(value["mappings"], f"{where}.mappings")
    for index, mapping in enumerate(mappings):
        mapping = exact(mapping, ["rom_space", "rom", "bank", "vram"], f"{where}.mappings[{index}]")
        if text(mapping["rom_space"], "mapping rom_space") not in ("Physical", "Virtual"):
            fail(f"{where}: invalid mapping ROM space")
        uint(mapping["rom"], "mapping rom"); uint(mapping["vram"], "mapping vram"); text(mapping["bank"], "mapping bank")
    for index, claim in enumerate(array(value["claims"], f"{where}.claims")):
        claim = exact(claim, ["detector", "proposed_states"], f"{where}.claims[{index}]")
        text(claim["detector"], "claim detector")
        for state in array(claim["proposed_states"], "claim states"): text(state, "claim state")
    for key in ("conclusion_states", "word_classes", "incoming_relations", "candidate_detectors"):
        for item in array(value[key], f"{where}.{key}"): text(item, f"{where}.{key} item")
    for index, owner in enumerate(array(value["owners"], f"{where}.owners")):
        owner = exact(owner, ["state", "blocker_kinds"], f"{where}.owners[{index}]")
        text(owner["state"], "owner state")
        for blocker in array(owner["blocker_kinds"], "owner blockers"): text(blocker, "owner blocker")
    return value


def observation_digest(observations: dict[str, Any]) -> str:
    categorical = [
        [mapping["rom_space"] for mapping in observations["mappings"]],
        observations["claims"], observations["conclusion_states"], observations["word_classes"],
        observations["owners"], observations["incoming_relations"], observations["candidate_detectors"],
    ]
    return sha(json.dumps(categorical, ensure_ascii=False, separators=(",", ":")).encode())


def validate_candidate(value: Any, index: int) -> dict[str, Any]:
    value = exact(value, ["identity", "combined", "detectors", "detector_sources", "status"], f"candidate[{index}]")
    identity = value["identity"]
    if not isinstance(identity, dict) or identity.get("kind") not in ("addressed", "ungradable"):
        fail(f"candidate[{index}]: invalid identity")
    if identity["kind"] == "addressed":
        exact(identity, ["kind", "entry"], f"candidate[{index}].identity")
        validate_addressed(identity["entry"], f"candidate[{index}].entry")
    else:
        exact(identity, ["kind", "address"], f"candidate[{index}].identity")
        address = exact(identity["address"], ["bank", "pc"], f"candidate[{index}].address")
        text(address["bank"], "candidate bank"); uint(address["pc"], "candidate pc")
    boolean(value["combined"], "candidate combined")
    for detector in array(value["detectors"], "candidate detectors"): text(detector, "candidate detector")
    for source_index, source in enumerate(array(value["detector_sources"], "candidate detector_sources")):
        source = exact(source, ["detector", "sources"], f"candidate[{index}].source[{source_index}]")
        text(source["detector"], "source detector")
        for entry_index, entry in enumerate(array(source["sources"], "candidate sources")):
            validate_addressed(entry, f"candidate[{index}].source[{source_index}][{entry_index}]")
    text(value["status"], "candidate status")
    return value


TOTAL_KEYS = ["raw_rows", "nonzero_rows", "distinct_bodies", "alias_rows", "marker_rows", "candidate_matched_rows", "missed_rows"]
CANDIDATE_TOTAL_KEYS = ["denominator", "gradable", "ungradable", "combined", "per_detector_only", "candidate_matched", "ambiguous_answer_mapping", "interior", "gap", "outside"]


def validate_totals(value: Any, keys: list[str], where: str) -> dict[str, Any]:
    value = exact(value, keys, where)
    for key in keys: uint(value[key], f"{where}.{key}", (1 << 64) - 1)
    return value


def load_report(data: bytes) -> dict[str, Any]:
    try:
        envelope = json.loads(data, object_pairs_hook=no_duplicate_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"invalid report JSON: {error}")
    if not isinstance(envelope, dict):
        fail("envelope: expected object")
    schema_version = uint(envelope.get("schema_version"), "envelope schema_version")
    algorithm = text(envelope.get("algorithm"), "algorithm")
    if (schema_version, algorithm) == (1, ALGORITHM_V1):
        candidate_binding = "cold_candidate_identities_v2_sha256"
    elif (schema_version, algorithm) == (2, ALGORITHM_V2):
        candidate_binding = "cold_candidate_identities_v3_sha256"
    else:
        fail("unsupported attribution envelope schema/algorithm")
    envelope = exact(envelope, ["schema_version", "algorithm", "normalized_rom_sha256", "cold_workspace_manifest_sha256", candidate_binding, "answer_key_sha256", "answer_key_execution_domain", "report"], "envelope")
    for key in ("normalized_rom_sha256", "cold_workspace_manifest_sha256", candidate_binding, "answer_key_sha256"):
        digest(envelope[key], key)
    if text(envelope["answer_key_execution_domain"], "answer-key execution domain") not in ("vr4300", "rsp", "cic", "unknown"):
        fail("unsupported answer-key execution domain")
    report = exact(envelope["report"], ["schema_version", "sections", "rows", "candidate_statuses", "candidate_totals", "totals", "per_domain", "canonical_sha256"], "report")
    if uint(report["schema_version"], "report schema_version") != 1:
        fail("unsupported inner report schema")
    sections = [validate_section(value, index) for index, value in enumerate(array(report["sections"], "sections"))]
    ordinals = [section["raw_ordinal"] for section in sections]
    if ordinals != sorted(set(ordinals)):
        fail("sections are not canonically ordered and unique")
    section_by_id = {section["raw_ordinal"]: section for section in sections}
    rows = array(report["rows"], "rows")
    if len(rows) > ROW_MAX:
        fail("row limit exceeded")
    prior_ordinal = -1
    recomputed = {key: 0 for key in TOTAL_KEYS}
    distinct_bodies: set[tuple[int, int]] = set()
    per_domain_recomputed: dict[str, dict[str, int]] = defaultdict(lambda: {key: 0 for key in TOTAL_KEYS})
    for index, row in enumerate(rows):
        row = exact(row, ["function", "execution_domain", "raw_rom", "status", "observations", "mechanism_cluster_key", "instance_cluster_key"], f"row[{index}]")
        function = exact(row["function"], ["raw_ordinal", "section_raw_ordinal", "name", "vram", "size", "kind"], f"row[{index}].function")
        ordinal = uint(function["raw_ordinal"], "function ordinal", (1 << 64) - 1)
        if ordinal <= prior_ordinal: fail("rows are not canonically ordered by unique ordinal")
        prior_ordinal = ordinal
        section_id = uint(function["section_raw_ordinal"], "section ordinal", (1 << 64) - 1)
        if section_id not in section_by_id: fail("row refers to missing section")
        text(function["name"], "function name"); uint(function["vram"], "function vram"); uint(function["size"], "function size")
        if text(function["kind"], "function kind") not in ("function", "alias", "zero_size_marker"):
            fail("unsupported answer row kind")
        domain = text(row["execution_domain"], "row domain")
        if domain not in ("vr4300", "rsp", "cic", "unknown"): fail("unsupported row execution domain")
        if domain != section_by_id[section_id]["execution_domain"]: fail("row/section execution domain mismatch")
        raw_rom = uint(row["raw_rom"], "row raw_rom")
        offset = function["vram"] - section_by_id[section_id]["vram_start"]
        if offset < 0 or raw_rom != section_by_id[section_id]["rom_start"] + offset or offset + function["size"] > section_by_id[section_id]["size"]:
            fail("row geometry disagrees with section")
        status, reason = validate_status(row["status"], f"row[{index}].status")
        observations = validate_observations(row["observations"], f"row[{index}].observations")
        if (function["size"] == 0) != (function["kind"] == "zero_size_marker"):
            fail("zero-size marker kind/size relation is inconsistent")
        marker = function["size"] == 0
        expected_reason = None
        if not marker and status == "missed":
            if not observations["mappings"]: expected_reason = "no_mapping"
            elif len(observations["mappings"]) != 1: expected_reason = "ambiguous_mapping"
            elif observations["candidate_detectors"]: expected_reason = "exact_candidate_not_promoted"
            elif "ProvenCode" in observations["word_classes"]: expected_reason = "proven_code_no_entry"
            elif "CandidateCode" in observations["word_classes"]: expected_reason = "candidate_code_no_entry"
            elif observations["incoming_relations"]: expected_reason = "mapped_unreached"
            else: expected_reason = "no_relation"
            if reason != expected_reason: fail("row miss reason disagrees with observation precedence")
        if marker != (status == "not_discoverable_marker"):
            fail("marker/status relation is inconsistent")
        mechanism = text(row["mechanism_cluster_key"], "mechanism key")
        instance = text(row["instance_cluster_key"], "instance key")
        mechanism_reason = "candidate_matched" if status == "candidate_matched" else "marker" if status == "not_discoverable_marker" else reason
        if mechanism != f"{domain}:{mechanism_reason}" or instance != f"{mechanism}:{observation_digest(observations)}":
            fail("row cluster keys are inconsistent")
        for target in (recomputed, per_domain_recomputed[domain]):
            target["raw_rows"] += 1
            if marker: target["marker_rows"] += 1
            else:
                target["nonzero_rows"] += 1
                target["alias_rows"] += int(function["kind"] == "alias")
                target["candidate_matched_rows"] += int(status == "candidate_matched")
                target["missed_rows"] += int(status == "missed")
        if not marker: distinct_bodies.add((raw_rom, function["vram"]))
    candidates = array(report["candidate_statuses"], "candidate_statuses")
    for index, candidate in enumerate(candidates): validate_candidate(candidate, index)
    candidate_totals = validate_totals(report["candidate_totals"], CANDIDATE_TOTAL_KEYS, "candidate_totals")
    expected_candidates = {key: 0 for key in CANDIDATE_TOTAL_KEYS}
    expected_candidates["denominator"] = len(candidates)
    status_total_key = {
        "candidate_matched": "candidate_matched", "ambiguous_answer_mapping": "ambiguous_answer_mapping",
        "interior": "interior", "gap": "gap", "outside": "outside", "ungradable": "ungradable",
    }
    for candidate in candidates:
        status = candidate["status"]
        if status not in status_total_key: fail("candidate has unsupported accounting status")
        expected_candidates[status_total_key[status]] += 1
        expected_candidates["gradable"] += int(status != "ungradable")
        expected_candidates["combined"] += int(candidate["combined"])
        expected_candidates["per_detector_only"] += int(not candidate["combined"])
    if candidate_totals != expected_candidates: fail("candidate totals disagree with candidate rows")
    input_totals = validate_totals(report["totals"], TOTAL_KEYS, "totals")
    recomputed["distinct_bodies"] = len(distinct_bodies)
    if input_totals != recomputed: fail("report totals disagree with rows")
    seen_domains: list[str] = []
    for index, domain in enumerate(array(report["per_domain"], "per_domain")):
        domain = exact(domain, ["execution_domain", "totals"], f"per_domain[{index}]")
        domain_name = text(domain["execution_domain"], "per-domain domain")
        if domain_name not in per_domain_recomputed: fail("per-domain row has no input rows")
        seen_domains.append(domain_name)
        domain_totals = validate_totals(domain["totals"], TOTAL_KEYS, "per-domain totals")
        expected_domain = per_domain_recomputed[domain_name]
        expected_domain["distinct_bodies"] = len({(row["raw_rom"], row["function"]["vram"]) for row in rows if row["execution_domain"] == domain_name and row["function"]["size"] != 0 and row["function"]["kind"] != "zero_size_marker"})
        if domain_totals != expected_domain: fail("per-domain totals disagree with rows")
    if seen_domains != sorted(per_domain_recomputed): fail("per-domain rows are incomplete or noncanonical")
    claimed = digest(report["canonical_sha256"], "inner canonical digest")
    canonical_body = {key: report[key] for key in ["schema_version", "sections", "rows", "candidate_statuses", "candidate_totals", "totals", "per_domain"]}
    encoded = json.dumps(canonical_body, ensure_ascii=False, separators=(",", ":")).encode()
    if sha(encoded) != claimed:
        fail("inner canonical digest mismatch (ASCII serde-compatible boundary)")
    return envelope


def size_bucket(size: int) -> str:
    if size < 16: return "1_15"
    lower = 16
    while lower < 4096:
        upper = lower * 2 - 1
        if size <= upper: return f"{lower}_{upper}"
        lower *= 2
    return "ge_4096"


def count_bucket(count: int) -> str:
    if count == 0: return "zero"
    if count == 1: return "one"
    if count <= 4: return "two_four"
    if count <= 16: return "five_sixteen"
    if count <= 64: return "seventeen_sixtyfour"
    return "gt_64"


def density_bucket(function_bytes: int, section_size: int) -> str:
    if section_size == 0: return "unknown"
    basis = function_bytes * 100 // section_size
    if basis < 25: return "lt_25pct"
    if basis < 50: return "25_49pct"
    if basis < 75: return "50_74pct"
    return "ge_75pct"


def prerequisites(reason: str, observations: dict[str, Any]) -> list[str]:
    base = {
        "no_mapping": ["prove_unique_mapping"],
        "ambiguous_mapping": ["disambiguate_mapping"],
        "exact_candidate_not_promoted": ["promote_exact_candidate"],
        "proven_code_no_entry": ["prove_function_entry"],
        "candidate_code_no_entry": ["prove_code", "prove_function_entry"],
        "mapped_unreached": ["prove_function_entry", "prove_reachable_relation"],
        "no_relation": ["discover_relation", "prove_function_entry"],
    }[reason]
    blockers = [f"resolve_owner_blocker:{blocker}" for owner in observations["owners"] for blocker in owner["blocker_kinds"]]
    return sorted(set([*base, *blockers]))


def opaque(*parts: Any) -> str:
    return sha(json.dumps(parts, ensure_ascii=False, separators=(",", ":")).encode())


def proximity(row: dict[str, Any], section: dict[str, Any], candidates: dict[tuple[str, int], list[int]]) -> tuple[str, bool]:
    mappings = row["observations"]["mappings"]
    if len(mappings) != 1:
        return "unknown", False
    rom_space = mappings[0]["rom_space"]
    mapping_delta = mappings[0]["vram"] - mappings[0]["rom"]
    starts = candidates.get((rom_space, mapping_delta), [])
    section_start = section["rom_start"]
    section_end = section_start + section["size"]
    left = bisect.bisect_left(starts, section_start)
    right = bisect.bisect_left(starts, section_end)
    if left == right: return "unknown", False
    start = row["raw_rom"]; end = start + row["function"]["size"]
    at = bisect.bisect_left(starts, start, left, right)
    choices = starts[max(left, at - 1):min(right, at + 2)]
    nearest = min(choices, key=lambda value: (abs(value - start), value))
    if nearest == start: return "exact", True
    if start < nearest < end: return "interior_nonstart", True
    distance = abs(nearest - start)
    if distance <= 16: return "1_16", True
    if distance <= 64: return "17_64", True
    if distance <= 256: return "65_256", True
    return "gt_256", True


PROXIMITY_ORDER = {name: index for index, name in enumerate(("exact", "interior_nonstart", "1_16", "17_64", "65_256", "gt_256", "unknown"))}


def build(envelope: dict[str, Any], report_sha: str, evidence_id: str, family: str, top: int) -> dict[str, Any]:
    report = envelope["report"]
    sections = {section["raw_ordinal"]: section for section in report["sections"]}
    section_rows: dict[int, list[dict[str, Any]]] = defaultdict(list)
    for row in report["rows"]:
        section_rows[row["function"]["section_raw_ordinal"]].append(row)
    section_classes: dict[int, dict[str, str]] = {}
    for section_id, section in sections.items():
        nonzero = [row for row in section_rows[section_id] if row["function"]["size"]]
        section_classes[section_id] = {
            "size_bucket": size_bucket(max(1, section["size"])),
            "function_count_bucket": count_bucket(len(nonzero)),
            "declared_function_density_bucket": density_bucket(
                sum(row["function"]["size"] for row in nonzero), section["size"]
            ),
        }
    bank_sections: dict[str, set[int]] = defaultdict(set)
    for row in report["rows"]:
        for mapping in row["observations"]["mappings"]:
            bank_sections[mapping["bank"]].add(row["function"]["section_raw_ordinal"])
    candidates: dict[tuple[str, int], list[int]] = defaultdict(list)
    for candidate in report["candidate_statuses"]:
        identity = candidate["identity"]
        if identity["kind"] == "addressed":
            entry = identity["entry"]
            candidates[(entry["rom_space"], entry["vram"] - entry["rom"])].append(entry["rom"])
    for values in candidates.values(): values.sort()

    bodies: dict[tuple[Any, ...], dict[str, Any]] = {}
    for row in report["rows"]:
        status, reason = validate_status(row["status"], "ranking row status")
        if row["function"]["size"] == 0: continue
        body_id = (row["execution_domain"], row["raw_rom"], row["function"]["vram"])
        if body_id in bodies:
            prior = bodies[body_id]["row"]
            if prior["function"]["size"] != row["function"]["size"] or prior["status"] != row["status"] or prior["instance_cluster_key"] != row["instance_cluster_key"]:
                fail("aliases disagree on body size/status/observation signature")
            continue
        bodies[body_id] = {"row": row, "status": status, "reason": reason}

    run_by_body: dict[tuple[Any, ...], tuple[str, bool, bool]] = {}
    run_totals = {"missed_runs": 0, "matched_predecessor_runs": 0, "matched_successor_runs": 0,
                  "bracketed_runs": 0, "singleton_bracketed_runs": 0,
                  "singleton_bracketed_bodies": 0, "singleton_bracketed_bytes": 0}
    for section_id in sorted(sections):
        ordered = [(key, item) for key, item in bodies.items() if item["row"]["function"]["section_raw_ordinal"] == section_id]
        ordered.sort(key=lambda pair: (pair[1]["row"]["function"]["vram"], pair[1]["row"]["function"]["raw_ordinal"]))
        index = 0
        while index < len(ordered):
            if ordered[index][1]["status"] != "missed": index += 1; continue
            end_index = index
            while end_index + 1 < len(ordered) and ordered[end_index + 1][1]["status"] == "missed":
                left = ordered[end_index][1]["row"]; right = ordered[end_index + 1][1]["row"]
                if left["function"]["vram"] + left["function"]["size"] != right["function"]["vram"] or left["raw_rom"] + left["function"]["size"] != right["raw_rom"]: break
                end_index += 1
            first = ordered[index][1]["row"]; last = ordered[end_index][1]["row"]
            pred = index > 0 and ordered[index - 1][1]["status"] == "candidate_matched" and ordered[index - 1][1]["row"]["function"]["vram"] + ordered[index - 1][1]["row"]["function"]["size"] == first["function"]["vram"] and ordered[index - 1][1]["row"]["raw_rom"] + ordered[index - 1][1]["row"]["function"]["size"] == first["raw_rom"]
            succ = end_index + 1 < len(ordered) and ordered[end_index + 1][1]["status"] == "candidate_matched" and last["function"]["vram"] + last["function"]["size"] == ordered[end_index + 1][1]["row"]["function"]["vram"] and last["raw_rom"] + last["function"]["size"] == ordered[end_index + 1][1]["row"]["raw_rom"]
            bracketed = pred and succ; singleton = bracketed and end_index == index
            run_id = opaque(envelope["normalized_rom_sha256"], section_id, first["function"]["raw_ordinal"], last["function"]["raw_ordinal"])
            run_totals["missed_runs"] += 1; run_totals["matched_predecessor_runs"] += int(pred)
            run_totals["matched_successor_runs"] += int(succ); run_totals["bracketed_runs"] += int(bracketed)
            run_totals["singleton_bracketed_runs"] += int(singleton)
            if singleton:
                run_totals["singleton_bracketed_bodies"] += 1; run_totals["singleton_bracketed_bytes"] += first["function"]["size"]
            for body_index in range(index, end_index + 1):
                run_by_body[ordered[body_index][0]] = (run_id, pred, succ, bracketed, singleton)
            index = end_index + 1

    aggregates: dict[str, dict[str, Any]] = {}
    missed_bytes = 0; missed_bodies = 0; matched_bodies = 0
    for body_id, item in bodies.items():
        row = item["row"]
        if item["status"] == "candidate_matched": matched_bodies += 1; continue
        if item["status"] != "missed": continue
        missed_bodies += 1; missed_bytes += row["function"]["size"]
        section_id = row["function"]["section_raw_ordinal"]
        section = sections[section_id]
        mappings = row["observations"]["mappings"]
        banks = sorted({mapping["bank"] for mapping in mappings})
        if not banks: bank_class = "unmapped"; bank_key = "none"
        elif len(banks) > 1: bank_class = "ambiguous"; bank_key = opaque(envelope["normalized_rom_sha256"], banks)
        else:
            bank_class = "unique_shared_across_sections" if len(bank_sections[banks[0]]) > 1 else "unique_section_local"
            bank_key = opaque(envelope["normalized_rom_sha256"], banks[0])
        prox, approximated = proximity(row, section, candidates)
        size = size_bucket(row["function"]["size"])
        section_key = opaque(envelope["normalized_rom_sha256"], envelope["answer_key_sha256"], section_id)
        prereqs = prerequisites(item["reason"], row["observations"])
        local_key = opaque(row["mechanism_cluster_key"], row["instance_cluster_key"], section_key,
                           section_classes[section_id], size, bank_class, bank_key, prox)
        if local_key not in aggregates and len(aggregates) >= CLUSTER_MAX: fail("cluster limit exceeded")
        aggregate = aggregates.setdefault(local_key, {
            "local_opportunity_key": local_key, "mechanism_cluster_key": row["mechanism_cluster_key"],
            "observation_signature": row["instance_cluster_key"], "execution_domain": row["execution_domain"],
            "section_local_key": section_key, "section_class": section_classes[section_id], "size_bucket": size,
            "mapped_bank_class": bank_class, "mapped_bank_local_key": bank_key,
            "candidate_proximity": {"bucket": prox, "addressed_approximation": approximated, "bank_authenticated": False},
            "observed_open_prerequisites": prereqs,
            "impact": {"bodies": 0, "declared_function_bytes": 0, "observed_bank_count": 0},
            "run_metrics": {"missed_runs_touched": set(), "matched_predecessor_runs_touched": set(),
                            "matched_successor_runs_touched": set(), "bracketed_runs_touched": set(),
                            "singleton_bracketed_bodies": 0, "singleton_bracketed_bytes": 0},
        })
        aggregate["impact"]["bodies"] += 1; aggregate["impact"]["declared_function_bytes"] += row["function"]["size"]
        aggregate["impact"]["observed_bank_count"] = max(aggregate["impact"]["observed_bank_count"], len(banks))
        run_id, pred, succ, bracketed, singleton = run_by_body[body_id]
        aggregate["run_metrics"]["missed_runs_touched"].add(run_id)
        if pred: aggregate["run_metrics"]["matched_predecessor_runs_touched"].add(run_id)
        if succ: aggregate["run_metrics"]["matched_successor_runs_touched"].add(run_id)
        if bracketed: aggregate["run_metrics"]["bracketed_runs_touched"].add(run_id)
        if singleton:
            aggregate["run_metrics"]["singleton_bracketed_bodies"] += 1; aggregate["run_metrics"]["singleton_bracketed_bytes"] += row["function"]["size"]
    ranked = list(aggregates.values())
    for item in ranked:
        for key in ("missed_runs_touched", "matched_predecessor_runs_touched",
                    "matched_successor_runs_touched", "bracketed_runs_touched"):
            item["run_metrics"][key] = len(item["run_metrics"][key])
    ranked.sort(key=lambda item: (-item["impact"]["bodies"], -item["impact"]["declared_function_bytes"], len(item["observed_open_prerequisites"]), PROXIMITY_ORDER[item["candidate_proximity"]["bucket"]], item["local_opportunity_key"]))
    for index, item in enumerate(ranked[:top], 1): item["rank"] = index
    output = {
        "schema": OUTPUT_SCHEMA, "schema_version": 1,
        "authority": "caller_attested_answer_derived_diagnostic",
        "eligible_use": "next_mechanism_training_only",
        "can_feed_current_or_evaluated_rom_discovery": False,
        "heldout_or_generalization_claim": False,
        "validation_boundary": "strict_ascii_structural_and_serde_compatible_inner_digest_without_cold_reconstruction",
        "ranking_algorithm": "bodies_desc_bytes_desc_prerequisite_count_asc_proximity_asc_key_asc",
        "bindings": {"evidence_id": evidence_id, "family": family, "normalized_rom_sha256": envelope["normalized_rom_sha256"], "answer_key_sha256": envelope["answer_key_sha256"], "attribution_report_sha256": report_sha},
        "limits": {"max_report_bytes": REPORT_MAX, "max_rows": ROW_MAX, "max_clusters": CLUSTER_MAX, "max_output_bytes": OUTPUT_MAX, "top": top},
        "totals": {"input_rows": len(report["rows"]), "distinct_bodies": len(bodies), "candidate_matched_bodies": matched_bodies, "missed_bodies": missed_bodies, "missed_declared_function_bytes": missed_bytes, "opportunity_clusters": len(ranked), "published_opportunity_clusters": min(top, len(ranked)), "omitted_opportunity_clusters": max(0, len(ranked) - top), **run_totals},
        "local_opportunities": ranked[:top],
    }
    output["canonical_sha256"] = sha(canonical(output))
    return output


def answer_denominator(envelope: dict[str, Any]) -> str:
    """Bind the complete answer-function denominator without publishing it."""
    report = envelope["report"]
    body = {
        "sections": report["sections"],
        "functions": [
            {"function": row["function"], "execution_domain": row["execution_domain"],
             "raw_rom": row["raw_rom"]}
            for row in report["rows"]
        ],
    }
    return sha(canonical(body))


def body_statuses(envelope: dict[str, Any]) -> dict[str, str]:
    """Return opaque body keys and statuses, rejecting conflicting aliases."""
    result: dict[str, str] = {}
    for row in envelope["report"]["rows"]:
        if row["function"]["size"] == 0:
            continue
        status, _ = validate_status(row["status"], "A/B row status")
        body_key = opaque(
            envelope["normalized_rom_sha256"], envelope["answer_key_sha256"],
            row["execution_domain"], row["raw_rom"], row["function"]["vram"],
            row["function"]["size"],
        )
        prior = result.setdefault(body_key, status)
        if prior != status:
            fail("aliases disagree on A/B body status")
    return result


def candidate_detector_sets(envelope: dict[str, Any]) -> dict[str, set[str]]:
    """Return opaque candidate identities mapped to detector names internally."""
    candidates = envelope["report"]["candidate_statuses"]
    if len(candidates) > ROW_MAX:
        fail("candidate limit exceeded")
    result: dict[str, set[str]] = {}
    for candidate in candidates:
        key = opaque(envelope["normalized_rom_sha256"], candidate["identity"])
        if key in result:
            fail("candidate identities are not unique for A/B comparison")
        result[key] = set(candidate["detectors"])
    return result


def signed_delta(baseline: dict[str, int], followup: dict[str, int], keys: list[str]) -> dict[str, int]:
    return {key: followup[key] - baseline[key] for key in keys}


def build_ab(baseline: dict[str, Any], baseline_sha: str,
             followup: dict[str, Any], followup_sha: str) -> dict[str, Any]:
    if baseline["normalized_rom_sha256"] != followup["normalized_rom_sha256"]:
        fail("A/B reports bind different normalized ROMs")
    if baseline["answer_key_sha256"] != followup["answer_key_sha256"]:
        fail("A/B reports bind different answer keys")
    if baseline["answer_key_execution_domain"] != followup["answer_key_execution_domain"]:
        fail("A/B reports bind different answer-key execution domains")
    denominator = answer_denominator(baseline)
    if denominator != answer_denominator(followup):
        fail("A/B reports have different answer denominators")

    baseline_bodies = body_statuses(baseline)
    followup_bodies = body_statuses(followup)
    if set(baseline_bodies) != set(followup_bodies):
        fail("A/B reports have different body denominators")
    transitions = {
        f"{before}_to_{after}": 0
        for before in ("candidate_matched", "missed")
        for after in ("candidate_matched", "missed")
    }
    for key in baseline_bodies:
        transitions[f"{baseline_bodies[key]}_to_{followup_bodies[key]}"] += 1

    baseline_candidates = candidate_detector_sets(baseline)
    followup_candidates = candidate_detector_sets(followup)
    detector_names = set().union(*baseline_candidates.values(), *followup_candidates.values())
    if len(detector_names) > CLUSTER_MAX:
        fail("detector limit exceeded")
    populations: list[dict[str, Any]] = []
    assignment_additions = 0
    for name in detector_names:
        baseline_population = sum(name in detectors for detectors in baseline_candidates.values())
        followup_population = sum(name in detectors for detectors in followup_candidates.values())
        additions = sum(
            name in detectors and name not in baseline_candidates.get(key, set())
            for key, detectors in followup_candidates.items()
        )
        assignment_additions += additions
        populations.append({
            "detector_local_key": opaque(baseline["normalized_rom_sha256"], baseline["answer_key_sha256"], name),
            "baseline_population": baseline_population,
            "followup_population": followup_population,
            "population_delta": followup_population - baseline_population,
            "followup_additions": additions,
        })
    populations.sort(key=lambda item: item["detector_local_key"])

    base_report = baseline["report"]
    follow_report = followup["report"]
    candidate_status_keys = [
        "candidate_matched", "ambiguous_answer_mapping", "interior", "gap", "outside", "ungradable",
    ]
    same_envelope = (
        baseline["schema_version"] == followup["schema_version"]
        and baseline["algorithm"] == followup["algorithm"]
    )
    baseline_candidate_binding = (
        "cold_candidate_identities_v2_sha256"
        if baseline["schema_version"] == 1
        else "cold_candidate_identities_v3_sha256"
    )
    followup_candidate_binding = (
        "cold_candidate_identities_v2_sha256"
        if followup["schema_version"] == 1
        else "cold_candidate_identities_v3_sha256"
    )
    value = {
        "schema": AB_OUTPUT_SCHEMA,
        "schema_version": 2,
        "authority": "caller_attested_answer_derived_ab_diagnostic",
        "eligible_use": "mechanism_comparison_and_next_mechanism_training_only",
        "can_feed_current_or_evaluated_rom_discovery": False,
        "comparison_kind": (
            "same_attribution_envelope_schema"
            if same_envelope
            else "cross_schema_unprojected_total_delta"
        ),
        "validation_boundary": "two_independently_strict_reports_plus_rom_answer_and_denominator_identity_without_cold_reconstruction_or_cross_schema_projection",
        "bindings": {
            "normalized_rom_sha256": baseline["normalized_rom_sha256"],
            "answer_key_sha256": baseline["answer_key_sha256"],
            "answer_denominator_sha256": denominator,
            "baseline_attribution_report_sha256": baseline_sha,
            "followup_attribution_report_sha256": followup_sha,
            "baseline_report_canonical_sha256": base_report["canonical_sha256"],
            "followup_report_canonical_sha256": follow_report["canonical_sha256"],
            "baseline_attribution_envelope": {
                "schema_version": baseline["schema_version"],
                "algorithm": baseline["algorithm"],
                "candidate_identity_sha256": baseline[baseline_candidate_binding],
            },
            "followup_attribution_envelope": {
                "schema_version": followup["schema_version"],
                "algorithm": followup["algorithm"],
                "candidate_identity_sha256": followup[followup_candidate_binding],
            },
        },
        "limits": {
            "max_report_bytes": REPORT_MAX, "max_rows": ROW_MAX,
            "max_detectors": CLUSTER_MAX, "max_output_bytes": OUTPUT_MAX,
        },
        "baseline": {"totals": base_report["totals"], "candidate_totals": base_report["candidate_totals"]},
        "followup": {"totals": follow_report["totals"], "candidate_totals": follow_report["candidate_totals"]},
        "deltas": {
            "totals": signed_delta(base_report["totals"], follow_report["totals"], TOTAL_KEYS),
            "candidate_totals": signed_delta(base_report["candidate_totals"], follow_report["candidate_totals"], CANDIDATE_TOTAL_KEYS),
        },
        "body_status_transitions": transitions,
        "candidate_status_deltas": signed_delta(base_report["candidate_totals"], follow_report["candidate_totals"], candidate_status_keys),
        "detectors": {
            "baseline_distinct": len(set().union(*baseline_candidates.values())),
            "followup_distinct": len(set().union(*followup_candidates.values())),
            "distinct_added": len(set().union(*followup_candidates.values()) - set().union(*baseline_candidates.values())),
            "followup_candidate_detector_additions": assignment_additions,
            "populations": populations,
        },
    }
    value["canonical_sha256"] = sha(canonical(value))
    return value


def outside_git(parent: Path) -> None:
    current = parent
    while True:
        if (current / ".git").exists(): fail("output must be outside a Git worktree")
        if current.parent == current: return
        current = current.parent


def publish(path_text: str, value: dict[str, Any]) -> None:
    path = Path(path_text)
    if not path.is_absolute() or path.exists() or path.is_symlink(): fail("output must be an absent absolute path")
    parent = path.parent
    if not parent.is_dir() or parent.resolve() != parent: fail("output parent must be an existing canonical directory")
    outside_git(parent)
    data = canonical(value)
    if len(data) > OUTPUT_MAX: fail("output byte limit exceeded")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    descriptor = os.open(path, flags, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data); stream.flush(); os.fsync(stream.fileno())
    except BaseException:
        try: path.unlink()
        except OSError: pass
        raise


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)
    single = commands.add_parser("single")
    single.add_argument("--report", required=True); single.add_argument("--expected-report-sha256", required=True)
    single.add_argument("--evidence-id", required=True); single.add_argument("--family", required=True)
    single.add_argument("--output", required=True); single.add_argument("--top", type=int, default=100)
    ab = commands.add_parser("ab")
    ab.add_argument("--baseline-report", required=True); ab.add_argument("--expected-baseline-report-sha256", required=True)
    ab.add_argument("--followup-report", required=True); ab.add_argument("--expected-followup-report-sha256", required=True)
    ab.add_argument("--output", required=True)
    return root


def main(argv: list[str] | None = None) -> int:
    try:
        args = parser().parse_args(argv)
        if args.command == "single":
            expected = digest(args.expected_report_sha256, "expected report digest")
            evidence_id = text(args.evidence_id, "evidence-id", token=True); family = text(args.family, "family", token=True)
            if args.top < 1 or args.top > TOP_MAX: fail("--top must be 1..1000")
            _, data = stable_regular(args.report, expected)
            value = build(load_report(data), expected, evidence_id, family, args.top)
        else:
            baseline_sha = digest(args.expected_baseline_report_sha256, "expected baseline report digest")
            followup_sha = digest(args.expected_followup_report_sha256, "expected followup report digest")
            _, baseline_data = stable_regular(args.baseline_report, baseline_sha)
            _, followup_data = stable_regular(args.followup_report, followup_sha)
            value = build_ab(load_report(baseline_data), baseline_sha, load_report(followup_data), followup_sha)
        publish(args.output, value)
    except RankingError as error:
        print(f"mechanism-opportunity-ranking: {error}", file=sys.stderr); return 1
    except OSError as error:
        print(f"mechanism-opportunity-ranking: operating-system error ({error.errno})", file=sys.stderr); return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
