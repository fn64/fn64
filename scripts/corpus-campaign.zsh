#!/bin/zsh
# One command sweeps a private ROM corpus from a pinned clean build and ranks
# blockers, so a full-corpus measurement costs no model tokens.
#
#   scripts/corpus-campaign.zsh --rom-dir /path/to/roms
#
# Chains: rom-catalog.py -> rom-frontier.py -> import-rom-frontier-campaign.py
#         -> corpus-recompile-sweep.py -> corpus-dashboard.py
#         -> corpus-unblock-rank.py
# into <campaign-root>/campaign-<YYYYMMDD>-<rev8>/ (default campaign root:
# $HOME/.cache/fn64-corpus). The temp-directory tree is never used:
# everything private lives under the campaign directory, whose private/
# subdirectory this script chmod 700's.
#
# Every printed line is path-free except the final "campaign dir:" line.

set -euo pipefail

typeset -r script_path=$0
typeset -r fn64_root=$(cd -- "$(dirname -- "$script_path")/.." && pwd -P)

usage() {
  print -u2 -- "usage: $script_path --rom-dir DIR [--campaign-root DIR] [--limit N]"
  print -u2 -- "       [--rom-ids FILE] [--resume DIR] [--jobs N] [--discover-timeout SECS]"
  print -u2 -- "       [--recompile-timeout SECS] [--allow-dirty] [--skip-build] [--binary PATH]"
  print -u2 -- "       [--dry-run]"
}

typeset rom_dir=
typeset campaign_root="$HOME/.cache/fn64-corpus"
typeset limit=
typeset rom_ids_file=
typeset resume_dir=
typeset -i jobs=2
typeset -i discover_timeout=600
typeset -i recompile_timeout=1200
typeset -i allow_dirty=0
typeset -i skip_build=0
typeset binary="$fn64_root/target/release/fn64-discover"
typeset -i dry_run=0

