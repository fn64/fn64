// Included via `#[path]` by several build scripts across separate workspaces
// (fn64-shell, examples/{oot,sm64,wm2000}-boot), each of which uses a
// different subset of the helpers below. Anything unused by one consumer is
// still used by another, so dead_code here is a property of the include model
// rather than genuinely unreachable code.
#![allow(dead_code)]

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
//! tables. A generic generated-C host may instead request structurally proven
//! repair: the transform is enabled only when a reachable predecessor fall-off
//! has an address-contiguous successor whose generated instructions are a
//! split stack epilogue. Ordinary adjacent functions do not satisfy that gate.

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

/// One generated section-local function: a `RECOMP_FUNC` body that N64Recomp
/// emitted but did NOT list in any `recomp_overlays.inl` `FuncEntry` table.
///
/// N64Recomp names these `static_<section_index>_<link_vram>` because the
/// original symbol had file-local (`static`) linkage in the game's own
/// objects, so it is never an indirect-call target and needs no dispatch-table
/// row. They are still real recompiled bodies reached by direct C-to-C calls,
/// and `instrument_generated_function_entries` injects the execution observer
/// into them exactly as it does for table-listed bodies. Without a matching
/// registration the very first one entered aborts in
/// `fn64_c_recompiled_function_enter` with "was not registered in the
/// generated section table".
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SectionLocalFunction {
    /// Generated symbol, e.g. `static_4_8011FFA4`. Externally linkable: the
    /// generated `funcs.h` declares it, so the registration TU can take its
    /// address.
    pub name: String,
    /// Owning `SectionTableEntry.index`, parsed from the name.
    pub section_index: u32,
    /// Static link VRAM, parsed from the name.
    pub link_vram: u32,
}

/// Discover every `static_<section>_<vram>` body defined in generated sources.
///
/// The name is the ONLY carrier of the owning section and link address --
/// these bodies are absent from `recomp_overlays.inl` by construction, so
/// nothing else in the corpus states where they live. Parsing is therefore
/// strict: a name that does not split into a decimal section index and an
/// 8-hex-digit VRAM is left out rather than guessed at, and the caller
/// reconciles the discovered set against the section table's geometry before
/// registering anything.
fn section_local_function_definitions(source: &str) -> BTreeSet<SectionLocalFunction> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("RECOMP_FUNC void ")?;
            let name = rest.split_once('(')?.0.trim();
            let suffix = name.strip_prefix("static_")?;
            let (section, vram) = suffix.split_once('_')?;
            if section.is_empty() || !section.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            if vram.len() != 8 || !vram.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            Some(SectionLocalFunction {
                name: name.to_owned(),
                section_index: section.parse().ok()?,
                link_vram: u32::from_str_radix(vram, 16).ok()?,
            })
        })
        .collect()
}

/// `(index, ram_addr, size)` for every row of the generated `section_table[]`.
///
/// Read from the same `recomp_overlays.inl` the C bridge walks, so the
/// geometry a section-local registration is checked against is the geometry
/// the runtime actually registers -- not a second, independently drifting
/// transcription.
fn section_table_geometry(inl_source: &str) -> BTreeMap<u32, (u32, u32)> {
    let mut geometry = BTreeMap::new();
    for line in inl_source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("{ .rom_addr = ") else {
            continue;
        };
        let field = |name: &str| -> Option<u32> {
            let start = rest.find(name)? + name.len();
            let tail = &rest[start..];
            let end = tail
                .find(|c: char| !c.is_ascii_hexdigit() && c != 'x' && c != 'X')
                .unwrap_or(tail.len());
            let literal = tail[..end].trim();
            let hex = literal
                .strip_prefix("0x")
                .or_else(|| literal.strip_prefix("0X"));
            match hex {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => literal.parse().ok(),
            }
        };
        let (Some(ram_addr), Some(size), Some(index)) =
            (field(".ram_addr = "), field(".size = "), field(".index = "))
        else {
            continue;
        };
        geometry.insert(index, (ram_addr, size));
    }
    geometry
}

