#!/usr/bin/env python3
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CLASSIFIER = ROOT / "classify-trace.py"


def run(rows):
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "trace.jsonl"
        path.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
        return subprocess.run([sys.executable, str(CLASSIFIER), str(path)], text=True,
                              capture_output=True, check=False)


frontier = run([{"event": "executed_pc", "pc": {"address": 0x80000450}}] * 64)
assert frontier.returncode == 2, frontier
assert json.loads(frontier.stdout)["classification"] == "device-progress-frontier"

diverse = run([{"event": "executed_pc", "pc": {"address": 0x80000400 + i * 4}}
               for i in range(64)])
assert diverse.returncode == 0, diverse
assert json.loads(diverse.stdout)["classification"] == "diverse-execution-observation"

print("classify-trace selftest: ok")
