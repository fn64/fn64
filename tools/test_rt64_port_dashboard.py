#!/usr/bin/env python3
"""Tests for tools/rt64_port_dashboard.py.

Covers malformed states, false-completion shapes, missing/recursive
dependencies, absolute/private paths, stale generated outputs, and the
loopback-only --serve bind policy.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import subprocess
import sys
import unittest
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
TOOL = ROOT / "tools" / "rt64_port_dashboard.py"


def load_module():
    spec = importlib.util.spec_from_file_location("rt64_port_dashboard", TOOL)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


dashboard = load_module()


def base_schema() -> dict:
    return json.loads(dashboard.SCHEMA_PATH.read_text(encoding="utf-8"))


def base_manifest() -> dict:
    return json.loads(dashboard.STATUS_PATH.read_text(encoding="utf-8"))


def minimal_manifest() -> dict:
    return {
        "schema": "fn64.rt64-port-status.v1",
        "program": {
            "goal": "test goal",
            "updated_utc": "2026-08-15T00:00:00Z",
            "program_state": "IN PROGRESS",
            "branch": "feat/x",
            "base_branch": "main",
        },
        "milestones": [
            {"id": "M0", "title": "M0 title", "state": "READY", "exit_headline": "exit"},
            {"id": "M1", "title": "M1 title", "state": "PLANNED", "exit_headline": "exit"},
        ],
        "tickets": [
            {
                "id": "T1",
                "milestone": "M0",
                "objective": "do a thing",
                "profile": "I",
                "model": "GPT-5.6 Sol",
                "effort": "high",
                "owner": "someone",
                "branch": "feat/x",
                "base_branch": "main",
                "writable_paths": ["tools/x.py"],
                "dependencies": [],
                "state": "READY",
                "findings": [],
                "verification_runs": [],
                "blocker": None,
                "next_action": "do the next thing",
                "started_utc": "2026-08-15T00:00:00Z",
                "updated_utc": "2026-08-15T00:00:00Z",
            }
        ],
    }


class RealManifestTests(unittest.TestCase):
    """The committed manifest must validate cleanly and match generated outputs."""

    def test_real_schema_and_manifest_validate(self) -> None:
        schema = base_schema()
        dashboard.validate_schema_shape(schema)
        manifest = base_manifest()
        dashboard.validate_manifest(schema, manifest)

    def test_generated_outputs_are_not_stale(self) -> None:
        result = subprocess.run(
            [sys.executable, str(TOOL), "--check"], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("clean", result.stdout)

    def test_no_private_or_absolute_paths_in_real_manifest(self) -> None:
        manifest = base_manifest()
        text = json.dumps(manifest)
        self.assertNotIn("/Users/", text)
        self.assertNotIn("/home/", text)


class MalformedStateTests(unittest.TestCase):
    def test_unknown_ticket_state_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["state"] = "DONE"
        with self.assertRaisesRegex(dashboard.DashboardError, "not in"):
            dashboard.validate_manifest(schema, manifest)

    def test_lowercase_state_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["state"] = "ready"
        with self.assertRaisesRegex(dashboard.DashboardError, "not in"):
            dashboard.validate_manifest(schema, manifest)

    def test_unknown_milestone_state_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["milestones"][0]["state"] = "DONE"
        with self.assertRaisesRegex(dashboard.DashboardError, "not in"):
            dashboard.validate_manifest(schema, manifest)

    def test_ticket_referencing_unknown_milestone_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["milestone"] = "M99"
        with self.assertRaisesRegex(dashboard.DashboardError, "not a declared milestone"):
            dashboard.validate_manifest(schema, manifest)

    def test_bad_profile_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["profile"] = "X"
        with self.assertRaisesRegex(dashboard.DashboardError, "profile"):
            dashboard.validate_manifest(schema, manifest)

    def test_placeholder_model_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["model"] = "TBD"
        with self.assertRaisesRegex(dashboard.DashboardError, "placeholder"):
            dashboard.validate_manifest(schema, manifest)


class FalseCompletionTests(unittest.TestCase):
    def test_integrated_with_no_verification_runs_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["state"] = "INTEGRATED"
        manifest["tickets"][0]["verification_runs"] = []
        with self.assertRaisesRegex(dashboard.DashboardError, "false-completion"):
            dashboard.validate_manifest(schema, manifest)

    def test_integrated_with_verification_runs_accepted(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["state"] = "INTEGRATED"
        manifest["tickets"][0]["verification_runs"] = [
            {"command": "cargo test", "clean_run_count": 10, "required_run_count": 10, "kind": "deterministic"}
        ]
        dashboard.validate_manifest(schema, manifest)

    def test_ready_for_review_before_reliability_bar_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["state"] = "READY_FOR_REVIEW"
        manifest["tickets"][0]["verification_runs"] = [
            {"command": "cargo test", "clean_run_count": 9, "required_run_count": 10, "kind": "deterministic"}
        ]
        with self.assertRaisesRegex(dashboard.DashboardError, "before the declared reliability bar"):
            dashboard.validate_manifest(schema, manifest)

    def test_boolean_run_count_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["verification_runs"] = [
            {"command": "cargo test", "clean_run_count": True, "required_run_count": 10, "kind": "deterministic"}
        ]
        with self.assertRaisesRegex(dashboard.DashboardError, "non-negative int"):
            dashboard.validate_manifest(schema, manifest)

    def test_blocked_state_requires_nonempty_blocker(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["state"] = "BLOCKED"
        manifest["tickets"][0]["blocker"] = None
        with self.assertRaisesRegex(dashboard.DashboardError, "BLOCKED state requires"):
            dashboard.validate_manifest(schema, manifest)

    def test_non_blocked_state_forbids_blocker_text(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["state"] = "READY"
        manifest["tickets"][0]["blocker"] = "something is wrong"
        with self.assertRaisesRegex(dashboard.DashboardError, "must have blocker=null"):
            dashboard.validate_manifest(schema, manifest)

    def test_generator_never_upgrades_state_from_evidence(self) -> None:
        # A ticket with lots of clean verification runs but state READY must
        # stay READY: the generator/validator does not infer completion.
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["verification_runs"] = [
            {"command": "cargo test", "clean_run_count": 999, "required_run_count": 10, "kind": "deterministic"}
        ]
        _, _, tickets = dashboard.validate_manifest(schema, manifest)
        self.assertEqual(tickets[0]["state"], "READY")


class DependencyTests(unittest.TestCase):
    def test_unknown_dependency_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["dependencies"] = ["GHOST"]
        with self.assertRaisesRegex(dashboard.DashboardError, "unknown dependency"):
            dashboard.validate_manifest(schema, manifest)

    def test_self_dependency_cycle_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["dependencies"] = ["T1"]
        with self.assertRaisesRegex(dashboard.DashboardError, "cycle"):
            dashboard.validate_manifest(schema, manifest)

    def test_two_ticket_dependency_cycle_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        second = copy.deepcopy(manifest["tickets"][0])
        second["id"] = "T2"
        manifest["tickets"][0]["dependencies"] = ["T2"]
        second["dependencies"] = ["T1"]
        manifest["tickets"].append(second)
        with self.assertRaisesRegex(dashboard.DashboardError, "cycle"):
            dashboard.validate_manifest(schema, manifest)

    def test_acyclic_dependency_chain_accepted(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        second = copy.deepcopy(manifest["tickets"][0])
        second["id"] = "T2"
        second["dependencies"] = ["T1"]
        manifest["tickets"].append(second)
        dashboard.validate_manifest(schema, manifest)


class ExternalIssueTests(unittest.TestCase):
    def test_canonical_github_issue_accepted(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["external_issue"] = "https://github.com/fn64/fn64/issues/123"
        dashboard.validate_manifest(schema, manifest)

    def test_noncanonical_issue_reference_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["external_issue"] = "issue 123"
        with self.assertRaisesRegex(dashboard.DashboardError, "canonical GitHub issue URL"):
            dashboard.validate_manifest(schema, manifest)

    def test_duplicate_ticket_id_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        duplicate = copy.deepcopy(manifest["tickets"][0])
        manifest["tickets"].append(duplicate)
        with self.assertRaisesRegex(dashboard.DashboardError, "duplicate id"):
            dashboard.validate_manifest(schema, manifest)


class PathPrivacyTests(unittest.TestCase):
    def test_absolute_unix_path_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["writable_paths"] = ["/Users/someone/fn64/tools/x.py"]
        with self.assertRaisesRegex(dashboard.DashboardError, "private|absolute"):
            dashboard.validate_manifest(schema, manifest)

    def test_home_relative_path_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["writable_paths"] = ["~/fn64/tools/x.py"]
        with self.assertRaisesRegex(dashboard.DashboardError, "private|absolute"):
            dashboard.validate_manifest(schema, manifest)

    def test_windows_drive_path_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["writable_paths"] = ["C:\\\\Users\\\\someone\\\\fn64"]
        with self.assertRaisesRegex(dashboard.DashboardError, "private|absolute"):
            dashboard.validate_manifest(schema, manifest)

    def test_email_like_identity_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["writable_paths"] = ["tools/someone@example.com/x.py"]
        with self.assertRaisesRegex(dashboard.DashboardError, "private-identity"):
            dashboard.validate_manifest(schema, manifest)

    def test_relative_repo_path_accepted(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["writable_paths"] = ["crates/fn64-render-ir/src/lib.rs"]
        dashboard.validate_manifest(schema, manifest)

    def test_empty_writable_paths_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["writable_paths"] = []
        with self.assertRaisesRegex(dashboard.DashboardError, "at least one path"):
            dashboard.validate_manifest(schema, manifest)


class TimestampTests(unittest.TestCase):
    def test_bad_timestamp_format_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["started_utc"] = "2026/08/15"
        with self.assertRaisesRegex(dashboard.DashboardError, "ISO-8601"):
            dashboard.validate_manifest(schema, manifest)

    def test_bad_month_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["started_utc"] = "2026-13-01T00:00:00Z"
        with self.assertRaisesRegex(dashboard.DashboardError, "invalid UTC calendar"):
            dashboard.validate_manifest(schema, manifest)

    def test_nonexistent_leap_day_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["started_utc"] = "2026-02-29T00:00:00Z"
        with self.assertRaisesRegex(dashboard.DashboardError, "invalid UTC calendar"):
            dashboard.validate_manifest(schema, manifest)

    def test_updated_before_started_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["started_utc"] = "2026-08-15T12:00:00Z"
        manifest["tickets"][0]["updated_utc"] = "2026-08-15T00:00:00Z"
        with self.assertRaisesRegex(dashboard.DashboardError, "precedes started_utc"):
            dashboard.validate_manifest(schema, manifest)

    def test_missing_timezone_marker_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["started_utc"] = "2026-08-15T00:00:00"
        with self.assertRaisesRegex(dashboard.DashboardError, "ISO-8601"):
            dashboard.validate_manifest(schema, manifest)


class BoundsTests(unittest.TestCase):
    def test_oversized_text_field_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["objective"] = "x" * 10000
        with self.assertRaisesRegex(dashboard.DashboardError, "exceeds"):
            dashboard.validate_manifest(schema, manifest)

    def test_empty_string_field_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["objective"] = ""
        with self.assertRaisesRegex(dashboard.DashboardError, "non-empty"):
            dashboard.validate_manifest(schema, manifest)

    def test_oversized_findings_array_rejected(self) -> None:
        schema = base_schema()
        manifest = minimal_manifest()
        manifest["tickets"][0]["findings"] = [f"finding {i}" for i in range(1000)]
        with self.assertRaisesRegex(dashboard.DashboardError, "exceeds"):
            dashboard.validate_manifest(schema, manifest)


class StaleOutputTests(unittest.TestCase):
    def test_check_detects_stale_markdown(self) -> None:
        schema, program, milestones, tickets = dashboard.load_and_validate()
        original = dashboard.MARKDOWN_PATH.read_text(encoding="utf-8")
        try:
            dashboard.atomic_write(dashboard.MARKDOWN_PATH, original + "\nstale trailer\n")
            result = subprocess.run(
                [sys.executable, str(TOOL), "--check"], cwd=ROOT, capture_output=True, text=True
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("stale", result.stderr)
        finally:
            dashboard.atomic_write(dashboard.MARKDOWN_PATH, original)

    def test_check_detects_stale_html(self) -> None:
        original = dashboard.HTML_PATH.read_text(encoding="utf-8")
        try:
            dashboard.atomic_write(dashboard.HTML_PATH, original.replace("RT64", "RT65", 1))
            result = subprocess.run(
                [sys.executable, str(TOOL), "--check"], cwd=ROOT, capture_output=True, text=True
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("stale", result.stderr)
        finally:
            dashboard.atomic_write(dashboard.HTML_PATH, original)

    def test_check_detects_missing_markdown(self) -> None:
        original = dashboard.MARKDOWN_PATH.read_text(encoding="utf-8")
        try:
            dashboard.MARKDOWN_PATH.unlink()
            result = subprocess.run(
                [sys.executable, str(TOOL), "--check"], cwd=ROOT, capture_output=True, text=True
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing", result.stderr)
        finally:
            dashboard.atomic_write(dashboard.MARKDOWN_PATH, original)

    def test_write_then_check_is_clean(self) -> None:
        result_write = subprocess.run(
            [sys.executable, str(TOOL), "--write"], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual(result_write.returncode, 0, result_write.stderr)
        result_check = subprocess.run(
            [sys.executable, str(TOOL), "--check"], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual(result_check.returncode, 0, result_check.stderr)

    def test_atomic_write_leaves_no_tmp_file_on_success(self) -> None:
        target = dashboard.MARKDOWN_PATH
        before = {p.name for p in target.parent.glob(f".{target.name}.*.tmp")}
        dashboard.atomic_write(target, target.read_text(encoding="utf-8"))
        after = {p.name for p in target.parent.glob(f".{target.name}.*.tmp")}
        self.assertEqual(before, after)


class TerminalViewTests(unittest.TestCase):
    def test_elapsed_uses_recorded_endpoints(self) -> None:
        ticket = minimal_manifest()["tickets"][0]
        ticket["updated_utc"] = "2026-08-15T01:02:00Z"
        self.assertEqual(dashboard.elapsed_seconds(ticket), 3720.0)
        self.assertEqual(dashboard.format_elapsed(dashboard.elapsed_seconds(ticket)), "1h2m")

    def test_terminal_view_contains_goal_and_ticket_ids(self) -> None:
        schema, program, milestones, tickets = dashboard.load_and_validate()
        text = dashboard.render_terminal(program, milestones, tickets)
        self.assertIn(program["goal"][:20], text)
        for ticket in tickets:
            self.assertIn(ticket["id"], text)

    def test_default_invocation_prints_terminal_view(self) -> None:
        result = subprocess.run([sys.executable, str(TOOL)], cwd=ROOT, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("RT64 PORT DASHBOARD", result.stdout)


class ServeLoopbackTests(unittest.TestCase):
    def test_serve_rejects_non_loopback_host(self) -> None:
        with self.assertRaisesRegex(dashboard.DashboardError, "non-loopback"):
            dashboard.serve("0.0.0.0", 0)

    def test_serve_rejects_wildcard_host(self) -> None:
        with self.assertRaisesRegex(dashboard.DashboardError, "non-loopback"):
            dashboard.serve("*", 0)

    def test_serve_accepts_loopback_and_binds_ephemeral_port(self) -> None:
        import threading

        server_holder: dict = {}

        def run() -> None:
            httpd = dashboard._LoopbackHTTPServer(("127.0.0.1", 0), dashboard.http.server.SimpleHTTPRequestHandler)
            server_holder["server"] = httpd
            httpd.handle_request()

        thread = threading.Thread(target=run, daemon=True)
        thread.start()
        thread.join(timeout=5)
        self.assertIn("server", server_holder)
        host, port = server_holder["server"].server_address
        self.assertEqual(host, "127.0.0.1")
        self.assertGreater(port, 0)
        server_holder["server"].server_close()

    def test_serve_get_of_dashboard_html_succeeds_readonly(self) -> None:
        import threading

        httpd = dashboard._LoopbackHTTPServer(("127.0.0.1", 0), None)
        httpd.server_close()  # rebuild with the real handler bound to the docs directory
        directory = str(dashboard.HTML_PATH.parent)

        class Handler(dashboard.http.server.SimpleHTTPRequestHandler):
            def __init__(self, *args, **kwargs):
                super().__init__(*args, directory=directory, **kwargs)

            def log_message(self, format, *args):
                pass

        server = dashboard._LoopbackHTTPServer(("127.0.0.1", 0), Handler)
        port = server.server_address[1]
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/{dashboard.HTML_PATH.name}", timeout=5) as response:
                self.assertEqual(response.status, 200)
                body = response.read().decode("utf-8")
                self.assertIn("RT64 port workflow dashboard", body)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


class SelfTestProcessTests(unittest.TestCase):
    """Run the checker itself as ten deterministic separate processes."""

    def test_ten_consecutive_clean_check_processes(self) -> None:
        for _ in range(10):
            result = subprocess.run(
                [sys.executable, str(TOOL), "--check"], cwd=ROOT, capture_output=True, text=True
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("clean", result.stdout)


if __name__ == "__main__":
    unittest.main()
