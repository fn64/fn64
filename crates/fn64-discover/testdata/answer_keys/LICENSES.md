# Vendored answer-key symbol tables — provenance and licenses

These files are grading keys consumed *only after* discovery (see
`src/answer_keys.rs`, `src/bin/gate_keys.rs`). They contain function/symbol
names and addresses only — no ROM bytes, no game-derived binary content — and
are safe to vendor for git under the licenses recorded here. Verified
2026-07-18.

## banjo_kazooie.symbol_addrs.us.v10.txt

- Source repo: https://github.com/n64decomp/banjo-kazooie
- Path in repo: `symbol_addrs.us.v10.txt` (repo root)
- Pinned commit: `1b2edf8bea686b6bfb6f35277606439991351a5b`
- Raw URL:
  https://raw.githubusercontent.com/n64decomp/banjo-kazooie/1b2edf8bea686b6bfb6f35277606439991351a5b/symbol_addrs.us.v10.txt
- License: CC0-1.0 (repo `LICENSE` — "CC0 1.0 Universal"). Public-domain
  dedication; vendoring the symbol table is permitted.
- SHA-256 of vendored bytes (gated by
  `answer_keys::tests::banjo_vendored_bytes_match_recorded_digest`):
  `66ba957b7c6b4f8a58150456b3cf014447b11cc1abe4d631d6059bcc13f86420`
- Byte-identical on repeated fetch at the pinned commit (verified).
- Format: splat `symbol_addrs` syntax — `name = 0xADDRESS;` with optional
  `// key:value` attribute comments (`allow_duplicated:true`, `name_end:...`).
  This particular file is the project's hand-maintained *override* list (60
  rows), NOT a full per-function boundary table: it carries no `type:` or
  `size:` attributes, so function-vs-data classification falls back to splat's
  name-prefix convention (`D_*`/`jtbl_*`/`jpt_*`/`rodata*` = data; everything
  else at a code address = function). The parser records this explicitly.
- Associated ROM (grading target, user-owned, NOT vendored): Banjo-Kazooie
  (USA) v1.0, ROM SHA-1 `1fb13cad402518d3ae9a8dc4b52c5c54b2a4adc7`, internal
  name "BANJO KAZOOIE", cartridge id `NBKE`
  (from `decompressed.us.v10.yaml` at the pinned commit).

## Perfect Dark — NOT vendored (source discrepancy, see gate_keys.rs)

The task premise cited `github.com/n64decomp/perfect_dark` as an MIT splat
decomp carrying `symbol_addrs.us.v10.txt` at its repo root. That file does not
exist in that repository at any commit: `n64decomp/perfect_dark` (HEAD
commit `169ed48bdcbfb3b568b028bd5bebb27680073514`, MIT — verified) is an armips-based
*matching* decomp whose symbols live in `ld/*.inc` / `ld/pd.ld` linker
scripts, not in a splat `symbol_addrs` text table. No splat-style Perfect Dark
symbol_addrs table could be located under that org. The Perfect Dark title is
therefore registered in `answer_keys.rs` with `key_file: None` (loudly
skipped, never fabricated) until a valid, license-verified source is
identified. The parser and gate are title-generic: dropping a real
`symbol_addrs`-format file in here and pointing the registry at it is all that
is needed to activate PD grading.
