//! Literal port of RT64's `ReplacementDatabase` pure string/hash helpers, a
//! literal port of the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/common/rt64_replacement_database.h`
//! / `.cpp` (SHA-256 of the whole files,
//! `045156fd1d53d6664e514e0ce3b6c5b32c0a731e72b60ccf355ff499e3255d30` /
//! `a66e4f73182d780dace2fccce5bff1011930bf5d9cc947bcb34d124c69ef7e85`):
//!
//! Only `checkWildcard`, `stringToHash`, and `hashToString` (both overloads)
//! are ported. `resolvePaths` (takes a `FileSystem*`), `resolveOperation(s)`
//! / `resolveShift(s)` (mutate/read `ReplacementDatabase` instance state
//! populated by JSON), `isExtensionKnown`/`endsWith`/`toLower`/
//! `removeKnownExtension` (path/extension helpers scoped to
//! `resolvePaths`'s filesystem walk, not this ticket's named
//! `checkWildcard`/`stringToHash`/`hashToString` cluster), all JSON
//! serialization (`to_json`/`from_json`, `nlohmann`), and
//! `ReplacementDatabase`'s stateful members (`addReplacement`,
//! `fixReplacement`, `getReplacement`, `buildHashMaps` -- all operate on the
//! `textures`/`tmemHashToReplaceMap` instance fields, not pure string/hash
//! algorithms) are excluded -- see "Nonclaims".
//!
//! ```text
//! // rt64_replacement_database.cpp
//! static bool checkWildcard(const std::string &str, const std::string &pat) {
//!     size_t strIndex = 0;
//!     size_t patIndex = 0;
//!     size_t wildcardIndex = std::string::npos;
//!     size_t matchIndex = std::string::npos;
//!     while (strIndex < str.size()) {
//!         // Characters match or pattern indicates any character here. Advance both cursors.
//!         if ((patIndex < pat.size()) && ((pat[patIndex] == '?') || (str[strIndex] == pat[patIndex]))) {
//!             strIndex++;
//!             patIndex++;
//!         }
//!         // Pattern indicates a wildcard. The match is accepted.
//!         else if ((patIndex < pat.size()) && (pat[patIndex] == '*')) {
//!             wildcardIndex = patIndex;
//!             matchIndex = strIndex;
//!             patIndex++;
//!         }
//!         // There's a match active.
//!         else if (wildcardIndex != std::string::npos) {
//!             assert(matchIndex != std::string::npos);
//!             matchIndex++;
//!             patIndex = wildcardIndex + 1;
//!             strIndex = matchIndex;
//!         }
//!         // It doesn't match and there's no wildcard, reject the match.
//!         else {
//!             return false;
//!         }
//!     }
//!
//!     // Check if the rest of the pattern consists of wildcards.
//!     while ((patIndex < pat.size()) && (pat[patIndex] == '*')) {
//!         patIndex++;
//!     }
//!
//!     // The match is accepted if we reached the end of the pattern.
//!     return (patIndex == pat.size());
//! }
//!
//! uint64_t ReplacementDatabase::stringToHash(const std::string &str) {
//!     return strtoull(str.c_str(), nullptr, 16);
//! }
//!
//! std::string ReplacementDatabase::hashToString(uint32_t hash) {
//!     char hexStr[32];
//!     snprintf(hexStr, sizeof(hexStr), "%08x", hash);
//!     return std::string(hexStr);
//! }
//!
//! std::string ReplacementDatabase::hashToString(uint64_t hash) {
//!     char hexStr[32];
//!     snprintf(hexStr, sizeof(hexStr), "%016" PRIx64, hash);
//!     return std::string(hexStr);
//! }
//! ```
//!
//! **Reuse, not new type.** These are free functions operating on borrowed
//! byte slices (`&[u8]`) and returning owned `Vec<u8>`/`u64` -- no new
//! string/hash wrapper type is introduced, matching `rt64_common.rs`'s and
//! `rt64_math.rs`'s precedent of using plain Rust primitives (`f32`, tuples,
//! arrays) in place of C++ value types that carry no invariants beyond their
//! own bytes.
//!
//! ## Admitted domain
//!
//! - **`stringToHash`'s hex parsing is `strtoull(str.c_str(), nullptr,
//!   16)`, not a strict/checked parse.** This is glibc/libc++'s C `strtoull`
//!   with base 16, which: (1) skips leading ASCII whitespace; (2) accepts an
//!   optional `+`/`-` sign (a negative input is parsed as its two's-
//!   complement-negated magnitude, mod 2^64 -- e.g. `"-1"` yields
//!   `u64::MAX`, confirmed against a compiled C++17 probe of this exact
//!   function); (3) accepts an optional `0x`/`0X` prefix before the hex
//!   digits (redundant with base 16, but accepted); (4) reads hex digits
//!   **case-insensitively** (`"ff"` and `"FF"` both yield `255`); (5) stops
//!   at the first non-hex-digit byte and ignores everything after it --
//!   trailing garbage such as `"1f_extra"` yields `31`, not an error; (6) on
//!   **no valid digits at all** (empty string, or a string with no
//!   hex-digit prefix such as `"xyz"`) returns **`0`**, not undefined
//!   behavior and not a sentinel -- a failed parse is indistinguishable from
//!   a genuine hash of `0`; (7) on **overflow** (more than 16 significant
//!   hex digits, e.g. `"10000000000000000"`, 17 digits) `strtoull`
//!   **saturates to `ULLONG_MAX`** (`u64::MAX`) per C standard `errno =
//!   ERANGE` semantics, it does not wrap or truncate. This port implements
//!   a hand-rolled equivalent (`parse_strtoull_hex`) rather than
//!   `u64::from_str_radix`, since Rust's `from_str_radix` rejects
//!   whitespace, signs, `0x` prefixes, trailing garbage, and empty/all-
//!   invalid input with `Err` instead of a graceful `0`/saturating
//!   fallback -- `from_str_radix` is a *stricter*, behaviorally different
//!   parser and would not be a literal port.
//! - **`hashToString`'s hex case and width are fixed by the `printf`
//!   format strings, not a general formatter.** `%08x` (u32 overload) and
//!   `%016" PRIx64` (u64 overload) both use **lowercase** `x` (never
//!   uppercase `X`) and zero-pad to a **fixed width** (8 hex digits for
//!   u32, 16 hex digits for u64) -- the output length never varies with the
//!   input value, unlike `stringToHash`'s parser, which accepts variable-
//!   length input. `hashToString(0)` (u32) is `"00000000"`;
//!   `hashToString(0)` (u64) is `"0000000000000000"`. There is no
//!   malformed-input case for `hashToString`: every `u32`/`u64` value
//!   formats successfully, and the 32-byte stack buffer (`char
//!   hexStr[32]`) is never close to overflowing (max 16 hex digits + NUL =
//!   17 bytes), so this is ported as an infallible function, matching the
//!   source's infallible `std::string` return.
//! - **Round-trip is not lossless across the pair in general.**
//!   `hashToString(stringToHash(s))` normalizes case, width, and strips
//!   leading zeros/garbage/whitespace -- it is not the identity function on
//!   arbitrary `s`. `stringToHash(hashToString(h))` **is** the identity on
//!   `h` for every `u64` value (since `hashToString`'s output is always a
//!   clean, non-overflowing, all-lowercase 16-hex-digit string that
//!   `stringToHash` parses back exactly) -- this direction is characterized
//!   below with `0`, `u64::MAX`, and leading-zero values.
//! - **`checkWildcard` is a greedy-with-backtracking single-pass
//!   automaton (the classic two-pointer `fnmatch`-style algorithm), not a
//!   regex engine.** Determined by hand-tracing the algorithm and
//!   cross-checked against a compiled C++17 probe of this exact function
//!   (not guessed, not captured from this Rust port):
//!   - **`*` matches zero or more of *any* character, including matching
//!     the empty string** (`checkWildcard("", "*")` is `true`); on a
//!     mismatch after committing to a wildcard, the algorithm **backtracks
//!     by advancing `matchIndex` by one and retrying** from just past the
//!     wildcard -- this is the standard greedy-then-backtrack matcher, so
//!     `*` behaves as if greedy, but produces the same true/false verdict a
//!     lazy matcher would (backtracking explores every split point, so
//!     **greediness only affects which split is chosen during matching,
//!     never whether a match exists** for this literal accept/reject
//!     boolean -- there's no capture output to make greedy-vs-lazy
//!     externally observable here).
//!   - **`?` matches exactly one arbitrary character** and is part of the
//!     same algorithm (`pat[patIndex] == '?'` is checked in the same
//!     branch as an exact-character match) -- ported faithfully even though
//!     no caller in the excluded `resolveOperation`/`resolveShift`/
//!     `resolvePaths` methods is included in this module; `checkWildcard`
//!     itself is the named port target and must preserve its full
//!     `?`-plus-`*` semantics, not just the `*`-only subset its current
//!     callers happen to exercise.
//!   - **Matching is case-sensitive** (`str[strIndex] == pat[patIndex]` is
//!     a raw byte/char comparison, no `tolower`) -- `checkWildcard("abc",
//!     "ABC")` is `false`.
//!   - **An empty pattern matches only an empty string**:
//!     `checkWildcard("", "")` is `true`; `checkWildcard("abc", "")` is
//!     `false` (the main loop's first iteration hits the `else` branch
//!     immediately, since `patIndex < pat.size()` is false and no wildcard
//!     is active yet).
//!   - **Multiple wildcards compose left-to-right**: each `*` encountered
//!     updates `wildcardIndex`/`matchIndex` to the *most recent* wildcard,
//!     so `"a*b*c"` against `"aXbXc"` and `"aXXXbXXXc"` both match --
//!     confirmed by probe. Consecutive `*`s (`"a**c"`) behave identically
//!     to a single `*`, since the second `*` immediately re-triggers the
//!     wildcard branch with the same `matchIndex`.
//!   - **Trailing `*`s after the input string is exhausted are consumed
//!     and do not cause a mismatch**: the loop after the main `while`
//!     (`while ((patIndex < pat.size()) && (pat[patIndex] == '*'))`) skips
//!     any run of trailing `*` pattern characters, so `checkWildcard("abc",
//!     "abc*")`-style patterns (any exact prefix match followed by
//!     trailing stars) succeed. A pattern with trailing **non-star**
//!     characters after the string is exhausted (e.g. `"abc"` against
//!     `"abcd"`... reversed as `"abc"` against pattern `"abcd"`) fails,
//!     since `patIndex` stops short of `pat.size()`.
//!   - **A literal-character mismatch with no wildcard ever having been
//!     seen rejects immediately** (`wildcardIndex == npos` falls to
//!     `return false`), matching `checkWildcard("xyz", "a*c")` being
//!     `false` here specifically because `'x' != 'a'` and no `*` precedes
//!     it in the pattern (not because of the trailing `*c` -- a different
//!     failure mode than a length mismatch).
//! - **`std::string`/byte-string vs Rust `String`/`&str`.** RT64's
//!   `checkWildcard` and `stringToHash` operate on `std::string`, which is
//!   a byte string with no UTF-8 validity requirement -- it may contain
//!   embedded NUL bytes (`std::string` is length-prefixed, not
//!   NUL-terminated internally, though `c_str()`/`stringToHash` *does*
//!   truncate at the first embedded NUL when handed to `strtoull`, since C
//!   strings are NUL-terminated) and arbitrary non-UTF-8 byte sequences
//!   (texture pack paths and wildcard filters are read from JSON/filesystem
//!   data of unspecified encoding upstream). This port therefore uses
//!   `&[u8]`/byte comparisons throughout (`check_wildcard(str: &[u8], pat:
//!   &[u8]) -> bool`, `string_to_hash(str: &[u8]) -> u64`) rather than Rust
//!   `String`/`&str`, since a `&str` parameter would silently assume
//!   UTF-8 validity that the C++ source neither requires nor checks --
//!   using `String` here would be an unrequested behavior narrowing (valid
//!   non-UTF-8 byte-string inputs the C++ accepts would become
//!   un-representable or require a lossy/panicking conversion at the call
//!   boundary). `hashToString`'s output is ASCII-only hex digits (`0-9`,
//!   `a-f`), which is always valid UTF-8, so `hash_to_string_u32`/
//!   `hash_to_string_u64` return `String` (a safe, lossless narrowing of
//!   the *output* side only, matching `rt64_common.rs`'s and
//!   `rt64_math.rs`'s precedent of using the most literal safe Rust type
//!   for values with no representable-but-invalid range).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_common.rs`'s and `rt64_math.rs`'s
//! precedent), and no RT64 visual/pixel/silicon parity or performance
//! claim. Deliberately not ported from `rt64_replacement_database.h`/
//! `.cpp`:
//!
//! - **`resolvePaths`** (takes a `const FileSystem *fileSystem` parameter
//!   and calls `fileSystem->makeCanonical`/`->exists`/iterates
//!   `*fileSystem` -- a filesystem I/O dependency, explicitly excluded by
//!   the ticket).
//! - **All JSON serialization**: `to_json`/`from_json` for
//!   `ReplacementConfiguration`, `ReplacementHashes`, `ReplacementTexture`,
//!   `ReplacementOperationFilter`, `ReplacementShiftFilter`, and
//!   `ReplacementDatabase` (all depend on `nlohmann::json`, explicitly
//!   excluded by the ticket).
//! - **Anything touching the filesystem** -- there is no filesystem access
//!   anywhere in this module.
//! - **`ReplacementDatabase::addReplacement`/`fixReplacement`/
//!   `getReplacement`/`buildHashMaps`**: these mutate or read the
//!   `textures`/`tmemHashToReplaceMap` instance fields of a live
//!   `ReplacementDatabase` (populated only via the excluded JSON path) --
//!   not pure string/hash algorithms, and out of this module's named scope
//!   (`checkWildcard`, `stringToHash`, `hashToString`, "sibling pure
//!   helpers").
//! - **`ReplacementDatabase::resolveOperation`/`resolveShift`/
//!   `resolveOperations`/`resolveShifts`**: these read `this->config` and
//!   `this->operationFilters`/`this->shiftFilters` (instance state) and
//!   call `checkWildcard` internally, but are themselves *not* pure
//!   functions of their arguments -- they're instance methods over
//!   JSON-populated state, and `resolveOperations`/`resolveShifts` also
//!   mutate a caller-supplied `resolvedPathMap` in place. `checkWildcard`
//!   itself (the pure primitive they call) is ported; the stateful
//!   resolution wrappers are not.
//! - **`isExtensionKnown`/`endsWith`/`toLower`/`removeKnownExtension`**:
//!   pure string helpers, but scoped specifically to `resolvePaths`'s
//!   filesystem-extension-matching logic (`ReplacementKnownExtensions`,
//!   `.dds`/`.png` handling) -- not part of the `checkWildcard`/
//!   `stringToHash`/`hashToString` cluster this ticket names, and every
//!   caller of them in the source is inside the excluded `resolvePaths`.
//! - **The `ReplacementConfiguration`/`ReplacementHashes`/
//!   `ReplacementTexture`/`ReplacementFilter`/`ReplacementOperationFilter`/
//!   `ReplacementShiftFilter`/`ReplacementResolvedPath`/
//!   `ReplacementMipmapCacheHeader`/`ReplacementDatabase` structs**: all
//!   are data-holding types whose only behavior is the JSON (de)serializers
//!   and the stateful methods above, both excluded.
//! - **`ReplacementOperation`/`ReplacementShift`/`ReplacementAutoPath`
//!   enums** and their `NLOHMANN_JSON_SERIALIZE_ENUM` mappings (JSON-only).
//! - **The free-standing constants** (`ReplacementDatabaseFilename`,
//!   `ReplacementLowMipCacheFilename`, `ReplacementPackExtension`,
//!   `ReplacementKnownExtensions`, `ReplacementMipmapCacheHeaderMagic`,
//!   `ReplacementMipmapCacheHeaderVersion`) -- filesystem/format constants
//!   with no behavior to characterize, and only consumed by the excluded
//!   `resolvePaths`/JSON paths.

