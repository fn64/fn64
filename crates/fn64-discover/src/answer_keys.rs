//! Grading-only ingestion of vendored splat-format `symbol_addrs` tables as
//! answer keys (see `testdata/answer_keys/LICENSES.md` for provenance).
//!
//! This module is reachable only from `bin/gate_keys.rs`; no detector in the
//! discovery pipeline consumes it. It parses the splat `symbol_addrs` syntax
//!
//! ```text
//! name  = 0xADDRESS;            // key:value key:value ...
//! ```
//!
//! into typed rows, classifies each as a function-boundary or data row, and
//! extracts the function-boundary set used to grade Phase 3 function-entry
//! candidates. Malformed lines are never silently dropped: every non-empty
//! source line is either parsed into a row or recorded as a skip with an
//! explicit reason (see [`SkipReason`]), and a parse that would drop a line
//! for an unrecognized reason fails loudly.
//!
//! # Classification honesty
//!
//! A splat `symbol_addrs` row is authoritatively a function only when it
//! carries an explicit `type:func` attribute. Vendored override tables (e.g.
//! Banjo-Kazooie's root `symbol_addrs.us.v10.txt`) frequently omit `type:`
//! entirely, so a name-prefix fallback is used: splat's data conventions
//! (`D_*`, `jtbl_*`, `jpt_*`, `rodata*`, `L*` local labels) are treated as
//! data; every other symbol sitting at a code-range address is treated as a
//! function boundary. Each row records *how* it was classified
//! ([`FunctionClass`]) so a caller can require explicit-typed rows only when
//! it must. Nothing here guesses an address or a size.

use std::collections::BTreeMap;
use std::fmt;

/// N64 kernel-segment (KSEG0/KSEG1) virtual-address floor. A splat symbol
/// whose address is below this is not a runtime code/data VA (it is a raw
/// ROM offset or a small constant) and is never a function boundary.
const N64_VA_FLOOR: u32 = 0x8000_0000;

/// Splat name prefixes that denote generated *data* symbols, never functions.
/// `L` alone is intentionally excluded (real function names begin with `L`);
/// only the splat local-label form `L8XXXXXXXX` matched by [`is_data_prefix`]
/// is treated as data.
const DATA_PREFIXES: &[&str] = &["D_", "jtbl_", "jpt_", "rodata"];

/// One parsed `symbol_addrs` row: a symbol name, its address, and any
/// `// key:value` attributes that followed on the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRow {
    pub name: String,
    pub address: u32,
    /// `size:0x...` attribute if present, else `None`. Splat override tables
    /// usually omit it; grading that needs an extent must handle `None`.
    pub size: Option<u32>,
    /// The explicit `type:` attribute if present (e.g. `"func"`, `"data"`),
    /// else `None`.
    pub explicit_type: Option<String>,
    /// All `key:value` attributes verbatim, in source order, for callers that
    /// need attributes this struct does not lift out.
    pub attributes: Vec<(String, String)>,
    /// 1-indexed source line, for diagnostics.
    pub line: usize,
}

/// How a row was classified as a function boundary (or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionClass {
    /// Carried an explicit `type:func` attribute — authoritative.
    ExplicitFunc,
    /// No explicit `type:`, but the name is not a data-convention prefix and
    /// the address is in the code VA range — a fallback function boundary.
    InferredFromNameAndAddress,
    /// Carried an explicit non-`func` `type:` (e.g. `type:data`).
    ExplicitNonFunc,
    /// A splat data-convention name prefix (`D_*`, `jtbl_*`, ...).
    DataPrefix,
    /// Address is below the N64 code/data VA floor (a raw ROM offset or small
    /// constant) — never a runtime function boundary.
    BelowVaFloor,
}

impl FunctionClass {
    /// Whether this classification counts the row as a function boundary.
    pub fn is_function(self) -> bool {
        matches!(self, Self::ExplicitFunc | Self::InferredFromNameAndAddress)
    }
}

/// Why a source line was skipped (i.e. produced no [`SymbolRow`]). Every
/// skipped line lands in exactly one of these categories; the parser never
/// drops a line for an unrecorded reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
    /// Blank or whitespace-only.
    Blank,
    /// A full-line comment (`// ...` with no assignment).
    CommentOnly,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => write!(f, "blank"),
            Self::CommentOnly => write!(f, "comment-only"),
        }
    }
}

/// A hard parse error on a line that *looked* like an assignment but was
/// malformed. These fail the whole parse loudly rather than being counted as
/// skips: a silently dropped assignment would understate the key and inflate
/// recall (AGENTS.md "Loud traps, no silent shrugs").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub text: String,
    pub reason: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "symbol_addrs line {}: {} (in {:?})",
            self.line, self.reason, self.text
        )
    }
}

