//! Build-time preparation shared by every generated-C consumer.
//!
//! N64Recomp emits C, while fn64 compiles those translation units as C++ so
//! `fn64_mmio_proxy.h` can preserve `MEM_W(...)` lvalue syntax. C permits a
//! `goto` to cross an initialized scalar declaration; C++ rejects it. The
//! indirect-jump snapshot is the one generated declaration with that shape,
//! so split only that exact declaration into an uninitialized declaration and
//! an assignment at the same program point. The same content-preserving pass
//! inserts fn64's destination observer as the first statement of every
//! generated `RECOMP_FUNC` body. That in-body boundary sees direct C-to-C
//! calls as well as `LOOKUP_FUNC` calls; lookup itself is not misreported as
//! execution. A caller with a proven bad function partition may explicitly
//! enable address-contiguous fall-through repair from the generated section
//! tables; ordinary corpora never receive that transform implicitly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn normalize_jump_snapshots(source: &str) -> (String, usize) {
    let mut output = String::with_capacity(source.len());
    let mut rewrite_count = 0;

    for segment in source.split_inclusive('\n') {
        let (line_with_optional_cr, newline) = segment
            .strip_suffix('\n')
            .map_or((segment, ""), |line| (line, "\n"));
        let (line, carriage_return) = line_with_optional_cr
            .strip_suffix('\r')
            .map_or((line_with_optional_cr, ""), |line| (line, "\r"));
        let indentation_len = line.len() - line.trim_start_matches([' ', '\t']).len();
        let (indentation, statement) = line.split_at(indentation_len);

        let Some(rest) = statement.strip_prefix("gpr jr_addend_") else {
            output.push_str(segment);
            continue;
        };
        let Some(rest) = rest.strip_suffix(';') else {
            output.push_str(segment);
            continue;
        };
        let Some((address, value)) = rest.split_once(" = ") else {
            output.push_str(segment);
            continue;
        };
        if address.is_empty()
            || !address.bytes().all(|byte| byte.is_ascii_hexdigit())
            || value.is_empty()
        {
            output.push_str(segment);
            continue;
        }

        let separator = if newline.is_empty() { "\n" } else { newline };
        output.push_str(indentation);
        output.push_str("gpr jr_addend_");
        output.push_str(address);
        output.push(';');
        output.push_str(carriage_return);
        output.push_str(separator);
        output.push_str(indentation);
        output.push_str("jr_addend_");
        output.push_str(address);
        output.push_str(" = ");
        output.push_str(value);
        output.push(';');
        output.push_str(carriage_return);
        output.push_str(newline);
        rewrite_count += 1;
    }

    (output, rewrite_count)
}

fn recomp_names_followed_by_paren(source: &str) -> BTreeSet<String> {
    let bytes = source.as_bytes();
    let mut names = BTreeSet::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            let name = &source[start..index];
            let mut next = index;
            while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                next += 1;
            }
            if name.ends_with("_recomp") && bytes.get(next) == Some(&b'(') {
                names.insert(name.to_owned());
            }
        } else {
            index += 1;
        }
    }
    names
}

fn generated_function_definitions(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("RECOMP_FUNC void ")?;
            let name = rest.split_once('(')?.0.trim();
            name.ends_with("_recomp").then(|| name.to_owned())
        })
        .collect()
}

fn instrument_generated_function_entries(source: &str) -> (String, usize) {
    let mut output = String::with_capacity(source.len());
    let mut instrumented = 0;

    for segment in source.split_inclusive('\n') {
        let (line_with_optional_cr, newline) = segment
            .strip_suffix('\n')
            .map_or((segment, ""), |line| (line, "\n"));
        let (line, carriage_return) = line_with_optional_cr
            .strip_suffix('\r')
            .map_or((line_with_optional_cr, ""), |line| (line, "\r"));
        let indentation_len = line.len() - line.trim_start_matches([' ', '\t']).len();
        let (indentation, statement) = line.split_at(indentation_len);
        let Some(rest) = statement.strip_prefix("RECOMP_FUNC void ") else {
            output.push_str(segment);
            continue;
        };
        let Some((name, _)) = rest.split_once('(') else {
            output.push_str(segment);
            continue;
        };
        let name = name.trim();
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "generated RECOMP_FUNC has invalid identifier {name:?}"
        );
        let brace = statement.find('{').unwrap_or_else(|| {
            panic!(
                "generated RECOMP_FUNC {name} no longer opens its body on the definition line; update the entry instrumentation parser"
            )
        });
        assert!(
            statement[brace + 1..].trim().is_empty(),
            "generated RECOMP_FUNC {name} has code on its opening-brace line; cannot insert the entry observer first"
        );

        output.push_str(line);
        output.push_str(carriage_return);
        output.push_str(if newline.is_empty() { "\n" } else { newline });
        output.push_str(indentation);
        output.push_str("    fn64_c_recompiled_function_enter(");
        output.push_str(name);
        output.push_str(");");
        output.push_str(carriage_return);
        output.push_str(newline);
        instrumented += 1;
    }

    (output, instrumented)
}