/// `checkWildcard(str, pat)`: greedy-with-backtracking glob match. `?`
/// matches exactly one byte; `*` matches zero or more bytes (see module doc
/// "Admitted domain" for the full characterization). Byte-string
/// (`&[u8]`), not `&str` -- see module doc's UTF-8-vs-bytes justification.
pub fn check_wildcard(str: &[u8], pat: &[u8]) -> bool {
    let mut str_index: usize = 0;
    let mut pat_index: usize = 0;
    // `None` stands in for C++'s `std::string::npos` sentinel.
    let mut wildcard_index: Option<usize> = None;
    let mut match_index: Option<usize> = None;

    while str_index < str.len() {
        if pat_index < pat.len() && (pat[pat_index] == b'?' || str[str_index] == pat[pat_index]) {
            str_index += 1;
            pat_index += 1;
        } else if pat_index < pat.len() && pat[pat_index] == b'*' {
            wildcard_index = Some(pat_index);
            match_index = Some(str_index);
            pat_index += 1;
        } else if let Some(w) = wildcard_index {
            let m = match_index.expect("matchIndex set whenever wildcardIndex is set");
            let m = m + 1;
            match_index = Some(m);
            pat_index = w + 1;
            str_index = m;
        } else {
            return false;
        }
    }

    while pat_index < pat.len() && pat[pat_index] == b'*' {
        pat_index += 1;
    }

    pat_index == pat.len()
}

