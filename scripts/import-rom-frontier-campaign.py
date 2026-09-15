#!/usr/bin/env python3
"""Import path-free rom-frontier rows as canonical discovery-stage receipts."""
from __future__ import annotations
import argparse, hashlib, json, os, sys
from pathlib import Path

CATALOG = "fn64.rom-catalog.v1"; FRONTIER = "fn64.rom-frontier.v2"
MANIFEST = "fn64.corpus-campaign.v1"; RECEIPT = "fn64.corpus-stage-receipt.v1"

def canonical(v): return json.dumps(v, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
def rows(path): return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
def main():
 p=argparse.ArgumentParser(description=__doc__); p.add_argument("--catalog",type=Path,required=True); p.add_argument("--frontier",type=Path,required=True); p.add_argument("--failures",type=Path,required=True); p.add_argument("--output-dir",type=Path,required=True); p.add_argument("--campaign-id",required=True); p.add_argument("--finished-at",required=True); a=p.parse_args()
 if a.output_dir.exists() or not a.output_dir.is_absolute() or ".." in a.output_dir.parts: raise SystemExit("import-rom-frontier: output-dir must be a new absolute path")
 catalog=rows(a.catalog); frontier=rows(a.frontier); failures=rows(a.failures)
 if not catalog or any(r.get("schema")!=CATALOG for r in catalog): raise SystemExit("import-rom-frontier: invalid catalog")
 if any(r.get("schema")!=FRONTIER for r in frontier): raise SystemExit("import-rom-frontier: invalid frontier")
 ids={r["normalized_rom_sha256"]:r for r in catalog}
 if len(ids)!=len(catalog) or any(r["normalized_rom_sha256"] not in ids for r in frontier): raise SystemExit("import-rom-frontier: digest mismatch")
 a.output_dir.mkdir(); receipts=a.output_dir/"receipts"; receipts.mkdir()
 manifest={"schema":MANIFEST,"campaign_id":a.campaign_id,"roms":[{"id":r["stable_id"],"normalized_sha256":r["normalized_rom_sha256"]} for r in catalog]}
 (a.output_dir/"manifest.json").write_bytes(canonical(manifest)+b"\n")
 for row in frontier:
  rom=ids[row["normalized_rom_sha256"]]; payload=canonical(row); rid="frontier-discover-"+hashlib.sha256(payload).hexdigest()
  result={"frontier":row}
  code_run_bytes=rom.get("code_run_bytes")
  if isinstance(code_run_bytes,int) and not isinstance(code_run_bytes,bool): result["code_run_bytes"]=code_run_bytes
  receipt={"schema":RECEIPT,"receipt_id":rid,"campaign_id":a.campaign_id,"attempt_id":"frontier-full-20260913","stage":"discover","rom":{"id":rom["stable_id"],"normalized_sha256":row["normalized_rom_sha256"]},"predecessors":[],"outcome":{"kind":"passed","frontier":None},"result":result,"finished_at":a.finished_at}
  (receipts/(rid+".json")).write_bytes(canonical(receipt)+b"\n")
 (a.output_dir/"unattributed-diagnostics.jsonl").write_bytes(b"".join(canonical(x)+b"\n" for x in failures))
 print(f"import-rom-frontier: receipts={len(frontier)} unattributed_diagnostics={len(failures)}")
if __name__=="__main__": main()