/// Parse N64Recomp's generated section tables into address-contiguous
/// successor pairs. Section boundaries reset the predecessor so a repair can
/// never cross independently loaded code banks.
fn fallthrough_successors(inl_source: &str) -> BTreeMap<String, String> {
    let mut successors = BTreeMap::new();
    let mut previous: Option<(String, u64, u64)> = None;
    for line in inl_source.lines() {
        let line = line.trim();
        if line.starts_with("static FuncEntry ") {
            previous = None;
            continue;
        }
        let Some(rest) = line.strip_prefix("{ .func = ") else {
            continue;
        };
        let parse_hex = |key: &str| -> Option<u64> {
            let value = rest.split(key).nth(1)?.trim_start().strip_prefix("0x")?;
            let end = value
                .find(|character: char| !character.is_ascii_hexdigit())
                .unwrap_or(value.len());
            u64::from_str_radix(&value[..end], 16).ok()
        };
        let Some(name) = rest.split(',').next() else {
            continue;
        };
        let name = name.trim().to_owned();
        let (Some(offset), Some(rom_size)) = (parse_hex(".offset = "), parse_hex(".rom_size = "))
        else {
            continue;
        };
        if let Some((previous_name, previous_offset, previous_size)) = previous.take() {
            if previous_offset
                .checked_add(previous_size)
                .is_some_and(|end| end == offset)
            {
                successors.insert(previous_name, name.clone());
            }
        }
        previous = Some((name, offset, rom_size));
    }
    successors
}

/// Restore hardware fall-through only for generated fragments whose section
/// metadata proves an address-contiguous successor and whose emitted body can
/// actually reach its closing brace. Normal functions end in an explicit
/// generated `return`, `goto`, or `switch_error` and remain untouched.
fn mend_proven_fallthroughs(
    source: &str,
    successors: &BTreeMap<String, String>,
    file_name: &str,
) -> (String, usize) {
    let mut output = String::with_capacity(source.len());
    let mut count = 0;
    let mut rest = source;
    const FUNCTION_PREFIX: &str = "RECOMP_FUNC void ";

    while let Some(start) = rest.find(FUNCTION_PREFIX) {
        let after_prefix = &rest[start + FUNCTION_PREFIX.len()..];
        let name_end = after_prefix
            .find('(')
            .unwrap_or_else(|| panic!("{file_name}: malformed RECOMP_FUNC header"));
        let name = &after_prefix[..name_end];
        let close_relative = rest[start..]
            .find("\n;}")
            .unwrap_or_else(|| panic!("{file_name}: {name}: no `;}}` body close found"));
        let body_end = start + close_relative;
        let body = &rest[start..body_end];
        let can_fall_through = body
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("//"))
            .is_none_or(|line| {
                line != "return;"
                    && !line.starts_with("goto ")
                    && !line.starts_with("switch_error(")
            });

        output.push_str(&rest[..body_end]);
        if can_fall_through {
            if let Some(successor) = successors.get(name) {
                output.push_str(
                    "\n    // fn64: address-proven generated-fragment fall-through.\n    ",
                );
                output.push_str(successor);
                output.push_str("(rdram, ctx);");
                count += 1;
            }
        }
        output.push_str("\n;}");
        rest = &rest[body_end + 3..];
    }
    output.push_str(rest);
    (output, count)
}

fn missing_prototype_prelude(names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        return String::new();
    }

    let mut prelude = String::from(
        "// fn64: prototypes required when generated C is compiled as C++.\n\
         #include \"recomp.h\"\n\
         extern \"C\" {\n",
    );
    for name in names {
        prelude.push_str("void ");
        prelude.push_str(name);
        prelude.push_str("(uint8_t* rdram, recomp_context* ctx);\n");
    }
    prelude.push_str("}\n");
    prelude
}

