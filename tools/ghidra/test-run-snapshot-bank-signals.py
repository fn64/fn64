#!/usr/bin/env python3

import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools/ghidra/run-snapshot-bank.sh"


def shell_quote(value: Path) -> str:
    return "'" + str(value).replace("'", "'\"'\"'") + "'"


def make_executable(path: Path, body: str) -> None:
    path.write_text(body)
    path.chmod(0o700)


@unittest.skipUnless(platform.system() == "Darwin", "memory guard requires macOS")
class SnapshotBankSignalTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-ghidra-signals-")
        self.root = Path(self.temporary.name).resolve()
        self.workspace = self.root / "workspace"
        self.workspace.mkdir(mode=0o700)
        self.snapshot = self.root / "snapshot.json"
        self.snapshot.write_text("{}\n")
        self.bank = self.root / "bank.bin"
        self.bank.write_bytes(b"\0\0\0\0")
        self.active = self.root / "active.pid"
        self.overlap = self.root / "overlap"
        self.quick = self.root / "quick"
        self.post_ingest = self.root / "post-ingest"

        self.ghidra = self.root / "ghidra"
        support = self.ghidra / "support"
        application = self.ghidra / "Ghidra"
        support.mkdir(parents=True)
        application.mkdir()
        (application / "application.properties").write_text(
            "application.version=synthetic\n"
        )
        active = shell_quote(self.active)
        overlap = shell_quote(self.overlap)
        quick = shell_quote(self.quick)
        make_executable(
            support / "analyzeHeadless",
            f"""#!/bin/sh
set -eu
active={active}
overlap={overlap}
quick={quick}
if [ -e "$quick" ]; then
    provider=
    for argument in "$@"; do
        case "$argument" in
            */provider.jsonl) provider=$argument ;;
        esac
    done
    [ -n "$provider" ]
    printf '{{}}\n' > "$provider"
    echo 'Using Loader: Raw Binary'
    echo 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32'
    exit 0
fi
[ ! -e "$active" ] || : > "$overlap"
cleanup() {{
    sleep 0.5
    rm -f -- "$active"
    exit 143
}}
trap cleanup HUP INT TERM
echo $$ > "$active"
while :; do sleep 1; done
""",
        )

        self.jdk = self.root / "jdk"
        (self.jdk / "bin").mkdir(parents=True)
        make_executable(self.jdk / "bin/java", "#!/bin/sh\nexit 0\n")

        self.stage = self.root / "stage"
        make_executable(
            self.stage,
            r'''#!/usr/bin/env python3
import hashlib
import json
import pathlib
import sys

if len(sys.argv) == 9 and sys.argv[1] == "--base-only":
    _, _, snapshot, bank, source, _workspace, output, evidence, base = sys.argv
    base_value = int(base, 0)
    schema_version = 2
    seeds = {"mode": "base_only", "base_seed": base_value}
elif len(sys.argv) == 8 and sys.argv[1] == "--discovery-only":
    _, _, snapshot, bank, source, _workspace, output, evidence = sys.argv
    base_value = 0x80000400
    schema_version = 3
    seeds = {"mode": "discovery_only", "role": "candidate_only"}
else:
    raise SystemExit("wrong fake-stage invocation")
data = pathlib.Path(source).read_bytes()
pathlib.Path(output).write_bytes(data)
digest = hashlib.sha256(data).hexdigest()
snapshot_digest = hashlib.sha256(pathlib.Path(snapshot).read_bytes()).hexdigest()
value = {
    "schema": "fn64.snapshot-bank-evidence",
    "schema_version": schema_version,
    "program_snapshot_sha256": snapshot_digest,
    "input": {
        "normalized_rom_sha256": "00" * 32,
        "bank": bank,
        "bank_bytes_sha256": digest,
        "mapping_sha256": "11" * 32,
        "va_start": base_value,
        "va_end": base_value + len(data),
    },
    "backing": {"rom_space": "Physical", "rom_start": 0, "rom_end": len(data)},
    "artifact": {"byte_length": len(data), "sha256": digest},
    "seeds": seeds,
}
pathlib.Path(evidence).write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
print(f"stage-snapshot-bank: snapshot={snapshot_digest}")
''',
        )
        self.ingest = self.root / "ingest"
        make_executable(
            self.ingest,
            r'''#!/usr/bin/env python3
import json
import pathlib
import sys

request_path = pathlib.Path(sys.argv[2])
output_path = pathlib.Path(sys.argv[4])
request = json.loads(request_path.read_text())
evidence = json.loads((request_path.parent / "raw/evidence.json").read_text())
value = {
    "schema": "fn64.tool-claim-set",
    "schema_version": 1,
    "program_snapshot_sha256": evidence["program_snapshot_sha256"],
    "sources": [{"tool": {"name": run["tool"]["name"]}} for run in request["runs"]],
    "claims": [{}],
}
output_path.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
print(f"ingest-tool-claims: snapshot={value['program_snapshot_sha256']}")
''',
        )
        wrapper = self.root / "bin"
        wrapper.mkdir()
        make_executable(
            wrapper / "python3",
            f"""#!/bin/sh
if [ "${{1:-}}" = - ]; then
    case "${{2:-}}" in
        */out/tool-claims.json)
            : > {shell_quote(self.post_ingest)}
            sleep 1
            ;;
    esac
fi
exec {shell_quote(Path(sys.executable))} "$@"
""",
        )
        self.path = f"{wrapper}:{os.environ.get('PATH', '/usr/bin:/bin')}"

    def tearDown(self):
        self.temporary.cleanup()

    def launch(self, *, discovery_only: bool = False) -> subprocess.Popen:
        environment = os.environ.copy()
        environment.update(
            {
                "FN64_STAGE_SNAPSHOT_BANK": str(self.stage),
                "FN64_INGEST_TOOL_CLAIMS": str(self.ingest),
                "GHIDRA_INSTALL_DIR": str(self.ghidra),
                "GHIDRA_JAVA_HOME": str(self.jdk),
                "PATH": self.path,
            }
        )
        arguments = [
            RUNNER,
            "--discovery-only" if discovery_only else "--unseeded-only",
            self.snapshot,
            "boot",
            self.bank,
            self.workspace,
        ]
        if not discovery_only:
            arguments.append("0x80000400")
        return subprocess.Popen(
            arguments,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=environment,
        )

    def wait_for_active(self, process: subprocess.Popen) -> int:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.active.exists():
                return int(self.active.read_text())
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                self.fail(
                    f"runner exited before analysis\nstdout:\n{stdout}\nstderr:\n{stderr}"
                )
            time.sleep(0.05)
        process.terminate()
        process.wait(timeout=10)
        self.fail("timed out waiting for fake Ghidra")

    def assert_pid_gone(self, pid: int) -> None:
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)

    def exercise_signal(
        self,
        signal_number: signal.Signals,
        expected_status: int,
        *,
        discovery_only: bool = False,
    ) -> None:
        first = self.launch(discovery_only=discovery_only)
        first_ghidra_pid = self.wait_for_active(first)
        first.send_signal(signal_number)
        stdout, stderr = first.communicate(timeout=15)
        self.assertEqual(first.returncode, expected_status, (stdout, stderr))
        self.assertFalse(self.active.exists())
        self.assert_pid_gone(first_ghidra_pid)

        attempts = sorted(self.workspace.glob("ghidra-snapshot-bank.*"))
        self.assertEqual(len(attempts), 1)
        receipt = json.loads(
            (attempts[0] / "diagnostics/runner-interruption.json").read_text()
        )
        self.assertEqual(receipt["signal"], signal_number.name.removeprefix("SIG"))
        self.assertEqual(receipt["phase"], "analysis_unseeded")
        self.assertEqual(receipt["active_guard_cleanup_complete"], True)

        second = self.launch(discovery_only=discovery_only)
        second_ghidra_pid = self.wait_for_active(second)
        self.assertFalse(self.overlap.exists(), "a second Ghidra overlapped the first")
        second.terminate()
        second.communicate(timeout=15)
        self.assertFalse(self.active.exists())
        self.assert_pid_gone(second_ghidra_pid)

    def test_term_waits_for_guard_cleanup_before_returning(self):
        self.exercise_signal(signal.SIGTERM, 143)

    def test_int_waits_for_guard_cleanup_before_returning(self):
        self.exercise_signal(signal.SIGINT, 130)

    def test_hup_waits_for_guard_cleanup_before_returning(self):
        self.exercise_signal(signal.SIGHUP, 129)

    def test_discovery_only_term_waits_for_guard_cleanup_before_returning(self):
        self.exercise_signal(signal.SIGTERM, 143, discovery_only=True)

    def test_signal_after_final_guard_cannot_publish_success(self):
        self.quick.touch()
        process = self.launch()
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and not self.post_ingest.exists():
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                self.fail(f"runner exited before post-ingest boundary\n{stdout}\n{stderr}")
            time.sleep(0.01)
        self.assertTrue(self.post_ingest.exists())
        process.terminate()
        stdout, stderr = process.communicate(timeout=15)
        self.assertEqual(process.returncode, 143, (stdout, stderr))
        attempts = sorted(self.workspace.glob("ghidra-snapshot-bank.*"))
        self.assertEqual(len(attempts), 1)
        receipt = json.loads(
            (attempts[0] / "diagnostics/runner-interruption.json").read_text()
        )
        self.assertEqual(receipt["signal"], "TERM")
        self.assertEqual(receipt["phase"], "after_ingest")
        self.assertFalse((attempts[0] / "out/receipt.json").exists())


if __name__ == "__main__":
    unittest.main()