impl std::error::Error for ParseError {}

/// The fully parsed table: every well-formed row, the per-reason skip counts,
/// and the exact classification of every row.
#[derive(Debug, Clone)]
pub struct ParsedSymbolTable {
    pub rows: Vec<SymbolRow>,
    classes: Vec<FunctionClass>,
    skipped: BTreeMap<SkipReason, usize>,
}

impl ParsedSymbolTable {
    /// Total well-formed rows parsed (functions + data + below-floor).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Rows classified as function boundaries, most-authoritative first is not
    /// guaranteed; order follows source order.
    pub fn functions(&self) -> impl Iterator<Item = (&SymbolRow, FunctionClass)> {
        self.rows
            .iter()
            .zip(self.classes.iter().copied())
            .filter(|(_, class)| class.is_function())
    }

    /// Exact count of function-boundary rows.
    pub fn function_count(&self) -> usize {
        self.classes.iter().filter(|c| c.is_function()).count()
    }

    /// Count of function-boundary rows carrying an explicit `type:func`.
    pub fn explicit_function_count(&self) -> usize {
        self.classes
            .iter()
            .filter(|c| matches!(c, FunctionClass::ExplicitFunc))
            .count()
    }

    /// Count of function-boundary rows inferred from name + address (no
    /// explicit `type:`).
    pub fn inferred_function_count(&self) -> usize {
        self.classes
            .iter()
            .filter(|c| matches!(c, FunctionClass::InferredFromNameAndAddress))
            .count()
    }

    /// Rows classified as data (any non-function reason).
    pub fn data_count(&self) -> usize {
        self.classes.iter().filter(|c| !c.is_function()).count()
    }

    /// Skip count for one reason.
    pub fn skipped(&self, reason: SkipReason) -> usize {
        self.skipped.get(&reason).copied().unwrap_or(0)
    }

    /// Total skipped source lines.
    pub fn total_skipped(&self) -> usize {
        self.skipped.values().sum()
    }

    /// The classification of the row at `index` (same order as [`rows`]).
    pub fn class_at(&self, index: usize) -> Option<FunctionClass> {
        self.classes.get(index).copied()
    }
}

fn is_data_prefix(name: &str) -> bool {
    if DATA_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    // splat local labels: `L` followed immediately by a hex VA, e.g.
    // `L80251234`. A leading `L` on an otherwise-alphabetic name (a real
    // function) is NOT matched.
    if let Some(rest) = name.strip_prefix('L') {
        return rest.len() >= 6 && rest.chars().all(|c| c.is_ascii_hexdigit());
    }
    false
}

fn classify(name: &str, address: u32, explicit_type: Option<&str>) -> FunctionClass {
    match explicit_type {
        Some("func") => return FunctionClass::ExplicitFunc,
        Some(_) => return FunctionClass::ExplicitNonFunc,
        None => {}
    }
    if is_data_prefix(name) {
        return FunctionClass::DataPrefix;
    }
    if address < N64_VA_FLOOR {
        return FunctionClass::BelowVaFloor;
    }
    FunctionClass::InferredFromNameAndAddress
}

/// Parse a `key:value` attribute list from the text after `//`. splat
/// attributes are whitespace-separated `key:value` tokens; a token without a
/// `:` is a hard error (an unrecognized comment shape on an assignment line
/// must not be silently ignored).
fn parse_attributes(
    comment: &str,
    line: usize,
    full: &str,
) -> Result<Vec<(String, String)>, ParseError> {
    let mut out = Vec::new();
    for token in comment.split_whitespace() {
        let (key, value) = token.split_once(':').ok_or_else(|| ParseError {
            line,
            text: full.to_string(),
            reason: format!("comment token {token:?} is not a key:value attribute"),
        })?;
        out.push((key.to_string(), value.to_string()));
    }
    Ok(out)
}

