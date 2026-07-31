#!/usr/bin/env python3
"""No-ROM regression tests for mechanism-opportunity-ranking.py."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).with_name("mechanism-opportunity-ranking.py")
SPEC = importlib.util.spec_from_file_location("ranking", SCRIPT)
assert SPEC and SPEC.loader
ranking = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ranking)

ROM = "1" * 64
ANSWER = "2" * 64
OTHER = "3" * 64
SUFFIX = "d5840c10f9b7c0e64238cdae49b3cd632a91719e3b0393ad7dfced1125203e49"


def observations(*, mappings: list[dict] | None = None, detectors: list[str] | None = None) -> dict:
    return {
        "mappings": mappings if mappings is not None else [
            {"rom_space": "Physical", "rom": 0x1000, "bank": "secret_bank", "vram": 0x80000000}
        ],
        "claims": [], "conclusion_states": [], "word_classes": [], "owners": [],
        "incoming_relations": [], "candidate_detectors": detectors or [],
    }


def row(ordinal: int, offset: int, status: str, *, reason: str | None = None,
        size: int = 16, mapping: list[dict] | None = None, detector: bool = False,
        name: str | None = None) -> dict:
    domain = "unknown"
    status_value = {"status": status}
    mechanism_reason = "candidate_matched" if status == "candidate_matched" else "marker"
    if status == "missed":
        assert reason
        status_value["primary_reason"] = reason
        mechanism_reason = reason
    obs = observations(
        mappings=(
            [{"rom_space": item["rom_space"], "rom": item.get("rom", 0x1000 + offset),
              "bank": item["bank"], "vram": item.get("vram", 0x80000000 + offset)} for item in mapping]
            if mapping is not None else
            [{"rom_space": "Physical", "rom": 0x1000 + offset, "bank": "secret_bank", "vram": 0x80000000 + offset}]
        ),
        detectors=["ProloguePattern"] if detector else [],
    )
    mechanism = f"{domain}:{mechanism_reason}"
    instance = ranking.observation_digest(obs)
    return {
        "function": {"raw_ordinal": ordinal, "section_raw_ordinal": 7, "name": name or f"secret_fn_{ordinal}",
                     "vram": 0x80000000 + offset, "size": size,
                     "kind": "zero_size_marker" if size == 0 else "function"},
        "execution_domain": domain, "raw_rom": 0x1000 + offset, "status": status_value,
        "observations": obs, "mechanism_cluster_key": mechanism,
        "instance_cluster_key": f"{mechanism}:{instance}",
    }


def candidate(rom: int, vram: int, *, combined: bool = False, status: str = "gap",
              detectors: list[str] | None = None) -> dict:
    detectors = detectors or ["ProloguePattern"]
    return {
        "identity": {"kind": "addressed", "entry": {"rom_space": "Physical", "rom": rom, "vram": vram}},
        "combined": combined, "detectors": detectors,
        "detector_sources": [{"detector": detector, "sources": []} for detector in detectors], "status": status,
    }


def refresh_instance(item: dict) -> None:
    item["instance_cluster_key"] = (
        f"{item['mechanism_cluster_key']}:{ranking.observation_digest(item['observations'])}"
    )


def totals(rows: list[dict]) -> dict:
    bodies = {(item["raw_rom"], item["function"]["vram"]) for item in rows if item["function"]["size"]}
    return {
        "raw_rows": len(rows), "nonzero_rows": sum(item["function"]["size"] != 0 for item in rows),
        "distinct_bodies": len(bodies), "alias_rows": sum(item["function"]["kind"] == "alias" for item in rows if item["function"]["size"]),
        "marker_rows": sum(item["function"]["size"] == 0 for item in rows),
        "candidate_matched_rows": sum(item["status"]["status"] == "candidate_matched" for item in rows),
        "missed_rows": sum(item["status"]["status"] == "missed" for item in rows),
    }


def candidate_totals(candidates: list[dict]) -> dict:
    result = {key: 0 for key in ranking.CANDIDATE_TOTAL_KEYS}
    result["denominator"] = len(candidates)
    for item in candidates:
        result[item["status"]] += 1
        result["gradable"] += item["status"] != "ungradable"
        result["combined"] += item["combined"]
        result["per_detector_only"] += not item["combined"]
    return result


def envelope(rows: list[dict], *, candidates: list[dict] | None = None,
             section_size: int = 0x1000, section_name: str = "secret_section",
             normalized_rom: str = ROM, answer: str = ANSWER) -> dict:
    candidates = candidates or []
    summary = totals(rows)
    report = {
        "schema_version": 1,
        "sections": [{"raw_ordinal": 7, "name": section_name, "execution_domain": "unknown",
                      "rom_start": 0x1000, "vram_start": 0x80000000, "size": section_size}],
        "rows": rows, "candidate_statuses": candidates,
        "candidate_totals": candidate_totals(candidates), "totals": summary,
        "per_domain": [{"execution_domain": "unknown", "totals": dict(summary)}],
    }
    report["canonical_sha256"] = hashlib.sha256(
        json.dumps(report, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    return {
        "schema_version": 2, "algorithm": ranking.ALGORITHM,
        "normalized_rom_sha256": normalized_rom, "cold_workspace_manifest_sha256": OTHER,
        "cold_candidate_identities_v3_sha256": "4" * 64, "answer_key_sha256": answer,
        "answer_key_execution_domain": "unknown", "report": report,
    }


def legacy_v1(value: dict) -> dict:
    value = json.loads(json.dumps(value))
    value["schema_version"] = 1
    value["algorithm"] = ranking.ALGORITHM_V1
    value["cold_candidate_identities_v2_sha256"] = value.pop(
        "cold_candidate_identities_v3_sha256"
    )
    return value


def encode(value: dict, *, pretty: bool = False) -> bytes:
    return (json.dumps(value, ensure_ascii=False, indent=2 if pretty else None,
                       separators=None if pretty else (",", ":")) + "\n").encode()


def reverse_objects(value):
    if isinstance(value, dict):
        return {key: reverse_objects(item) for key, item in reversed(list(value.items()))}
    if isinstance(value, list):
        return [reverse_objects(item) for item in value]
    return value


def write_report(root: Path, value: dict, name: str = "report.json") -> tuple[Path, str]:
    path = root / name
    data = encode(value, pretty=True)
    path.write_bytes(data)
    return path, hashlib.sha256(data).hexdigest()


def run_single(root: Path, value: dict, *, output_name: str = "ranking.json", top: int = 100) -> tuple[int, Path]:
    report, report_sha = write_report(root, value, f"report-{output_name}")
    output = root / output_name
    result = ranking.main(["single", "--report", str(report), "--expected-report-sha256", report_sha,
                           "--evidence-id", "oot", "--family", "zelda", "--output", str(output),
                           "--top", str(top)])
    return result, output


def run_ab(root: Path, baseline: dict, followup: dict, *, output_name: str = "ab.json") -> tuple[int, Path]:
    baseline_path, baseline_sha = write_report(root, baseline, f"baseline-{output_name}")
    followup_path, followup_sha = write_report(root, followup, f"followup-{output_name}")
    output = root / output_name
    result = ranking.main([
        "ab", "--baseline-report", str(baseline_path),
        "--expected-baseline-report-sha256", baseline_sha,
        "--followup-report", str(followup_path),
        "--expected-followup-report-sha256", followup_sha,
        "--output", str(output),
    ])
    return result, output


def set_row_status(item: dict, status: str, reason: str | None = None) -> None:
    item["status"] = {"status": status}
    mechanism_reason = "candidate_matched" if status == "candidate_matched" else "marker"
    if status == "missed":
        assert reason
        item["status"]["primary_reason"] = reason
        mechanism_reason = reason
    item["mechanism_cluster_key"] = f"{item['execution_domain']}:{mechanism_reason}"
    refresh_instance(item)


def test_determinism_permutation_stable_keys_and_redaction(root: Path) -> None:
    assert ranking.observation_digest(observations()) == SUFFIX
    original = envelope([row(9, 0, "missed", reason="no_relation", name="do_not_leak")])
    result_a, output_a = run_single(root, original, output_name="a.json")
    assert result_a == 0
    result_repeat, output_repeat = run_single(root, original, output_name="repeat.json")
    assert result_repeat == 0 and output_a.read_bytes() == output_repeat.read_bytes()
    permuted = reverse_objects(original)
    result_b, output_b = run_single(root, permuted, output_name="b.json")
    assert result_b == 0
    value_a, value_b = json.loads(output_a.read_bytes()), json.loads(output_b.read_bytes())
    assert value_a["local_opportunities"] == value_b["local_opportunities"]
    assert value_a["totals"] == value_b["totals"]
    payload = output_a.read_bytes()
    assert b"do_not_leak" not in payload and b"secret_section" not in payload and b"secret_bank" not in payload
    parsed = json.loads(payload)
    assert parsed["authority"] == "caller_attested_answer_derived_diagnostic"
    assert parsed["can_feed_current_or_evaluated_rom_discovery"] is False
    assert stat.S_IMODE(output_a.stat().st_mode) == 0o600
    body = dict(parsed); claimed = body.pop("canonical_sha256")
    assert hashlib.sha256(ranking.canonical(body)).hexdigest() == claimed
    forbidden_keys = {"name", "raw_rom", "vram", "rom", "rom_start", "vram_start", "section_raw_ordinal", "bank"}
    def keys(value):
        if isinstance(value, dict):
            return set(value).union(*(keys(item) for item in value.values()))
        if isinstance(value, list):
            return set().union(*(keys(item) for item in value)) if value else set()
        return set()
    assert not (keys(parsed) & forbidden_keys)

    renamed = envelope([row(9, 0, "missed", reason="no_relation", name="different")], section_name="different_section")
    result_c, output_c = run_single(root, renamed, output_name="c.json")
    assert result_c == 0
    assert json.loads(output_a.read_bytes())["local_opportunities"][0]["local_opportunity_key"] == json.loads(output_c.read_bytes())["local_opportunities"][0]["local_opportunity_key"]


def test_runs_adjacency_and_top_totals(root: Path) -> None:
    rows = [
        row(1, 0, "candidate_matched"), row(2, 16, "missed", reason="no_relation"),
        row(3, 32, "candidate_matched"), row(4, 48, "missed", reason="no_relation", size=32),
        row(5, 80, "candidate_matched"), row(6, 96, "missed", reason="proven_code_no_entry"),
    ]
    rows[-1]["observations"]["word_classes"] = ["ProvenCode"]
    refresh_instance(rows[-1])
    result, output = run_single(root, envelope(rows), output_name="runs.json", top=1)
    assert result == 0
    value = json.loads(output.read_bytes())
    assert value["totals"]["missed_runs"] == 3
    assert value["totals"]["bracketed_runs"] == 2
    assert value["totals"]["matched_predecessor_runs"] == 3
    assert value["totals"]["matched_successor_runs"] == 2
    assert value["totals"]["singleton_bracketed_bodies"] == 2
    assert value["totals"]["singleton_bracketed_bytes"] == 48
    assert value["totals"]["opportunity_clusters"] == 3
    assert value["totals"]["published_opportunity_clusters"] == 1
    assert value["totals"]["omitted_opportunity_clusters"] == 2
    assert value["totals"]["missed_bodies"] == 3

    broken = [row(1, 0, "candidate_matched"), row(2, 32, "missed", reason="no_relation"), row(3, 48, "candidate_matched")]
    result, output = run_single(root, envelope(broken), output_name="broken.json")
    assert result == 0
    broken_totals = json.loads(output.read_bytes())["totals"]
    assert broken_totals["bracketed_runs"] == 0
    assert broken_totals["matched_predecessor_runs"] == 0
    assert broken_totals["matched_successor_runs"] == 1


def test_buckets_mapping_and_candidate_approximation(root: Path) -> None:
    for value, expected in [(1, "1_15"), (15, "1_15"), (16, "16_31"), (31, "16_31"),
                            (32, "32_63"), (63, "32_63"), (64, "64_127"), (127, "64_127"),
                            (128, "128_255"), (255, "128_255"), (256, "256_511"),
                            (511, "256_511"), (512, "512_1023"), (1023, "512_1023"),
                            (1024, "1024_2047"), (2047, "1024_2047"),
                            (2048, "2048_4095"), (4095, "2048_4095"), (4096, "ge_4096")]:
        assert ranking.size_bucket(value) == expected
    section = {"rom_start": 0x1000, "size": 0x1000}
    probe = row(1, 0x100, "missed", reason="no_relation", size=32)
    mapping_delta = 0x80000000 - 0x1000
    for candidate_delta, expected in [(0, "exact"), (-1, "1_16"), (-16, "1_16"),
                                      (-17, "17_64"), (1, "interior_nonstart"), (31, "interior_nonstart"),
                                      (32, "17_64"), (64, "17_64"), (65, "65_256"),
                                      (256, "65_256"), (257, "gt_256")]:
        bucket, approximate = ranking.proximity(
            probe, section, {("Physical", mapping_delta): [probe["raw_rom"] + candidate_delta]}
        )
        assert (bucket, approximate) == (expected, True)
    assert ranking.proximity(probe, section, {("Virtual", mapping_delta): [probe["raw_rom"]]}) == ("unknown", False)
    base = row(1, 0, "missed", reason="exact_candidate_not_promoted", detector=True)
    exact_candidate = candidate(0x1000, 0x80000000, status="candidate_matched")
    result, output = run_single(root, envelope([base], candidates=[exact_candidate]), output_name="candidate.json")
    assert result == 0
    proximity = json.loads(output.read_bytes())["local_opportunities"][0]["candidate_proximity"]
    assert proximity == {"bucket": "exact", "addressed_approximation": True, "bank_authenticated": False}

    unmapped = row(1, 0, "missed", reason="no_mapping", mapping=[])
    result, output = run_single(root, envelope([unmapped]), output_name="unmapped.json")
    assert result == 0
    opportunity = json.loads(output.read_bytes())["local_opportunities"][0]
    assert opportunity["mapped_bank_class"] == "unmapped" and opportunity["candidate_proximity"]["bucket"] == "unknown"

    ambiguous_maps = [
        {"rom_space": "Physical", "bank": "a"}, {"rom_space": "Physical", "bank": "b"}
    ]
    ambiguous = row(1, 0, "missed", reason="ambiguous_mapping", mapping=ambiguous_maps)
    result, output = run_single(root, envelope([ambiguous]), output_name="ambiguous.json")
    assert result == 0 and json.loads(output.read_bytes())["local_opportunities"][0]["mapped_bank_class"] == "ambiguous"


def test_alias_marker_collapse_and_physical_runs(root: Path) -> None:
    first = row(1, 0, "missed", reason="no_relation")
    alias = row(2, 0, "missed", reason="no_relation"); alias["function"]["kind"] = "alias"
    marker = row(3, 16, "not_discoverable_marker", size=0)
    second = row(4, 16, "missed", reason="no_relation")
    result, output = run_single(root, envelope([first, alias, marker, second]), output_name="aliases.json")
    assert result == 0
    value = json.loads(output.read_bytes())
    assert value["totals"]["distinct_bodies"] == 2
    assert value["totals"]["missed_bodies"] == 2
    assert value["totals"]["missed_runs"] == 1


def test_strict_ab_summary_redaction_and_rejections(root: Path) -> None:
    baseline_rows = [
        row(1, 0, "missed", reason="no_relation", name="do_not_leak_ab"),
        row(2, 16, "candidate_matched", name="also_private"),
    ]
    followup_rows = json.loads(json.dumps(baseline_rows))
    set_row_status(followup_rows[0], "candidate_matched")
    set_row_status(followup_rows[1], "missed", "no_relation")
    baseline = envelope(baseline_rows, candidates=[
        candidate(0x1000, 0x80000000, status="gap", detectors=["PrivateDetector"]),
    ])
    followup = envelope(followup_rows, candidates=[
        candidate(0x1000, 0x80000000, status="candidate_matched", detectors=["PrivateDetector", "NewDetector"]),
        candidate(0x1010, 0x80000010, status="outside", detectors=["NewDetector"]),
    ])
    # Historical V1 reports remain admissible only as exact read-only inputs,
    # allowing a strict pre-migration baseline to be compared with V2 output.
    baseline = legacy_v1(baseline)
    result, output = run_ab(root, baseline, followup)
    assert result == 0
    value = json.loads(output.read_bytes())
    assert value["schema"] == ranking.AB_OUTPUT_SCHEMA
    assert value["schema_version"] == 2
    assert value["comparison_kind"] == "cross_schema_unprojected_total_delta"
    assert value["bindings"]["baseline_attribution_envelope"]["schema_version"] == 1
    assert value["bindings"]["followup_attribution_envelope"]["schema_version"] == 2
    assert value["baseline"]["totals"]["candidate_matched_rows"] == 1
    assert value["followup"]["totals"]["candidate_matched_rows"] == 1
    assert value["body_status_transitions"] == {
        "candidate_matched_to_candidate_matched": 0,
        "candidate_matched_to_missed": 1,
        "missed_to_candidate_matched": 1,
        "missed_to_missed": 0,
    }
    assert value["candidate_status_deltas"] == {
        "candidate_matched": 1, "ambiguous_answer_mapping": 0, "interior": 0,
        "gap": -1, "outside": 1, "ungradable": 0,
    }
    assert value["detectors"]["baseline_distinct"] == 1
    assert value["detectors"]["followup_distinct"] == 2
    assert value["detectors"]["distinct_added"] == 1
    assert value["detectors"]["followup_candidate_detector_additions"] == 2
    assert len(value["detectors"]["populations"]) == 2
    assert stat.S_IMODE(output.stat().st_mode) == 0o600
    payload = output.read_bytes()
    assert b"do_not_leak_ab" not in payload and b"also_private" not in payload
    assert b"PrivateDetector" not in payload and b"NewDetector" not in payload
    forbidden_keys = {"name", "raw_rom", "vram", "rom", "rom_start", "vram_start", "section_raw_ordinal", "bank", "path"}
    def keys(item):
        if isinstance(item, dict): return set(item).union(*(keys(value) for value in item.values()))
        if isinstance(item, list): return set().union(*(keys(value) for value in item)) if item else set()
        return set()
    assert not (keys(value) & forbidden_keys)
    canonical = dict(value); claimed = canonical.pop("canonical_sha256")
    assert hashlib.sha256(ranking.canonical(canonical)).hexdigest() == claimed

    wrong_answer = json.loads(json.dumps(followup)); wrong_answer["answer_key_sha256"] = OTHER
    result, _ = run_ab(root, baseline, wrong_answer, output_name="wrong-answer.json")
    assert result == 1
    wrong_denominator = json.loads(json.dumps(followup)); wrong_denominator["report"]["rows"][0]["function"]["name"] = "different"
    wrong_denominator["report"]["canonical_sha256"] = hashlib.sha256(json.dumps(
        {key: wrong_denominator["report"][key] for key in ["schema_version", "sections", "rows", "candidate_statuses", "candidate_totals", "totals", "per_domain"]},
        ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()
    result, _ = run_ab(root, baseline, wrong_denominator, output_name="wrong-denominator.json")
    assert result == 1


def test_rejections_and_caps(root: Path) -> None:
    value = envelope([row(1, 0, "missed", reason="no_relation")])
    report, report_sha = write_report(root, value, "reject-report.json")
    assert ranking.main(["single", "--report", str(report), "--expected-report-sha256", "0" * 64,
                         "--evidence-id", "oot", "--family", "zelda", "--output", str(root / "bad-hash.json")]) == 1
    unknown = json.loads(json.dumps(value)); unknown["surprise"] = 1
    result, _ = run_single(root, unknown, output_name="unknown.json")
    assert result == 1
    schema = json.loads(json.dumps(value)); schema["algorithm"] = "wrong"
    result, _ = run_single(root, schema, output_name="schema.json")
    assert result == 1
    tampered = json.loads(json.dumps(value)); tampered["report"]["rows"][0]["function"]["size"] = 8
    result, _ = run_single(root, tampered, output_name="tampered.json")
    assert result == 1
    existing = root / "existing.json"; existing.write_text("x")
    assert ranking.main(["single", "--report", str(report), "--expected-report-sha256", report_sha,
                         "--evidence-id", "oot", "--family", "zelda", "--output", str(existing)]) == 1
    assert ranking.main(["single", "--report", "relative", "--expected-report-sha256", report_sha,
                         "--evidence-id", "oot", "--family", "zelda", "--output", str(root / "relative.json")]) == 1
    assert ranking.main(["single", "--report", str(report), "--expected-report-sha256", report_sha,
                         "--evidence-id", "oot", "--family", "zelda", "--output", str(root / "top.json"), "--top", "1001"]) == 1

    repo_output = SCRIPT.parent / "forbidden-ranking-output.json"
    assert not repo_output.exists()
    assert ranking.main(["single", "--report", str(report), "--expected-report-sha256", report_sha,
                         "--evidence-id", "oot", "--family", "zelda", "--output", str(repo_output)]) == 1

    prior_rows, prior_clusters, prior_report, prior_output = ranking.ROW_MAX, ranking.CLUSTER_MAX, ranking.REPORT_MAX, ranking.OUTPUT_MAX
    try:
        ranking.ROW_MAX = 0
        result, _ = run_single(root, value, output_name="row-cap.json")
        assert result == 1
        ranking.ROW_MAX = prior_rows; ranking.CLUSTER_MAX = 0
        result, _ = run_single(root, value, output_name="cluster-cap.json")
        assert result == 1
        ranking.CLUSTER_MAX = prior_clusters; ranking.REPORT_MAX = 1
        result, _ = run_single(root, value, output_name="report-cap.json")
        assert result == 1
        ranking.REPORT_MAX = prior_report; ranking.OUTPUT_MAX = 32
        result, _ = run_single(root, value, output_name="output-cap.json")
        assert result == 1
    finally:
        ranking.ROW_MAX, ranking.CLUSTER_MAX, ranking.REPORT_MAX, ranking.OUTPUT_MAX = prior_rows, prior_clusters, prior_report, prior_output


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="fn64-opportunity-ranking.", dir="/private/tmp") as temporary:
        root = Path(temporary).resolve()
        test_determinism_permutation_stable_keys_and_redaction(root)
        test_runs_adjacency_and_top_totals(root)
        test_buckets_mapping_and_candidate_approximation(root)
        test_alias_marker_collapse_and_physical_runs(root)
        test_strict_ab_summary_redaction_and_rejections(root)
        test_rejections_and_caps(root)
    print("mechanism opportunity ranking selftest: PASS")


if __name__ == "__main__":
    main()
