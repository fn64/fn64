#!/usr/bin/env python3
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent
SCRIPT = ROOT / "join-review-computed-flows.py"


class JoinReviewComputedFlowsTest(unittest.TestCase):
    def test_joins_sites_and_targets_without_promoting_candidates(self):
        inventory = {
            "candidate_only": True,
            "functions": [
                {"entry": 0x80001000, "body_ranges": [{"start": 0x80001000, "end_exclusive": 0x80001020}],
                 "reachable_from_loader_entry": True},
                {"entry": 0x80002000, "body_ranges": [{"start": 0x80002000, "end_exclusive": 0x80002020}],
                 "reachable_from_loader_entry": False},
            ],
        }
        provider = "\n".join([
            json.dumps({"record": "header", "schema": "fn64.tool-adapter"}),
            json.dumps({"record": "claim", "sequence": 0, "provider_claim_id": "flow-0",
                        "claim": {"type": "computed_control_flow", "site": {"pc": 0x80001004},
                                  "via_call": False, "targets": [{"pc": 0x80002000}]}}),
        ]) + "\n"
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            inventory_path = directory / "inventory.json"
            provider_path = directory / "provider.jsonl"
            output_path = directory / "join.json"
            inventory_path.write_text(json.dumps(inventory))
            provider_path.write_text(provider)
            subprocess.run(["python3", str(SCRIPT), str(inventory_path), str(provider_path), str(output_path)], check=True)
            body = json.loads(output_path.read_text())["body"]
            self.assertTrue(body["candidate_only"])
            self.assertFalse(body["production_ingest_authority"])
            self.assertEqual(body["claims_with_unreachable_target"], 1)
            self.assertEqual(body["claims"][0]["site_function_entry"], 0x80001000)
            self.assertEqual(body["claims"][0]["targets"][0]["function_entry"], 0x80002000)


if __name__ == "__main__":
    unittest.main()