/// `ReplacementDatabase::stringToHash(str)`: `strtoull(str.c_str(),
/// nullptr, 16)`. See module doc "Admitted domain" for the full
/// whitespace/sign/prefix/overflow/malformed-input characterization.
/// Byte-string input (`&[u8]`) -- see module doc's UTF-8-vs-bytes
/// justification.
pub fn string_to_hash(str: &[u8]) -> u64 {
    parse_strtoull_hex(str)
}

/// `ReplacementDatabase::hashToString(uint32_t)`: `%08x` -- lowercase,
/// zero-padded to 8 hex digits.
pub fn hash_to_string_u32(hash: u32) -> String {
    format!("{hash:08x}")
}

/// `ReplacementDatabase::hashToString(uint64_t)`: `%016" PRIx64`  --
/// lowercase, zero-padded to 16 hex digits.
pub fn hash_to_string_u64(hash: u64) -> String {
    format!("{hash:016x}")
}

/// Hand-rolled equivalent of glibc/libc++'s `strtoull(s, nullptr, 16)`:
/// skip leading ASCII whitespace, accept an optional `+`/`-` sign, accept
/// an optional `0x`/`0X` prefix, then parse hex digits case-insensitively
/// until the first non-hex-digit byte (or end of input). Zero valid digits
/// (empty input, or no hex-digit prefix) yields `0`. Overflow past `u64`
/// saturates to `u64::MAX`. A `-` sign negates the parsed magnitude modulo
/// 2^64 (two's-complement wraparound), matching `strtoull`'s documented
/// behavior for a negative input on an unsigned parse.
fn parse_strtoull_hex(input: &[u8]) -> u64 {
    let mut i = 0usize;
    let len = input.len();

    // (1) Skip leading ASCII whitespace (matches C `isspace` for the
    // space/tab/newline/vtab/formfeed/CR set that `strtoull` skips).
    while i < len && input[i].is_ascii_whitespace() {
        i += 1;
    }

    // (2) Optional sign.
    let mut negative = false;
    if i < len && (input[i] == b'+' || input[i] == b'-') {
        negative = input[i] == b'-';
        i += 1;
    }

    // (3) Optional 0x/0X prefix (redundant with base 16, but accepted).
    if i + 1 < len && input[i] == b'0' && (input[i + 1] == b'x' || input[i + 1] == b'X') {
        i += 2;
    }

    // (4)-(7) Parse hex digits case-insensitively, saturating on overflow.
    // Zero digits consumed yields 0 (a failed parse is indistinguishable
    // from a genuine zero -- see module doc "Admitted domain").
    let mut value: u64 = 0;
    let mut any_digits = false;
    let mut overflowed = false;
    while i < len {
        let digit = match input[i] {
            b'0'..=b'9' => input[i] - b'0',
            b'a'..=b'f' => input[i] - b'a' + 10,
            b'A'..=b'F' => input[i] - b'A' + 10,
            _ => break,
        };
        any_digits = true;
        match value
            .checked_mul(16)
            .and_then(|v| v.checked_add(digit as u64))
        {
            Some(v) => value = v,
            None => overflowed = true,
        }
        i += 1;
    }

    if !any_digits {
        return 0;
    }

    if overflowed {
        return u64::MAX;
    }

    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- check_wildcard: no-wildcard exact match ---

    #[test]
    fn check_wildcard_empty_str_empty_pat_matches() {
        assert!(check_wildcard(b"", b""));
    }

    #[test]
    fn check_wildcard_nonempty_str_empty_pat_fails() {
        assert!(!check_wildcard(b"abc", b""));
    }

    #[test]
    fn check_wildcard_empty_str_nonempty_pat_fails() {
        assert!(!check_wildcard(b"", b"abc"));
    }

    #[test]
    fn check_wildcard_exact_match_succeeds() {
        assert!(check_wildcard(b"abc", b"abc"));
    }

    #[test]
    fn check_wildcard_exact_mismatch_fails() {
        assert!(!check_wildcard(b"abc", b"abd"));
    }

    #[test]
    fn check_wildcard_length_mismatch_no_wildcard_fails() {
        assert!(!check_wildcard(b"ab", b"abc"));
        assert!(!check_wildcard(b"abc", b"ab"));
    }

    // --- case sensitivity ---

    #[test]
    fn check_wildcard_is_case_sensitive() {
        assert!(!check_wildcard(b"abc", b"ABC"));
        assert!(check_wildcard(b"ABC", b"ABC"));
    }

    #[test]
    fn check_wildcard_case_sensitive_with_wildcard() {
        assert!(!check_wildcard(b"textures/foo.png", b"*.PNG"));
        assert!(check_wildcard(b"textures/foo.png", b"*.png"));
    }

    // --- leading wildcard ---

    #[test]
    fn check_wildcard_leading_star_matches_suffix() {
        assert!(check_wildcard(b"abc", b"*c"));
    }

    #[test]
    fn check_wildcard_leading_star_no_matching_suffix_fails() {
        assert!(!check_wildcard(b"abc", b"*d"));
    }

    #[test]
    fn check_wildcard_leading_star_glob_suffix_pattern() {
        assert!(check_wildcard(b"abcabcabc", b"*abc"));
    }

    // --- trailing wildcard ---

    #[test]
    fn check_wildcard_trailing_star_matches_prefix() {
        assert!(check_wildcard(b"abc", b"a*"));
    }

    #[test]
    fn check_wildcard_trailing_star_matches_whole_string_when_prefix_is_empty() {
        assert!(check_wildcard(b"abc", b"*"));
    }

    #[test]
    fn check_wildcard_trailing_star_no_matching_prefix_fails() {
        assert!(!check_wildcard(b"abc", b"d*"));
    }

    #[test]
    fn check_wildcard_star_alone_matches_empty_string() {
        assert!(check_wildcard(b"", b"*"));
    }

    #[test]
    fn check_wildcard_star_alone_matches_nonempty_string() {
        assert!(check_wildcard(b"anything at all", b"*"));
    }

    // --- middle wildcard ---

    #[test]
    fn check_wildcard_middle_star_matches() {
        assert!(check_wildcard(b"abc", b"a*c"));
    }

    #[test]
    fn check_wildcard_middle_star_wrong_prefix_fails() {
        assert!(!check_wildcard(b"xyz", b"a*c"));
    }

    #[test]
    fn check_wildcard_middle_star_wrong_suffix_fails() {
        assert!(!check_wildcard(b"abd", b"a*c"));
    }

    #[test]
    fn check_wildcard_middle_star_matches_empty_gap() {
        assert!(check_wildcard(b"ac", b"a*c"));
    }

    #[test]
    fn check_wildcard_middle_star_matches_long_gap() {
        assert!(check_wildcard(b"aXXXXXXXXc", b"a*c"));
    }

    #[test]
    fn check_wildcard_star_matches_repeated_prefix_content() {
        // Backtracking must retry past a false-start match of the
        // wildcard's suffix inside the wildcard span.
        assert!(check_wildcard(b"aaa", b"a*a"));
    }

    // --- multiple wildcards ---

    #[test]
    fn check_wildcard_two_stars_compose() {
        assert!(check_wildcard(b"abc", b"a*b*c"));
        assert!(check_wildcard(b"aXbXc", b"a*b*c"));
        assert!(check_wildcard(b"aXXXbXXXc", b"a*b*c"));
    }

    #[test]
    fn check_wildcard_two_stars_missing_middle_anchor_fails() {
        assert!(!check_wildcard(b"ac", b"a*b*c"));
    }

    #[test]
    fn check_wildcard_consecutive_stars_behave_as_one() {
        assert!(check_wildcard(b"abc", b"a**c"));
        assert!(check_wildcard(b"abc", b"**"));
    }

    #[test]
    fn check_wildcard_bracketing_stars() {
        assert!(check_wildcard(b"abc", b"*a*b*c*"));
        assert!(check_wildcard(b"XaXbXcX", b"*a*b*c*"));
    }

    #[test]
    fn check_wildcard_trailing_stars_after_string_exhausted_are_consumed() {
        assert!(check_wildcard(b"abc", b"abc*"));
        assert!(check_wildcard(b"abc", b"abc**"));
    }

    // --- '?' single-character wildcard (part of the same algorithm) ---

    #[test]
    fn check_wildcard_question_mark_matches_single_char() {
        assert!(check_wildcard(b"a", b"?"));
        assert!(check_wildcard(b"abc", b"a?c"));
        assert!(check_wildcard(b"abc", b"??c"));
        assert!(check_wildcard(b"abc", b"???"));
    }

    #[test]
    fn check_wildcard_question_mark_does_not_match_empty() {
        assert!(!check_wildcard(b"", b"?"));
    }

    #[test]
    fn check_wildcard_question_mark_wrong_length_fails() {
        assert!(!check_wildcard(b"abc", b"????"));
    }

    #[test]
    fn check_wildcard_question_mark_and_star_combine() {
        assert!(check_wildcard(b"abcdef", b"a?*f"));
    }

    // --- non-UTF-8 byte strings ---

    #[test]
    fn check_wildcard_matches_non_utf8_bytes() {
        let s: &[u8] = &[0xFF, 0xFE, b'x'];
        let p: &[u8] = &[0xFF, b'*'];
        assert!(check_wildcard(s, p));
    }

    #[test]
    fn check_wildcard_matches_embedded_nul() {
        let s: &[u8] = b"a\0b";
        let p: &[u8] = b"a\0b";
        assert!(check_wildcard(s, p));
        let p2: &[u8] = b"a*b";
        assert!(check_wildcard(s, p2));
    }

    // --- string_to_hash: base cases ---

    #[test]
    fn string_to_hash_empty_is_zero() {
        assert_eq!(string_to_hash(b""), 0);
    }

    #[test]
    fn string_to_hash_zero_is_zero() {
        assert_eq!(string_to_hash(b"0"), 0);
    }

    #[test]
    fn string_to_hash_lowercase_hex() {
        assert_eq!(string_to_hash(b"ff"), 0xff);
    }

    #[test]
    fn string_to_hash_uppercase_hex_is_case_insensitive() {
        assert_eq!(string_to_hash(b"FF"), 0xff);
    }

    #[test]
    fn string_to_hash_mixed_case_hex() {
        assert_eq!(string_to_hash(b"aAbBcC"), 0x00AA_BBCC);
    }

    #[test]
    fn string_to_hash_max_u64_all_f() {
        assert_eq!(string_to_hash(b"ffffffffffffffff"), u64::MAX);
    }

    #[test]
    fn string_to_hash_leading_zeros_in_hex() {
        assert_eq!(string_to_hash(b"0000000000000001"), 1);
        assert_eq!(string_to_hash(b"007"), 7);
        assert_eq!(string_to_hash(b"00000000"), 0);
    }

    #[test]
    fn string_to_hash_typical_16_digit_value() {
        assert_eq!(string_to_hash(b"0123456789abcdef"), 0x0123_4567_89ab_cdef);
    }

    // --- string_to_hash: malformed / short / overlong input ---

    #[test]
    fn string_to_hash_malformed_no_hex_digits_is_zero() {
        // A failed parse is 0, not undefined -- see module doc.
        assert_eq!(string_to_hash(b"xyz"), 0);
        assert_eq!(string_to_hash(b"!!!"), 0);
    }

    #[test]
    fn string_to_hash_short_input_parses_partial_value() {
        assert_eq!(string_to_hash(b"a"), 0xa);
        assert_eq!(string_to_hash(b"1"), 1);
    }

    #[test]
    fn string_to_hash_overlong_input_saturates_to_u64_max() {
        // 17 hex digits: one beyond u64's 16-digit capacity.
        assert_eq!(string_to_hash(b"10000000000000000"), u64::MAX);
        assert_eq!(string_to_hash(b"fffffffffffffffff"), u64::MAX);
    }

    #[test]
    fn string_to_hash_trailing_garbage_stops_at_first_non_hex_byte() {
        assert_eq!(string_to_hash(b"1f_extra"), 0x1f);
        assert_eq!(string_to_hash(b"ff.png"), 0xff);
    }

    #[test]
    fn string_to_hash_leading_whitespace_is_skipped() {
        assert_eq!(string_to_hash(b"  ff"), 0xff);
        assert_eq!(string_to_hash(b"\t\nff"), 0xff);
    }

    #[test]
    fn string_to_hash_optional_0x_prefix_is_accepted() {
        assert_eq!(string_to_hash(b"0x1f"), 0x1f);
        assert_eq!(string_to_hash(b"0X1f"), 0x1f);
    }

    #[test]
    fn string_to_hash_plus_sign_is_accepted() {
        assert_eq!(string_to_hash(b"+1"), 1);
    }

    #[test]
    fn string_to_hash_minus_sign_negates_modulo_2_pow_64() {
        assert_eq!(string_to_hash(b"-1"), u64::MAX);
        assert_eq!(string_to_hash(b"-2"), u64::MAX - 1);
    }

    #[test]
    fn string_to_hash_lone_sign_no_digits_is_zero() {
        assert_eq!(string_to_hash(b"-"), 0);
        assert_eq!(string_to_hash(b"+"), 0);
    }

    #[test]
    fn string_to_hash_lone_0x_no_digits_is_zero() {
        assert_eq!(string_to_hash(b"0x"), 0);
    }

    #[test]
    fn string_to_hash_non_utf8_bytes_before_garbage_still_stop_cleanly() {
        let s: &[u8] = &[b'1', b'f', 0xFF, 0xFE];
        assert_eq!(string_to_hash(s), 0x1f);
    }

    // --- hash_to_string_u32 ---

    #[test]
    fn hash_to_string_u32_zero() {
        assert_eq!(hash_to_string_u32(0), "00000000");
    }

    #[test]
    fn hash_to_string_u32_max() {
        assert_eq!(hash_to_string_u32(u32::MAX), "ffffffff");
    }

    #[test]
    fn hash_to_string_u32_is_lowercase() {
        assert_eq!(hash_to_string_u32(0xABCDEF), "00abcdef");
        assert!(!hash_to_string_u32(0xABCDEF).contains(|c: char| c.is_ascii_uppercase()));
    }

    #[test]
    fn hash_to_string_u32_zero_pads_to_eight_digits() {
        let s = hash_to_string_u32(0xff);
        assert_eq!(s.len(), 8);
        assert_eq!(s, "000000ff");
    }

    #[test]
    fn hash_to_string_u32_leading_zeros_preserved_in_output() {
        assert_eq!(hash_to_string_u32(1), "00000001");
    }

    // --- hash_to_string_u64 ---

    #[test]
    fn hash_to_string_u64_zero() {
        assert_eq!(hash_to_string_u64(0), "0000000000000000");
    }

    #[test]
    fn hash_to_string_u64_max() {
        assert_eq!(hash_to_string_u64(u64::MAX), "ffffffffffffffff");
    }

    #[test]
    fn hash_to_string_u64_is_lowercase() {
        let s = hash_to_string_u64(0xDEAD_BEEF_CAFE_F00D);
        assert_eq!(s, "deadbeefcafef00d");
        assert!(!s.contains(|c: char| c.is_ascii_uppercase()));
    }

    #[test]
    fn hash_to_string_u64_zero_pads_to_sixteen_digits() {
        let s = hash_to_string_u64(0xff);
        assert_eq!(s.len(), 16);
        assert_eq!(s, "00000000000000ff");
    }

    #[test]
    fn hash_to_string_u64_leading_zeros_preserved_in_output() {
        assert_eq!(hash_to_string_u64(1), "0000000000000001");
    }

    // --- round trips ---

    #[test]
    fn round_trip_string_to_hash_of_hash_to_string_u64_is_identity_zero() {
        assert_eq!(string_to_hash(hash_to_string_u64(0).as_bytes()), 0);
    }

    #[test]
    fn round_trip_string_to_hash_of_hash_to_string_u64_is_identity_max() {
        assert_eq!(
            string_to_hash(hash_to_string_u64(u64::MAX).as_bytes()),
            u64::MAX
        );
    }

    #[test]
    fn round_trip_string_to_hash_of_hash_to_string_u64_is_identity_leading_zeros() {
        let h: u64 = 0x0000_0000_0000_00AB;
        assert_eq!(string_to_hash(hash_to_string_u64(h).as_bytes()), h);
    }

    #[test]
    fn round_trip_string_to_hash_of_hash_to_string_u64_is_identity_arbitrary() {
        let h: u64 = 0x0123_4567_89AB_CDEF;
        assert_eq!(string_to_hash(hash_to_string_u64(h).as_bytes()), h);
    }

    #[test]
    fn round_trip_hash_to_string_of_string_to_hash_normalizes_case_and_width() {
        // Not the identity function: uppercase/short input normalizes to
        // lowercase/16-digit output.
        assert_eq!(
            hash_to_string_u64(string_to_hash(b"FF")),
            "00000000000000ff"
        );
    }
}