/// Copy generated C into `OUT_DIR` as C++ translation units, applying the
/// syntax normalization required by fn64's MMIO lvalue proxy and inserting
/// the first-in-body native execution observer.
pub fn prepare_recompiled_cxx_sources(
    recompiled_dir: &Path,
    out_dir: &Path,
) -> (Vec<PathBuf>, usize, usize) {
    let (paths, rewrites, prototypes, fallthroughs) =
        prepare_recompiled_cxx_sources_inner(recompiled_dir, out_dir, false);
    assert_eq!(fallthroughs, 0);
    (paths, rewrites, prototypes)
}

/// Prepare generated C while repairing address-proven fragments that the
/// supplied N64Recomp corpus split at an internal fall-through label.
///
/// This is deliberately opt-in: a corpus without evidence of bad partition
/// boundaries must not silently acquire calls between merely adjacent normal
/// functions.
pub fn prepare_recompiled_cxx_sources_with_fallthrough_repair(
    recompiled_dir: &Path,
    out_dir: &Path,
) -> (Vec<PathBuf>, usize, usize, usize) {
    prepare_recompiled_cxx_sources_inner(recompiled_dir, out_dir, true)
}

fn prepare_recompiled_cxx_sources_inner(
    recompiled_dir: &Path,
    out_dir: &Path,
    repair_fallthroughs: bool,
) -> (Vec<PathBuf>, usize, usize, usize) {
    let mut source_paths: Vec<_> = std::fs::read_dir(recompiled_dir)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read RECOMPILED_DIR={}: {error}",
                recompiled_dir.display()
            )
        })
        .map(|entry| {
            entry
                .expect("failed to read generated source directory entry")
                .path()
        })
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("c"))
        .collect();
    source_paths.sort();
    assert!(
        !source_paths.is_empty(),
        "found zero .c files in RECOMPILED_DIR={} -- expected N64Recomp's generated RecompiledFuncs/*.c output",
        recompiled_dir.display()
    );

    let prepared_dir = out_dir.join("fn64-recompiled-cxx");
    std::fs::create_dir_all(&prepared_dir).unwrap_or_else(|error| {
        panic!(
            "failed to create generated-C preparation directory {}: {error}",
            prepared_dir.display()
        )
    });

    let funcs_header_path = recompiled_dir.join("funcs.h");
    let funcs_header = std::fs::read_to_string(&funcs_header_path).unwrap_or_else(|error| {
        panic!(
            "failed to read generated declarations {}: {error}",
            funcs_header_path.display()
        )
    });
    let successors = if repair_fallthroughs {
        let table_path = recompiled_dir.join("recomp_overlays.inl");
        let tables = std::fs::read_to_string(&table_path).unwrap_or_else(|error| {
            panic!(
                "failed to read generated section tables {}: {error}",
                table_path.display()
            )
        });
        let successors = fallthrough_successors(&tables);
        assert!(
            !successors.is_empty(),
            "generated section tables contain no address-contiguous successor pairs; refusing a silent fall-through repair"
        );
        successors
    } else {
        BTreeMap::new()
    };

    let mut declared_names = recomp_names_followed_by_paren(&funcs_header);
    let mut called_names = BTreeSet::new();
    let sources: Vec<_> = source_paths
        .into_iter()
        .map(|source_path| {
            let source = std::fs::read_to_string(&source_path).unwrap_or_else(|error| {
                panic!(
                    "failed to read generated source {}: {error}",
                    source_path.display()
                )
            });
            declared_names.extend(generated_function_definitions(&source));
            called_names.extend(recomp_names_followed_by_paren(&source));
            (source_path, source)
        })
        .collect();
    called_names.extend(successors.values().cloned());
    let missing_names: BTreeSet<_> = called_names.difference(&declared_names).cloned().collect();
    let prototype_prelude = missing_prototype_prelude(&missing_names);

    let mut prepared_paths = Vec::with_capacity(sources.len());
    let mut rewrite_count = 0;
    let mut fallthrough_count = 0;
    for (source_path, source) in sources {
        let (normalized, file_rewrite_count) = normalize_jump_snapshots(&source);
        let file_name = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("generated C source filename must be valid Unicode");
        let (normalized, file_fallthrough_count) =
            mend_proven_fallthroughs(&normalized, &successors, file_name);
        let (prepared, _) = instrument_generated_function_entries(&normalized);
        rewrite_count += file_rewrite_count;
        fallthrough_count += file_fallthrough_count;

        let file_stem = source_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("generated C source filename must be valid Unicode");
        let prepared_path = prepared_dir.join(format!("{file_stem}.cpp"));
        std::fs::write(&prepared_path, format!("{prototype_prelude}{prepared}")).unwrap_or_else(
            |error| {
                panic!(
                    "failed to write prepared generated source {}: {error}",
                    prepared_path.display()
                )
            },
        );
        prepared_paths.push(prepared_path);
    }

    (
        prepared_paths,
        rewrite_count,
        missing_names.len(),
        fallthrough_count,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_only_exact_indirect_jump_snapshot_declarations() {
        let source = concat!(
            "    goto L_80B5B4B8;\n",
            "    gpr jr_addend_80B5B1F8 = ctx->r10;\n",
            "L_80B5B4B8:\n",
        );

        let (normalized, rewrite_count) = normalize_jump_snapshots(source);

        assert_eq!(rewrite_count, 1);
        assert_eq!(
            normalized,
            concat!(
                "    goto L_80B5B4B8;\n",
                "    gpr jr_addend_80B5B1F8;\n",
                "    jr_addend_80B5B1F8 = ctx->r10;\n",
                "L_80B5B4B8:\n",
            )
        );
    }

    #[test]
    fn leaves_other_declarations_and_near_matches_unchanged() {
        let source = concat!(
            "    uint64_t hi = 0;\n",
            "    gpr ordinary = ctx->r10;\n",
            "    gpr jr_addend_not_hex = ctx->r10;\n",
            "    gpr jr_addend_80B5B1F8=ctx->r10;\n",
        );

        let (normalized, rewrite_count) = normalize_jump_snapshots(source);

        assert_eq!(rewrite_count, 0);
        assert_eq!(normalized, source);
    }

    #[test]
    fn discovers_only_called_recomp_identifiers_missing_from_declarations() {
        let calls = recomp_names_followed_by_paren(
            "declared_recomp(rdram, ctx); missing_recomp (rdram, ctx); not_recomp;",
        );
        let declared = recomp_names_followed_by_paren(
            "void declared_recomp(uint8_t* rdram, recomp_context* ctx);",
        );
        let missing: BTreeSet<_> = calls.difference(&declared).cloned().collect();

        assert_eq!(missing, BTreeSet::from(["missing_recomp".to_owned()]));
        assert_eq!(
            missing_prototype_prelude(&missing),
            concat!(
                "// fn64: prototypes required when generated C is compiled as C++.\n",
                "#include \"recomp.h\"\n",
                "extern \"C\" {\n",
                "void missing_recomp(uint8_t* rdram, recomp_context* ctx);\n",
                "}\n",
            )
        );
    }

    #[test]
    fn inserts_observer_first_in_every_generated_function_body() {
        let source = concat!(
            "RECOMP_FUNC void func_80001000(uint8_t* rdram, recomp_context* ctx) {\n",
            "    func_80002000(rdram, ctx);\n",
            "}\n",
            "  RECOMP_FUNC void func_80002000(uint8_t* rdram, recomp_context* ctx) {\r\n",
            "    ctx->r2 = 7;\r\n",
            "}\r\n",
        );

        let (instrumented, count) = instrument_generated_function_entries(source);

        assert_eq!(count, 2);
        assert_eq!(
            instrumented,
            concat!(
                "RECOMP_FUNC void func_80001000(uint8_t* rdram, recomp_context* ctx) {\n",
                "    fn64_c_recompiled_function_enter(func_80001000);\n",
                "    func_80002000(rdram, ctx);\n",
                "}\n",
                "  RECOMP_FUNC void func_80002000(uint8_t* rdram, recomp_context* ctx) {\r\n",
                "      fn64_c_recompiled_function_enter(func_80002000);\r\n",
                "    ctx->r2 = 7;\r\n",
                "}\r\n",
            )
        );
    }

    #[test]
    fn mends_only_address_proven_bodies_that_can_reach_their_close() {
        let tables = concat!(
            "static FuncEntry section_0[] = {\n",
            "{ .func = func_80001000, .offset = 0x00000000, .rom_size = 0x00000008 },\n",
            "{ .func = func_80001008, .offset = 0x00000008, .rom_size = 0x00000008 },\n",
            "{ .func = func_80001010, .offset = 0x00000010, .rom_size = 0x00000008 },\n",
            "};\n",
            "static FuncEntry section_1[] = {\n",
            "{ .func = func_80002000, .offset = 0x00000018, .rom_size = 0x00000008 },\n",
            "};\n",
        );
        let successors = fallthrough_successors(tables);
        assert_eq!(
            successors,
            BTreeMap::from([
                ("func_80001000".to_owned(), "func_80001008".to_owned()),
                ("func_80001008".to_owned(), "func_80001010".to_owned()),
            ])
        );

        let source = concat!(
            "RECOMP_FUNC void func_80001000(uint8_t* rdram, recomp_context* ctx) {\n",
            "    ctx->r2 = 7;\n",
            ";}\n",
            "RECOMP_FUNC void func_80001008(uint8_t* rdram, recomp_context* ctx) {\n",
            "    return;\n",
            ";}\n",
        );
        let (mended, count) = mend_proven_fallthroughs(source, &successors, "funcs_0.c");
        assert_eq!(count, 1);
        assert!(mended.contains(
            "ctx->r2 = 7;\n    // fn64: address-proven generated-fragment fall-through.\n    func_80001008(rdram, ctx);"
        ));
        assert!(!mended.contains("func_80001010(rdram, ctx)"));
    }

    #[test]
    fn fallthrough_repair_is_opt_in_and_retains_entry_instrumentation() {
        let root = std::env::temp_dir().join(format!(
            "fn64-build-support-fallthrough-{}",
            std::process::id()
        ));
        let input = root.join("input");
        let default_output = root.join("default-output");
        let repaired_output = root.join("repaired-output");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("funcs.h"), "").unwrap();
        std::fs::write(
            input.join("recomp_overlays.inl"),
            concat!(
                "static FuncEntry section_0[] = {\n",
                "{ .func = func_80001000, .offset = 0x0, .rom_size = 0x8 },\n",
                "{ .func = func_80001008, .offset = 0x8, .rom_size = 0x8 },\n",
                "};\n",
            ),
        )
        .unwrap();
        let original = concat!(
            "RECOMP_FUNC void func_80001000(uint8_t* rdram, recomp_context* ctx) {\n",
            "    ctx->r2 = 7;\n",
            ";}\n",
            "RECOMP_FUNC void func_80001008(uint8_t* rdram, recomp_context* ctx) {\n",
            "    return;\n",
            ";}\n",
        );
        std::fs::write(input.join("funcs_0.c"), original).unwrap();

        let (default_paths, _, _) = prepare_recompiled_cxx_sources(&input, &default_output);
        let default_prepared = std::fs::read_to_string(&default_paths[0]).unwrap();
        assert!(default_prepared
            .contains("fn64_c_recompiled_function_enter(func_80001000);\n    ctx->r2 = 7;"));
        assert!(!default_prepared.contains("func_80001008(rdram, ctx);"));

        let (repaired_paths, _, prototype_count, fallthrough_count) =
            prepare_recompiled_cxx_sources_with_fallthrough_repair(&input, &repaired_output);
        let repaired = std::fs::read_to_string(&repaired_paths[0]).unwrap();
        assert_eq!(prototype_count, 1);
        assert_eq!(fallthrough_count, 1);
        assert!(repaired.contains(
            "fn64_c_recompiled_function_enter(func_80001000);\n    ctx->r2 = 7;\n    // fn64: address-proven generated-fragment fall-through.\n    func_80001008(rdram, ctx);"
        ));
        assert!(repaired.contains("fn64_c_recompiled_function_enter(func_80001008);\n    return;"));
        assert_eq!(
            std::fs::read_to_string(input.join("funcs_0.c")).unwrap(),
            original
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[should_panic(expected = "cannot insert the entry observer first")]
    fn rejects_generated_body_with_code_on_opening_line() {
        let _ = instrument_generated_function_entries(
            "RECOMP_FUNC void func_80001000(uint8_t*, recomp_context*) { return; }\n",
        );
    }

    #[test]
    fn prepares_cpp_files_without_modifying_the_input_tree() {
        let root = std::env::temp_dir().join(format!("fn64-build-support-{}", std::process::id()));
        let input = root.join("input");
        let output = root.join("output");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("funcs.h"), "").unwrap();
        let original = "gpr jr_addend_80000000 = ctx->r2;\n";
        std::fs::write(input.join("funcs_1.c"), original).unwrap();

        let (paths, rewrite_count, missing_prototype_count) =
            prepare_recompiled_cxx_sources(&input, &output);

        assert_eq!(rewrite_count, 1);
        assert_eq!(missing_prototype_count, 0);
        assert_eq!(paths.len(), 1);
        assert_eq!(
            std::fs::read_to_string(input.join("funcs_1.c")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(&paths[0]).unwrap(),
            "gpr jr_addend_80000000;\njr_addend_80000000 = ctx->r2;\n"
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