while (( $# > 0 )); do
  case $1 in
    --rom-dir)
      (( $# >= 2 )) || { usage; exit 2; }
      rom_dir=$2; shift 2 ;;
    --campaign-root)
      (( $# >= 2 )) || { usage; exit 2; }
      campaign_root=$2; shift 2 ;;
    --limit)
      (( $# >= 2 )) || { usage; exit 2; }
      limit=$2; shift 2 ;;
    --rom-ids)
      (( $# >= 2 )) || { usage; exit 2; }
      rom_ids_file=$2; shift 2 ;;
    --resume)
      (( $# >= 2 )) || { usage; exit 2; }
      resume_dir=$2; shift 2 ;;
    --jobs)
      (( $# >= 2 )) || { usage; exit 2; }
      jobs=$2; shift 2 ;;
    --discover-timeout)
      (( $# >= 2 )) || { usage; exit 2; }
      discover_timeout=$2; shift 2 ;;
    --recompile-timeout)
      (( $# >= 2 )) || { usage; exit 2; }
      recompile_timeout=$2; shift 2 ;;
    --allow-dirty)
      allow_dirty=1; shift ;;
    --skip-build)
      skip_build=1; shift ;;
    --binary)
      (( $# >= 2 )) || { usage; exit 2; }
      binary=$2; shift 2 ;;
    --dry-run)
      dry_run=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      print -u2 -- "$script_path: FATAL: unrecognized argument: $1"
      usage
      exit 2 ;;
  esac
done

if [[ -z "$rom_dir" ]]; then
  print -u2 -- "$script_path: FATAL: --rom-dir is required"
  usage
  exit 2
fi

typeset git_rev= git_rev8=
git_rev=$(cd -- "$fn64_root" && git rev-parse HEAD)
git_rev8=${git_rev[1,8]}

typeset campaign_dir=
if [[ -n "$resume_dir" ]]; then
  campaign_dir=$resume_dir
else
  campaign_dir="$campaign_root/campaign-$(date -u +%Y%m%d)-$git_rev8"
fi
typeset private_dir="$campaign_dir/private"
typeset catalog_path="$private_dir/catalog.jsonl"
typeset rom_map_path="$private_dir/rom-map.json"
typeset frontier_path="$private_dir/frontier.jsonl"
typeset frontier_failures_path="$private_dir/frontier-failures.jsonl"
typeset receipts_import_dir="$campaign_dir/receipts-import"
typeset candidate_path="$private_dir/candidate.json"
typeset campaign_id=$(basename -- "$campaign_dir")

if [[ -n "$rom_ids_file" ]]; then
  rom_ids_file=$(cd -- "$(dirname -- "$rom_ids_file")" && pwd -P)/$(basename -- "$rom_ids_file")
fi
if [[ ! -d "$rom_dir" ]]; then
  print -u2 -- "corpus-campaign: FATAL: --rom-dir $rom_dir is not a directory"
  exit 1
fi
rom_dir=$(cd -- "$rom_dir" && pwd -P)

# --------------------------------------------------------------------------
# --dry-run: print the exact command list, in stage order, and exit without
# running anything.
# --------------------------------------------------------------------------
if (( dry_run )); then
  print -- "corpus-campaign: dry-run plan"
  print -- "  campaign dir: $campaign_dir"
  print -- "  resume: $([[ -n $resume_dir ]] && print -n yes || print -n no)"
  print -- "  limit: ${limit:-<none>}"
  print -- "  skip-build: $skip_build"
  print --
  if (( ! skip_build )); then
    print -- "1) build: CARGO_INCREMENTAL=0 cargo build --release -p fn64-discover --bin fn64-discover"
  else
    print -- "1) build: skipped (--skip-build)"
  fi
  print -- "2) stage1 catalog: python3 scripts/rom-catalog.py --rom-dir $rom_dir --deduplicate-identities --output $catalog_path"
  print -- "   stage1 rom-map: python3 - scripts/rom-catalog.py $rom_dir $rom_map_path <<inline-heredoc>> (derived via rom-catalog.discover_roms + rom-catalog.catalog_rom, imported by path)"
  typeset frontier_cmd="python3 scripts/rom-frontier.py --catalog $catalog_path --binary $binary --rom-dir $rom_dir --output $frontier_path --failures-output $frontier_failures_path --timeout-seconds $discover_timeout --jobs $jobs"
  [[ -n "$limit" ]] && frontier_cmd+=" --limit $limit"
  print -- "3) stage2 rom-frontier: $frontier_cmd"
  print -- "4) stage3 import: python3 scripts/import-rom-frontier-campaign.py --catalog $catalog_path --frontier $frontier_path --failures $frontier_failures_path --output-dir $receipts_import_dir --campaign-id $campaign_id --finished-at <now-utc>"
  print -- "   stage3 merge: move $receipts_import_dir/manifest.json -> $campaign_dir/manifest.json; merge $receipts_import_dir/receipts/* -> $campaign_dir/receipts/"
  typeset sweep_cmd="python3 scripts/corpus-recompile-sweep.py --campaign-dir $campaign_dir --binary $binary --rom-map $rom_map_path --candidate $candidate_path --jobs $jobs --timeout-seconds $recompile_timeout"
  [[ -n "$limit" ]] && sweep_cmd+=" --limit $limit"
  [[ -n "$rom_ids_file" ]] && sweep_cmd+=" --rom-ids $rom_ids_file"
  [[ -n "$resume_dir" ]] && sweep_cmd+=" --resume"
  print -- "5) stage4 recompile-sweep: $sweep_cmd"
  print -- "6) stage5 dashboard: python3 scripts/corpus-dashboard.py --manifest $campaign_dir/manifest.json --receipts $campaign_dir/receipts --output-json $campaign_dir/dashboard-<utc-timestamp>.json --output-html $campaign_dir/dashboard-<utc-timestamp>.html"
  print -- "   stage5 rank: python3 scripts/corpus-unblock-rank.py --campaign-dir $campaign_dir --dashboard-json <dashboard.json> --stage recompile (and --stage discover) -> $campaign_dir/rank-<utc-timestamp>.txt"
  exit 0
fi

# --------------------------------------------------------------------------
# Real run
# --------------------------------------------------------------------------

typeset -i is_dirty=0
if [[ -n $(cd -- "$fn64_root" && git status --porcelain) ]]; then
  is_dirty=1
fi
if (( is_dirty && ! allow_dirty )); then
  print -u2 -- "corpus-campaign: FATAL: working tree is dirty; pass --allow-dirty to record dirty=true"
  exit 1
fi

typeset -A stage_wall_start stage_wall_end

now_epoch() { date +%s.%N }
now_utc_iso() { date -u +%Y-%m-%dT%H:%M:%SZ }

if (( ! skip_build )); then
  stage_wall_start[build]=$(now_epoch)
  print -- "corpus-campaign: building fn64-discover (release)"
  ( cd -- "$fn64_root" && CARGO_INCREMENTAL=0 cargo build --release -p fn64-discover --bin fn64-discover )
  stage_wall_end[build]=$(now_epoch)
fi

if [[ ! -x "$binary" ]]; then
  print -u2 -- "corpus-campaign: FATAL: binary is not an executable file (build it first or pass --binary)"
  exit 1
fi

typeset binary_sha256 toolchain
binary_sha256=$(shasum -a 256 "$binary" | awk '{print $1}')
toolchain=$(rustc --version)

mkdir -p "$campaign_root"
mkdir -p "$campaign_dir"
mkdir -p "$private_dir"
chmod 700 "$private_dir"

python3 - "$candidate_path" "$git_rev" "$is_dirty" "$binary_sha256" "$toolchain" <<'PY'
import json
import sys

path, git_rev, is_dirty, binary_sha256, toolchain = sys.argv[1:6]
candidate = {
    "git_commit": git_rev,
    "dirty": is_dirty == "1",
    "binary_sha256": binary_sha256,
    "toolchain": toolchain,
    "features": [],
}
with open(path, "w") as handle:
    json.dump(candidate, handle, sort_keys=True, separators=(",", ":"))
    handle.write("\n")
PY

# --- Stage 1: catalog + rom-map --------------------------------------------
stage_wall_start[catalog]=$(now_epoch)
if [[ -n "$resume_dir" && -f "$catalog_path" ]]; then
  print -- "corpus-campaign: stage1 catalog: reusing existing catalog (--resume)"
else
  print -- "corpus-campaign: stage1 catalog: running rom-catalog.py"
  python3 "$fn64_root/scripts/rom-catalog.py" --rom-dir "$rom_dir" --deduplicate-identities --output "$catalog_path"
fi
if [[ -n "$resume_dir" && -f "$rom_map_path" ]]; then
  print -- "corpus-campaign: stage1 rom-map: reusing existing rom-map (--resume)"
else
  print -- "corpus-campaign: stage1 rom-map: deriving normalized_sha256 -> path table"
  # The catalog is path-free by design, so the private sha256->path table is
  # rebuilt by re-walking --rom-dir with rom-catalog.py's OWN discover_roms
  # and catalog_rom helpers (imported as a module by path), never by
  # re-implementing ROM normalization here.
  python3 - "$fn64_root/scripts/rom-catalog.py" "$rom_dir" "$rom_map_path" <<'PY'
import importlib.util
import json
import sys
from pathlib import Path

module_path, rom_dir, output_path = sys.argv[1:4]

spec = importlib.util.spec_from_file_location("rom_catalog", module_path)
rom_catalog = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rom_catalog)

roms = rom_catalog.discover_roms(Path(rom_dir))
entries = {}
for rom_path in roms:
    record = rom_catalog.catalog_rom(rom_path)
    entries[record["normalized_rom_sha256"]] = str(rom_path.resolve())

with open(output_path, "w") as handle:
    json.dump(entries, handle, sort_keys=True, separators=(",", ":"))
    handle.write("\n")
PY
fi
chmod 700 "$private_dir"
stage_wall_end[catalog]=$(now_epoch)

# --- Stage 2: rom-frontier --------------------------------------------------
stage_wall_start[frontier]=$(now_epoch)
if [[ -n "$resume_dir" && -f "$frontier_path" && -f "$frontier_failures_path" ]]; then
  print -- "corpus-campaign: stage2 frontier: reusing existing frontier outputs (--resume)"
else
  print -- "corpus-campaign: stage2 frontier: running rom-frontier.py"
  typeset -a frontier_args
  frontier_args=(
    --catalog "$catalog_path"
    --binary "$binary"
    --rom-dir "$rom_dir"
    --output "$frontier_path"
    --failures-output "$frontier_failures_path"
    --timeout-seconds "$discover_timeout"
    --jobs "$jobs"
  )
  [[ -n "$limit" ]] && frontier_args+=(--limit "$limit")
  python3 "$fn64_root/scripts/rom-frontier.py" "${frontier_args[@]}"
fi
stage_wall_end[frontier]=$(now_epoch)

# --- Stage 3: import into campaign manifest/receipts ------------------------
stage_wall_start[import]=$(now_epoch)
if [[ -n "$resume_dir" && -f "$campaign_dir/manifest.json" ]]; then
  print -- "corpus-campaign: stage3 import: reusing existing manifest (--resume)"
else
  print -- "corpus-campaign: stage3 import: running import-rom-frontier-campaign.py"
  if [[ -d "$receipts_import_dir" ]]; then
    print -u2 -- "corpus-campaign: FATAL: $receipts_import_dir already exists; import-rom-frontier-campaign.py requires a new directory. Remove it or use --resume with an already-imported campaign."
    exit 1
  fi
  python3 "$fn64_root/scripts/import-rom-frontier-campaign.py" \
    --catalog "$catalog_path" \
    --frontier "$frontier_path" \
    --failures "$frontier_failures_path" \
    --output-dir "$receipts_import_dir" \
    --campaign-id "$campaign_id" \
    --finished-at "$(now_utc_iso)"

  if [[ -f "$campaign_dir/manifest.json" ]]; then
    print -u2 -- "corpus-campaign: FATAL: $campaign_dir/manifest.json already exists but --resume was not requested"
    exit 1
  fi
  mv "$receipts_import_dir/manifest.json" "$campaign_dir/manifest.json"
  mkdir -p "$campaign_dir/receipts"
  # Merge (not replace): a --resume run may already hold pack/recompile
  # receipts under receipts/ from a prior stage4 attempt.
  for f in "$receipts_import_dir/receipts"/*.json(N); do
    mv "$f" "$campaign_dir/receipts/"
  done
  if [[ -f "$receipts_import_dir/unattributed-diagnostics.jsonl" ]]; then
    mv "$receipts_import_dir/unattributed-diagnostics.jsonl" "$campaign_dir/unattributed-diagnostics.jsonl"
  fi
  rmdir "$receipts_import_dir/receipts" 2>/dev/null || true
  rmdir "$receipts_import_dir" 2>/dev/null || true
fi
stage_wall_end[import]=$(now_epoch)

# --- Stage 4: recompile sweep (pack + recompile receipts) -------------------
stage_wall_start[recompile]=$(now_epoch)
print -- "corpus-campaign: stage4 recompile-sweep: running corpus-recompile-sweep.py"
typeset -a sweep_args
sweep_args=(
  --campaign-dir "$campaign_dir"
  --binary "$binary"
  --rom-map "$rom_map_path"
  --candidate "$candidate_path"
  --jobs "$jobs"
  --timeout-seconds "$recompile_timeout"
)
[[ -n "$limit" ]] && sweep_args+=(--limit "$limit")
[[ -n "$rom_ids_file" ]] && sweep_args+=(--rom-ids "$rom_ids_file")
[[ -n "$resume_dir" ]] && sweep_args+=(--resume)
python3 "$fn64_root/scripts/corpus-recompile-sweep.py" "${sweep_args[@]}"
stage_wall_end[recompile]=$(now_epoch)

# --- Stage 5: dashboard + unblock-rank --------------------------------------
stage_wall_start[dashboard]=$(now_epoch)
typeset utc_stamp=$(date -u +%Y%m%dT%H%M%SZ)
typeset dashboard_json="$campaign_dir/dashboard-$utc_stamp.json"
typeset dashboard_html="$campaign_dir/dashboard-$utc_stamp.html"
print -- "corpus-campaign: stage5 dashboard: running corpus-dashboard.py"
python3 "$fn64_root/scripts/corpus-dashboard.py" \
  --manifest "$campaign_dir/manifest.json" \
  --receipts "$campaign_dir/receipts" \
  --output-json "$dashboard_json" \
  --output-html "$dashboard_html"

typeset rank_path="$campaign_dir/rank-$utc_stamp.txt"
print -- "corpus-campaign: stage5 unblock-rank: running corpus-unblock-rank.py (--stage recompile, --stage discover)"
: > "$rank_path"
{
  print -- "=== stage: recompile ==="
  python3 "$fn64_root/scripts/corpus-unblock-rank.py" \
    --campaign-dir "$campaign_dir" \
    --dashboard-json "$dashboard_json" \
    --stage recompile
  print --
  print -- "=== stage: discover ==="
  python3 "$fn64_root/scripts/corpus-unblock-rank.py" \
    --campaign-dir "$campaign_dir" \
    --dashboard-json "$dashboard_json" \
    --stage discover
} | tee "$rank_path"
# corpus-unblock-rank.py's --stage flag only supports recompile/discover
# (checked against its argparse `choices`); pack is not a supported stage,
# so it is intentionally skipped here rather than invoked and failing.
stage_wall_end[dashboard]=$(now_epoch)

# --------------------------------------------------------------------------
# Timing summary: per-stage wall seconds, plus p50/p95 wall_time_ms and max
# peak_rss_bytes from pack/recompile receipt policy blocks. Path-free.
# --------------------------------------------------------------------------
print --
print -- "corpus-campaign: timing summary"
typeset stage_name
for stage_name in build catalog frontier import recompile dashboard; do
  if [[ -n "${stage_wall_start[$stage_name]:-}" && -n "${stage_wall_end[$stage_name]:-}" ]]; then
    typeset -F 3 elapsed=$(( ${stage_wall_end[$stage_name]} - ${stage_wall_start[$stage_name]} ))
    print -- "  stage=$stage_name wall_seconds=$elapsed"
  else
    print -- "  stage=$stage_name wall_seconds=<skipped>"
  fi
done

python3 - "$campaign_dir/receipts" <<'PY'
import json
import sys
from pathlib import Path

receipts_dir = Path(sys.argv[1])


def percentile(sorted_values, pct):
    if not sorted_values:
        return None
    if len(sorted_values) == 1:
        return sorted_values[0]
    rank = (pct / 100) * (len(sorted_values) - 1)
    low = int(rank)
    high = min(low + 1, len(sorted_values) - 1)
    frac = rank - low
    return sorted_values[low] + (sorted_values[high] - sorted_values[low]) * frac


for stage in ("pack", "recompile"):
    wall_times = []
    peak_rss = []
    for path in sorted(receipts_dir.rglob("*.json")):
        try:
            receipt = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if receipt.get("stage") != stage:
            continue
        policy = receipt.get("policy")
        if not isinstance(policy, dict):
            continue
        wall_time_ms = policy.get("wall_time_ms")
        if isinstance(wall_time_ms, (int, float)):
            wall_times.append(wall_time_ms)
        rss = policy.get("peak_rss_bytes")
        if isinstance(rss, (int, float)):
            peak_rss.append(rss)
    wall_times.sort()
    p50 = percentile(wall_times, 50)
    p95 = percentile(wall_times, 95)
    max_rss = max(peak_rss) if peak_rss else None
    print(
        f"  stage={stage} receipts={len(wall_times)} "
        f"p50_wall_ms={p50 if p50 is None else round(p50)} "
        f"p95_wall_ms={p95 if p95 is None else round(p95)} "
        f"max_peak_rss_bytes={max_rss}"
    )
PY

print --
print -- "campaign dir: $campaign_dir"
