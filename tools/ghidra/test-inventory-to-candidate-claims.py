import json
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("inventory-to-candidate-claims.py")
ZERO = "0" * 64


class InventoryClaimsTest(unittest.TestCase):
    def test_emits_sorted_candidate_only_entry_and_extent_claims(self):
        inventory = {"candidate_only": True, "functions": [
            {"block": ".ram", "entry": 0x80000200, "body_envelope_end_exclusive": 0x80000220},
            {"block": ".boot", "entry": 0xA4000040, "body_envelope_end_exclusive": 0xA4000050},
        ]}
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source = directory / "inventory.json"
            output = directory / "claims.jsonl"
            source.write_text(json.dumps(inventory))
            subprocess.run([
                "python3", str(SCRIPT), str(source), str(output), "boot",
                "0x80000000", "0x80100000", ZERO, ZERO, ZERO,
                "a" * 40, ZERO, ZERO, ZERO,
            ], check=True)
            records = [json.loads(line) for line in output.read_text().splitlines()]
            self.assertEqual(records[0]["role"], "function_boundary_candidates")
            self.assertEqual(records[-1]["claim_records"], 2)
            self.assertEqual(records[1]["claim"]["type"], "function_entry")
            self.assertEqual(records[2]["claim"]["type"], "function_extent")


if __name__ == "__main__":
    unittest.main()