/// Strictly parse splat `symbol_addrs` text. Blank and comment-only lines are
/// counted as skips; every other line must be a well-formed
/// `name = 0xADDRESS;` assignment (optionally with `// key:value` attributes)
/// or the whole parse fails with the offending line.
pub fn parse_symbol_addrs(text: &str) -> Result<ParsedSymbolTable, ParseError> {
    let mut rows = Vec::new();
    let mut classes = Vec::new();
    let mut skipped: BTreeMap<SkipReason, usize> = BTreeMap::new();

    for (idx, raw) in text.lines().enumerate() {
        let line = idx + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            *skipped.entry(SkipReason::Blank).or_insert(0) += 1;
            continue;
        }
        if trimmed.starts_with("//") {
            *skipped.entry(SkipReason::CommentOnly).or_insert(0) += 1;
            continue;
        }

        // Split off a trailing `// ...` comment (attributes).
        let (assign, comment) = match trimmed.split_once("//") {
            Some((a, c)) => (a.trim(), Some(c.trim())),
            None => (trimmed, None),
        };

        let attributes = match comment {
            Some(c) => parse_attributes(c, line, raw)?,
            None => Vec::new(),
        };

        let assign = assign.strip_suffix(';').ok_or_else(|| ParseError {
            line,
            text: raw.to_string(),
            reason: "assignment does not end with ';'".to_string(),
        })?;
        let (name, value) = assign.split_once('=').ok_or_else(|| ParseError {
            line,
            text: raw.to_string(),
            reason: "line is not a 'name = value' assignment".to_string(),
        })?;
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            return Err(ParseError {
                line,
                text: raw.to_string(),
                reason: "empty symbol name".to_string(),
            });
        }
        let hex = value.strip_prefix("0x").ok_or_else(|| ParseError {
            line,
            text: raw.to_string(),
            reason: format!("address {value:?} is not a 0x-prefixed hex literal"),
        })?;
        let address = u32::from_str_radix(hex, 16).map_err(|error| ParseError {
            line,
            text: raw.to_string(),
            reason: format!("address {value:?} is not a valid u32: {error}"),
        })?;

        let explicit_type = attributes
            .iter()
            .find(|(k, _)| k == "type")
            .map(|(_, v)| v.clone());
        let size = match attributes.iter().find(|(k, _)| k == "size") {
            Some((_, v)) => {
                let hex = v.strip_prefix("0x").ok_or_else(|| ParseError {
                    line,
                    text: raw.to_string(),
                    reason: format!("size {v:?} is not a 0x-prefixed hex literal"),
                })?;
                Some(u32::from_str_radix(hex, 16).map_err(|error| ParseError {
                    line,
                    text: raw.to_string(),
                    reason: format!("size {v:?} is not a valid u32: {error}"),
                })?)
            }
            None => None,
        };

        let class = classify(name, address, explicit_type.as_deref());
        rows.push(SymbolRow {
            name: name.to_string(),
            address,
            size,
            explicit_type,
            attributes,
            line,
        });
        classes.push(class);
    }

    Ok(ParsedSymbolTable {
        rows,
        classes,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_assignment_as_inferred_function() {
        let table = parse_symbol_addrs("osRomBase = 0x80000308;\n").unwrap();
        assert_eq!(table.row_count(), 1);
        assert_eq!(table.function_count(), 1);
        assert_eq!(table.inferred_function_count(), 1);
        assert_eq!(table.explicit_function_count(), 0);
        assert_eq!(table.rows[0].address, 0x8000_0308);
        assert_eq!(table.rows[0].size, None);
        assert_eq!(
            table.class_at(0),
            Some(FunctionClass::InferredFromNameAndAddress)
        );
    }

    #[test]
    fn data_prefix_and_below_floor_are_not_functions() {
        let table = parse_symbol_addrs("D_5E90 = 0x5E90;\nD_D846C0 = 0xD846C0;\n").unwrap();
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.function_count(), 0);
        assert_eq!(table.data_count(), 2);
        // D_ prefix wins regardless of address.
        assert_eq!(table.class_at(0), Some(FunctionClass::DataPrefix));
        assert_eq!(table.class_at(1), Some(FunctionClass::DataPrefix));
    }

    #[test]
    fn below_floor_non_data_name_is_not_a_function() {
        let table = parse_symbol_addrs("some_offset = 0x5E90;\n").unwrap();
        assert_eq!(table.function_count(), 0);
        assert_eq!(table.class_at(0), Some(FunctionClass::BelowVaFloor));
    }

    #[test]
    fn explicit_type_func_and_size_are_lifted() {
        let table = parse_symbol_addrs("func_x = 0x80251000; // type:func size:0x40\n").unwrap();
        assert_eq!(table.function_count(), 1);
        assert_eq!(table.explicit_function_count(), 1);
        assert_eq!(table.rows[0].size, Some(0x40));
        assert_eq!(table.rows[0].explicit_type.as_deref(), Some("func"));
        assert_eq!(table.class_at(0), Some(FunctionClass::ExplicitFunc));
    }

    #[test]
    fn explicit_type_data_overrides_code_address_and_name() {
        let table = parse_symbol_addrs("looks_like_fn = 0x80251000; // type:data\n").unwrap();
        assert_eq!(table.function_count(), 0);
        assert_eq!(table.class_at(0), Some(FunctionClass::ExplicitNonFunc));
    }

    #[test]
    fn allow_duplicated_attribute_is_parsed_not_dropped() {
        let table = parse_symbol_addrs("bzero = 0x800020F0; // allow_duplicated:true\n").unwrap();
        assert_eq!(table.function_count(), 1);
        assert_eq!(
            table.rows[0].attributes,
            vec![("allow_duplicated".to_string(), "true".to_string())]
        );
    }

    #[test]
    fn splat_local_label_is_data_but_leading_l_function_is_not() {
        let table =
            parse_symbol_addrs("L80251234 = 0x80251234;\nLoadThing = 0x80251240;\n").unwrap();
        assert_eq!(table.class_at(0), Some(FunctionClass::DataPrefix));
        assert_eq!(
            table.class_at(1),
            Some(FunctionClass::InferredFromNameAndAddress)
        );
        assert_eq!(table.function_count(), 1);
    }

    #[test]
    fn blank_and_comment_lines_are_counted_skips() {
        let table = parse_symbol_addrs("\n// header comment\nfoo = 0x80000000;\n\n").unwrap();
        assert_eq!(table.row_count(), 1);
        assert_eq!(table.skipped(SkipReason::Blank), 2);
        assert_eq!(table.skipped(SkipReason::CommentOnly), 1);
        assert_eq!(table.total_skipped(), 3);
    }

    #[test]
    fn missing_semicolon_fails_loudly() {
        let err = parse_symbol_addrs("foo = 0x80000000\n").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.reason.contains("';'"));
    }

    #[test]
    fn non_hex_address_fails_loudly() {
        let err = parse_symbol_addrs("foo = 12345;\n").unwrap_err();
        assert!(err.reason.contains("0x-prefixed"));
    }

    #[test]
    fn missing_equals_fails_loudly() {
        let err = parse_symbol_addrs("foo 0x80000000;\n").unwrap_err();
        assert!(err.reason.contains("assignment"));
    }

    #[test]
    fn unrecognized_comment_token_fails_loudly() {
        let err = parse_symbol_addrs("foo = 0x80000000; // not_an_attribute\n").unwrap_err();
        assert!(err.reason.contains("key:value"));
    }

    #[test]
    fn empty_name_fails_loudly() {
        let err = parse_symbol_addrs(" = 0x80000000;\n").unwrap_err();
        assert!(err.reason.contains("empty symbol name"));
    }

    // ---- Vendored-file exact-count assertions (grading-key integrity) ----
    //
    // These pin the parse of the real, license-verified Banjo-Kazooie
    // `symbol_addrs.us.v10.txt` (testdata/answer_keys/, commit
    // 1b2edf8bea686b6bfb6f35277606439991351a5b, CC0). A count drift here means
    // the vendored file changed or the parser regressed — either way it must
    // fail, since the file is a grading key.

    const BANJO_KEY: &str =
        include_str!("../testdata/answer_keys/banjo_kazooie.symbol_addrs.us.v10.txt");

    #[test]
    fn banjo_key_parses_with_exact_counts() {
        let table = parse_symbol_addrs(BANJO_KEY).unwrap();
        // 60 total rows, no blank/comment-only lines in this file.
        assert_eq!(table.row_count(), 60);
        assert_eq!(table.total_skipped(), 0);
        // 5 `D_*` data rows, 55 function rows.
        assert_eq!(table.data_count(), 5);
        assert_eq!(table.function_count(), 55);
        // The file carries no explicit `type:` attributes; every function is
        // inferred from name + code address.
        assert_eq!(table.explicit_function_count(), 0);
        assert_eq!(table.inferred_function_count(), 55);
        // A known function row.
        let os_rom_base = table
            .rows
            .iter()
            .find(|r| r.name == "osRomBase")
            .expect("osRomBase present");
        assert_eq!(os_rom_base.address, 0x8000_0308);
        // A known data row.
        assert!(table
            .rows
            .iter()
            .any(|r| r.name == "D_5E90" && r.address == 0x5E90));
    }

    #[test]
    fn banjo_vendored_bytes_match_recorded_digest() {
        // Owns the SHA-256 cited in testdata/answer_keys/LICENSES.md: the
        // vendored key is a grading input, so its bytes are pinned. A drift
        // here means the file was re-fetched or edited and its provenance
        // note must be updated in the same change.
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(BANJO_KEY.as_bytes()));
        assert_eq!(
            digest,
            "66ba957b7c6b4f8a58150456b3cf014447b11cc1abe4d631d6059bcc13f86420"
        );
    }
}
