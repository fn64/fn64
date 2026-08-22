#!/bin/zsh
# GATE: the wgpu backend against the RT64 C++ oracle, one command stream,
# both engines, byte-compared.
#
# This gate exists because 49 of 50 rows in docs/rt64-port-parity.json sat
# RUST_PENDING while ~13,600 in-crate assertions stayed green: those tests
# pin fn64's OWN output, so they cannot detect fn64 disagreeing with the
# oracle. Two mundane blockers had silently broken this path -- FN64_RT64_DIR
# unset, and a RawRdpScan tri-state that no other gate compiled -- and
# nothing failed, because nothing ran it.
#
# It MUST be able to fail. A gate that cannot compare exits nonzero rather
# than reporting success on unusable input (docs: gates-must-fail-on-unusable-input).
set -euo pipefail

: ${FN64_RT64_DIR:=$HOME/Code/no-mercy-recompiled/third_party/rt64}
export FN64_RT64_DIR
if [[ ! -d "$FN64_RT64_DIR" ]]; then
  echo "[gate-rt64-parity] FATAL: RT64 source tree not found at $FN64_RT64_DIR" >&2
  echo "[gate-rt64-parity] Set FN64_RT64_DIR to the MIT RT64 checkout." >&2
  exit 1
fi

# Expected divergences, each one MEASURED and explained in the runner's own
# `intent` text. Any NEW divergence fails the gate; any of these DISAPPEARING
# also fails it, because a vanished known-difference means the corpus stopped
# exercising the case.
export EXPECTED_DIFFERS=${EXPECTED_DIFFERS:-1}
export EXPECTED_ONE_REFUSED=${EXPECTED_ONE_REFUSED:-1}
export MIN_AUTHORITATIVE_CASES=${MIN_AUTHORITATIVE_CASES:-19}

echo "[gate-rt64-parity] building parity runner (RT64 C++ + wgpu)"
cargo build -p fn64-render-conformance --features parity-runner \
  --bin fn64-render-conformance-parity-runner --offline

RUNNER=target/debug/fn64-render-conformance-parity-runner
[[ -x "$RUNNER" ]] || { echo "[gate-rt64-parity] FATAL: runner missing" >&2; exit 1; }

echo "[gate-rt64-parity] running three-way differential"
OUT=$("$RUNNER" 2>/dev/null)

echo "$OUT" | python3 "$(dirname "$0")/check_rt64_parity.py"