/// Emit the C++ translation unit that registers the discovered section-local
/// functions with the runtime, and return it plus the reconciled set.
///
/// Every discovered function is checked against the generated section table
/// twice, from two independent facts, and BOTH must hold or the build fails:
/// its parsed `section_index` must name a real row, and its parsed `link_vram`
/// must fall inside that row's `[ram_addr, ram_addr + size)`. A name that
/// disagrees with the table is a parse this code got wrong (or a corpus whose
/// naming convention changed), and registering it would bind an execution
/// observation to the wrong section -- silently mislabelling every downstream
/// measurement. Refuse by name instead, per `AGENTS.md`'s loud-trap rule.
///
/// The emitted TU calls the same `fn64_register_section_local_func` the Rust
/// harness exports for the section bridge, so registration flows through one
/// path rather than two.
fn section_local_registration_unit(
    functions: &BTreeSet<SectionLocalFunction>,
    geometry: &BTreeMap<u32, (u32, u32)>,
) -> String {
    let mut unit = String::new();
    unit.push_str(
        "/* Generated by fn64-boot-harness/build_support.rs -- do not edit.\n \
         * Registers N64Recomp's section-local (`static_<section>_<vram>`) bodies,\n \
         * which carry the execution observer but appear in no `FuncEntry` table. */\n",
    );
    unit.push_str("#include <stdint.h>\n#include <stddef.h>\n");
    unit.push_str("#include \"recomp.h\"\n#include \"funcs.h\"\n\n");
    unit.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n\n");
    unit.push_str(
        "extern void fn64_register_section_local_func(\n    \
         uint32_t section_index,\n    uint32_t link_vram,\n    recomp_func_t* func\n);\n\n",
    );
    unit.push_str("void fn64_bridge_register_section_local_funcs(void) {\n");
    for function in functions {
        let (ram_addr, size) = geometry.get(&function.section_index).unwrap_or_else(|| {
            panic!(
                "generated section-local function {} names section {}, which the generated \
                 section_table[] does not declare -- refusing to register it against a section \
                 that does not exist",
                function.name, function.section_index
            )
        });
        let end = u64::from(*ram_addr) + u64::from(*size);
        assert!(
            u64::from(function.link_vram) >= u64::from(*ram_addr)
                && u64::from(function.link_vram) < end,
            "generated section-local function {} parses to link vram {:#010x}, outside its own \
             section {}'s range [{:#010x}, {:#010x}) -- refusing to register a function at a \
             section it does not belong to",
            function.name,
            function.link_vram,
            function.section_index,
            ram_addr,
            end
        );
        unit.push_str(&format!(
            "    fn64_register_section_local_func({}u, {:#010x}u, {});\n",
            function.section_index, function.link_vram, function.name
        ));
    }
    unit.push_str("}\n\n#ifdef __cplusplus\n}\n#endif\n");
    unit
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
fn body_can_fall_through(body: &str) -> bool {
    body.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"))
        .is_none_or(|line| {
            line != "return;" && !line.starts_with("goto ") && !line.starts_with("switch_error(")
        })
}

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
        let can_fall_through = body_can_fall_through(body);

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

/// Inject an `FN64_BACKEDGE();` statement before every *backward* `goto` in
/// each generated function body -- a `goto L_x;` whose target label `L_x:`
/// appears earlier in the same function, i.e. a loop edge. This gives fn64's
/// cooperative executor a preemption point inside tight guest loops that poll
/// ordinary RDRAM (SM64's `while (gAudioFrameCount < n) {}`), which real
/// VR4300 hardware would preempt with the VI interrupt but fn64 otherwise
/// cannot: without a checkpoint, such a loop spins forever inside one
/// `run_one_step` resume and virtual time never advances. See
/// `fn64_mmio_proxy.h`'s `FN64_BACKEDGE` macro and `fn64-abi`'s
/// `fn64_c_backedge`. Forward gotos (structured `if`/`switch` control flow)
/// are left untouched: they cannot form a loop on their own and instrumenting
/// them would only add cost.
///
/// N64Recomp emits every label as `L_<hex-pc>:` and every jump as
/// `goto L_<hex-pc>;`, both on their own trimmed lines, and label names are
/// unique within a function (they are the target instruction's VA). The pass
/// is a two-scan-per-function transform: collect label positions, then emit,
/// prefixing any `goto` to an already-seen label.
fn inject_loop_backedges(source: &str) -> (String, usize) {
    let mut output = String::with_capacity(source.len() + source.len() / 16);
    let mut injected = 0;
    let mut rest = source;
    const FUNCTION_PREFIX: &str = "RECOMP_FUNC void ";

    while let Some(start) = rest.find(FUNCTION_PREFIX) {
        // Copy everything up to and including this function's opening region up
        // to its body close, transforming the body in between.
        let close_relative = rest[start..].find("\n;}").unwrap_or_else(|| {
            let after_prefix = &rest[start + FUNCTION_PREFIX.len()..];
            let name = after_prefix
                .split_once('(')
                .map_or("<unknown>", |(name, _)| name);
            panic!("inject_loop_backedges: {name}: no `;}}` body close found");
        });
        let body_end = start + close_relative;
        let function_span = &rest[start..body_end];

        // First scan: which labels are defined, and at what line index, so a
        // `goto` can tell backward (target already seen) from forward.
        let mut seen_labels: BTreeSet<&str> = BTreeSet::new();

        output.push_str(&rest[..start]);
        for segment in function_span.split_inclusive('\n') {
            let trimmed = segment.trim();
            // Record a label definition BEFORE emitting, so a `goto` on the
            // same-named label later in the function counts as backward.
            if let Some(label) = trimmed
                .strip_suffix(':')
                .filter(|candidate| is_generated_label(candidate))
            {
                seen_labels.insert(label);
                output.push_str(segment);
                continue;
            }
            if let Some(target) = trimmed
                .strip_prefix("goto ")
                .and_then(|rest| rest.strip_suffix(';'))
                .filter(|target| is_generated_label(target))
            {
                if seen_labels.contains(target) {
                    // Backward goto: preserve the goto's indentation for the
                    // injected call so the emitted C stays readable.
                    let indent_len = segment.len() - segment.trim_start_matches([' ', '\t']).len();
                    output.push_str(&segment[..indent_len]);
                    output.push_str("FN64_BACKEDGE();\n");
                    output.push_str(segment);
                    injected += 1;
                    continue;
                }
            }
            output.push_str(segment);
        }

        rest = &rest[body_end..];
    }
    output.push_str(rest);
    (output, injected)
}

/// True for an N64Recomp generated label/target identifier: `L_` followed by
/// hex digits (the target instruction's VA). Excludes the harness's own
/// `skip_N`/other structured labels, which are never loop targets the guest
/// branches back to.
fn is_generated_label(identifier: &str) -> bool {
    identifier
        .strip_prefix("L_")
        .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FallthroughRepair {
    Disabled,
    StructurallyProven,
    Forced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GeneratedFunctionShape {
    can_fall_through: bool,
    is_split_stack_epilogue: bool,
}

fn generated_function_shapes(
    source: &str,
    file_name: &str,
) -> BTreeMap<String, GeneratedFunctionShape> {
    let mut shapes = BTreeMap::new();
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
        let can_fall_through = body_can_fall_through(body);

        let restores_ra = body
            .lines()
            .any(|line| line.contains(": lw          $ra,") && line.contains("($sp)"));
        let saved_register_restores = body
            .lines()
            .filter(|line| line.contains(": lw          $s") && line.contains("($sp)"))
            .count();
        let returns_through_ra = body.lines().any(|line| line.contains(": jr          $ra"));
        let advances_sp = body
            .lines()
            .any(|line| line.contains(": addiu       $sp, $sp, 0x"));
        let allocates_stack = body
            .lines()
            .any(|line| line.contains(": addiu       $sp, $sp, -0x"));
        let is_split_stack_epilogue = restores_ra
            && saved_register_restores > 0
            && returns_through_ra
            && advances_sp
            && !allocates_stack;

        assert!(
            shapes
                .insert(
                    name.to_owned(),
                    GeneratedFunctionShape {
                        can_fall_through,
                        is_split_stack_epilogue,
                    },
                )
                .is_none(),
            "generated function {name} is defined more than once"
        );
        rest = &rest[body_end + 3..];
    }

    shapes
}

fn corpus_proves_split_epilogue(
    successors: &BTreeMap<String, String>,
    shapes: &BTreeMap<String, GeneratedFunctionShape>,
) -> bool {
    successors.iter().any(|(predecessor, successor)| {
        shapes
            .get(predecessor)
            .is_some_and(|shape| shape.can_fall_through)
            && shapes
                .get(successor)
                .is_some_and(|shape| shape.is_split_stack_epilogue)
    })
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
        prepare_recompiled_cxx_sources_inner(recompiled_dir, out_dir, FallthroughRepair::Disabled);
    assert_eq!(fallthroughs, 0);
    (paths, rewrites, prototypes)
}

/// Prepare generated C while enabling the section-local fall-through mend
/// only when the generated corpus proves it contains an answer-key-split
/// stack epilogue.
///
/// The proof requires a predecessor body that can reach its close and an
/// address-contiguous successor in the same generated section that restores
/// `$ra` plus at least one saved register, returns through `$ra`, and advances
/// `$sp` without allocating a new frame. This is the generic host path: a
/// normal pair of adjacent functions cannot opt itself into the wider repair.
pub fn prepare_recompiled_cxx_sources_with_proven_fallthrough_repair(
    recompiled_dir: &Path,
    out_dir: &Path,
) -> (Vec<PathBuf>, usize, usize, usize) {
    prepare_recompiled_cxx_sources_inner(
        recompiled_dir,
        out_dir,
        FallthroughRepair::StructurallyProven,
    )
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
    prepare_recompiled_cxx_sources_inner(recompiled_dir, out_dir, FallthroughRepair::Forced)
}

fn prepare_recompiled_cxx_sources_inner(
    recompiled_dir: &Path,
    out_dir: &Path,
    repair_fallthroughs: FallthroughRepair,
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
    let mut successors = if repair_fallthroughs != FallthroughRepair::Disabled {
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
    let mut section_local: BTreeSet<SectionLocalFunction> = BTreeSet::new();
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
            section_local.extend(section_local_function_definitions(&source));
            (source_path, source)
        })
        .collect();
    if repair_fallthroughs == FallthroughRepair::StructurallyProven {
        let mut shapes = BTreeMap::new();
        for (source_path, source) in &sources {
            let file_name = source_path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("generated C source filename must be valid Unicode");
            for (name, shape) in generated_function_shapes(source, file_name) {
                assert!(
                    shapes.insert(name.clone(), shape).is_none(),
                    "generated function {name} is defined more than once"
                );
            }
        }
        if !corpus_proves_split_epilogue(&successors, &shapes) {
            successors.clear();
        }
    }
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
        let (normalized, _file_backedge_count) = inject_loop_backedges(&normalized);
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

    // Section-local bodies carry the execution observer but appear in no
    // `FuncEntry` table, so they need their own registration TU. The geometry
    // they are reconciled against is read from the same generated
    // `recomp_overlays.inl` the C bridge walks.
    // Emitted unconditionally: the harness links
    // `fn64_bridge_register_section_local_funcs` whatever the corpus contains,
    // so a corpus with no section-local bodies gets an empty registrar rather
    // than an unresolved symbol.
    let section_local_count = section_local.len();
    let geometry = if section_local_count > 0 {
        let table_path = recompiled_dir.join("recomp_overlays.inl");
        let tables = std::fs::read_to_string(&table_path).unwrap_or_else(|error| {
            panic!(
                "failed to read generated section tables {}: {error}",
                table_path.display()
            )
        });
        let geometry = section_table_geometry(&tables);
        assert!(
            !geometry.is_empty(),
            "generated section tables at {} declare no sections, but {section_local_count} \
             section-local functions were found -- refusing to register them unreconciled",
            table_path.display()
        );
        geometry
    } else {
        BTreeMap::new()
    };
    let unit = section_local_registration_unit(&section_local, &geometry);
    let unit_path = prepared_dir.join("fn64_section_local_registration.cpp");
    std::fs::write(&unit_path, unit).unwrap_or_else(|error| {
        panic!(
            "failed to write section-local registration unit {}: {error}",
            unit_path.display()
        )
    });
    prepared_paths.push(unit_path);

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
    fn injects_backedge_only_before_backward_gotos() {
        // Mirrors SM64's `wait_for_audio_frames`: a backward `goto` (loop) plus
        // a forward `goto` (structured skip). Only the backward one is a spin
        // edge that needs a preemption point.
        let source = concat!(
            "RECOMP_FUNC void wait_for_audio_frames(uint8_t* rdram, recomp_context* ctx) {\n",
            "    if (ctx->r1 == 0) {\n",
            "        goto L_80317940;\n", // forward: label defined later
            "    }\n",
            "L_80317934:\n",
            "    if (ctx->r1 != 0) {\n",
            "        goto L_80317934;\n", // backward: label defined earlier
            "    }\n",
            "L_80317940:\n",
            "    return;\n",
            ";}\n",
        );

        let (injected, count) = inject_loop_backedges(source);

        assert_eq!(count, 1, "exactly one backward goto");
        // The backward goto gains a preceding FN64_BACKEDGE(); the forward one
        // is untouched.
        assert!(injected.contains("        FN64_BACKEDGE();\n        goto L_80317934;\n"));
        assert!(!injected.contains("FN64_BACKEDGE();\n        goto L_80317940;"));
        assert_eq!(
            injected.matches("FN64_BACKEDGE();").count(),
            1,
            "no spurious injections"
        );
    }

    #[test]
    fn backedge_labels_are_scoped_per_function() {
        // A label named identically in two functions must not make a later
        // function's forward goto look backward because an earlier function
        // defined that label. (N64Recomp's PC-based labels are actually unique,
        // but the pass must still reset its seen-label set per function.)
        let source = concat!(
            "RECOMP_FUNC void first(uint8_t* rdram, recomp_context* ctx) {\n",
            "L_100:\n",
            "    goto L_100;\n", // backward in `first`
            ";}\n",
            "RECOMP_FUNC void second(uint8_t* rdram, recomp_context* ctx) {\n",
            "    goto L_100;\n", // forward in `second` -- label defined below
            "L_100:\n",
            "    return;\n",
            ";}\n",
        );

        let (_injected, count) = inject_loop_backedges(source);
        assert_eq!(count, 1, "only `first`'s backward goto is instrumented");
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
    fn generic_host_repairs_a_section_local_bank_split_epilogue() {
        let root = std::env::temp_dir().join(format!(
            "fn64-build-support-proven-bank-fallthrough-{}",
            std::process::id()
        ));
        let input = root.join("input");
        let output = root.join("output");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("funcs.h"), "").unwrap();
        std::fs::write(
            input.join("recomp_overlays.inl"),
            concat!(
                "static FuncEntry section_5[] = {\n",
                "{ .func = func_8011F000_bank4_text, .offset = 0x100, .rom_size = 0x8 },\n",
                "{ .func = func_8011F008_bank4_text, .offset = 0x108, .rom_size = 0x10 },\n",
                "};\n",
            ),
        )
        .unwrap();
        let original = concat!(
            "RECOMP_FUNC void func_8011F000_bank4_text(uint8_t* rdram, recomp_context* ctx) {\n",
            "    ctx->r2 = 7;\n",
            ";}\n",
            "RECOMP_FUNC void func_8011F008_bank4_text(uint8_t* rdram, recomp_context* ctx) {\n",
            "    // 0x8011F008: lw          $ra, 0x14($sp)\n",
            "    ctx->r31 = MEM_W(ctx->r29, 0X14);\n",
            "    // 0x8011F00C: lw          $s0, 0x10($sp)\n",
            "    ctx->r16 = MEM_W(ctx->r29, 0X10);\n",
            "    // 0x8011F010: jr          $ra\n",
            "    // 0x8011F014: addiu       $sp, $sp, 0x18\n",
            "    ctx->r29 = ADD32(ctx->r29, 0X18);\n",
            "    return;\n",
            ";}\n",
        );
        std::fs::write(input.join("funcs_41.c"), original).unwrap();

        let (paths, _, prototype_count, fallthrough_count) =
            prepare_recompiled_cxx_sources_with_proven_fallthrough_repair(&input, &output);
        let repaired = std::fs::read_to_string(&paths[0]).unwrap();
        assert_eq!(prototype_count, 1);
        assert_eq!(fallthrough_count, 1);
        assert!(repaired.contains(
            "ctx->r2 = 7;\n    // fn64: address-proven generated-fragment fall-through.\n    func_8011F008_bank4_text(rdram, ctx);"
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generic_host_does_not_repair_merely_adjacent_normal_functions() {
        let root = std::env::temp_dir().join(format!(
            "fn64-build-support-unproven-fallthrough-{}",
            std::process::id()
        ));
        let input = root.join("input");
        let output = root.join("output");
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

        let (paths, _, prototype_count, fallthrough_count) =
            prepare_recompiled_cxx_sources_with_proven_fallthrough_repair(&input, &output);
        let prepared = std::fs::read_to_string(&paths[0]).unwrap();
        assert_eq!(prototype_count, 0);
        assert_eq!(fallthrough_count, 0);
        assert!(!prepared.contains("func_80001008(rdram, ctx);"));

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
    fn discovers_section_local_bodies_and_ignores_table_listed_ones() {
        let source = concat!(
            "RECOMP_FUNC void func_8011EA20(uint8_t* rdram, recomp_context* ctx) {\n",
            "RECOMP_FUNC void static_4_8011FFA4(uint8_t* rdram, recomp_context* ctx) {\n",
            "RECOMP_FUNC void static_5_8013EAD0(uint8_t* rdram, recomp_context* ctx) {\n",
            // Malformed names are skipped, never guessed at: the name is the
            // only carrier of section and address for these bodies.
            "RECOMP_FUNC void static_x_8011FFA4(uint8_t* rdram, recomp_context* ctx) {\n",
            "RECOMP_FUNC void static_4_80(uint8_t* rdram, recomp_context* ctx) {\n",
            "RECOMP_FUNC void static_4_ZZZZZZZZ(uint8_t* rdram, recomp_context* ctx) {\n",
        );

        let found = section_local_function_definitions(source);

        assert_eq!(
            found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["static_4_8011FFA4", "static_5_8013EAD0"]
        );
        let first = found.iter().next().unwrap();
        assert_eq!(first.section_index, 4);
        assert_eq!(first.link_vram, 0x8011_FFA4);
    }

    #[test]
    fn reads_section_geometry_from_the_generated_table() {
        let inl = concat!(
            "static SectionTableEntry section_table[] = {\n",
            "    { .rom_addr = 0x00073390, .ram_addr = 0x8011C900, .size = 0x00005DF0, \
             .funcs = a, .num_funcs = 1, .relocs = nullptr, .num_relocs = 0, .index = 3 },\n",
            "    { .rom_addr = 0x000809D0, .ram_addr = 0x8011C900, .size = 0x00044B60, \
             .funcs = b, .num_funcs = 1, .relocs = nullptr, .num_relocs = 0, .index = 4 },\n",
            "};\n",
        );

        let geometry = section_table_geometry(inl);

        // Both spellings of the same fact are asserted: the literal, and the
        // end address derived from base + size. Sections 3 and 4 genuinely
        // share one link base -- that overlap is the overlay-bank shape, and
        // it must survive parsing rather than be deduplicated away.
        assert_eq!(geometry.get(&3), Some(&(0x8011_C900, 0x5DF0)));
        assert_eq!(geometry.get(&4), Some(&(0x8011_C900, 0x0004_4B60)));
        assert_eq!(geometry[&3].0 + geometry[&3].1, 0x8012_26F0);
        assert_eq!(geometry.len(), 2);
    }

    #[test]
    #[should_panic(expected = "outside its own section")]
    fn refuses_a_section_local_function_outside_its_named_section() {
        let functions = [SectionLocalFunction {
            name: "static_3_80200000".to_owned(),
            section_index: 3,
            link_vram: 0x8020_0000,
        }]
        .into_iter()
        .collect();
        let geometry = [(3u32, (0x8011_C900u32, 0x5DF0u32))].into_iter().collect();

        let _ = section_local_registration_unit(&functions, &geometry);
    }

    #[test]
    #[should_panic(expected = "does not declare")]
    fn refuses_a_section_local_function_naming_an_absent_section() {
        let functions = [SectionLocalFunction {
            name: "static_9_8011D000".to_owned(),
            section_index: 9,
            link_vram: 0x8011_D000,
        }]
        .into_iter()
        .collect();
        let geometry = [(3u32, (0x8011_C900u32, 0x5DF0u32))].into_iter().collect();

        let _ = section_local_registration_unit(&functions, &geometry);
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
        // One prepared translation unit per input, plus the section-local
        // registration unit, which is emitted unconditionally so the harness
        // always links `fn64_bridge_register_section_local_funcs`.
        assert_eq!(paths.len(), 2);
        assert_eq!(
            std::fs::read_to_string(input.join("funcs_1.c")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(&paths[0]).unwrap(),
            "gpr jr_addend_80000000;\njr_addend_80000000 = ctx->r2;\n"
        );
        // This corpus declares no section-local bodies, so the registrar is
        // present but empty -- an empty registrar, never a missing symbol.
        let registration = std::fs::read_to_string(&paths[1]).unwrap();
        assert!(registration.contains("void fn64_bridge_register_section_local_funcs(void) {\n}\n"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
