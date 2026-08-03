use super::*;

/// A cited claim locating a game's load-request wrapper inside the proven
/// boot image: the callee's entry VA and which argument registers carry the
/// destination pointer, device address, and byte count (from the game's own
/// calling convention, e.g. MM boot's `DmaMgr_RequestAsync(req, ram, vrom,
/// size, ...)`). `device_space` declares what namespace the device operand
/// uses: `Physical` for raw cartridge offsets, `Virtual` for VROM that a DMA
/// manager translates — the latter is only accepted when the recovered range
/// sits inside exactly one already-proven VROM file mapping. The claim says
/// where to look; the boot image's instruction bytes still have to yield
/// fully constant operands, or the site stays an open frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticRequestDmaInput {
    pub name: String,
    pub callee_va: u32,
    pub dram_arg_register: u8,
    pub device_arg_register: u8,
    pub size_arg_register: u8,
    /// When set, the size register carries the EXCLUSIVE END device address
    /// instead of a byte count (SM64's `dma_read(dest, srcStart, srcEnd)`
    /// shape); the length is `end - device`, rejected unless positive.
    #[serde(default)]
    pub size_is_end_address: bool,
    pub device_space: RomAddressSpace,
    pub bank_name: BankNamePattern,
}

/// What a request-DMA scan proved and what it left open, for gate reports.
#[derive(Debug, Default)]
pub struct StaticRequestDmaReport {
    pub proven_banks: Vec<String>,
    pub open: Vec<String>,
    /// The deterministic loader-input prefix was scanned, but additional
    /// inputs were withheld by [`MAX_STATIC_REQUEST_DMA_INPUTS`].
    pub input_limit_hit: bool,
    /// Boot-image wrapper shapes examined by the candidate-only classifier.
    pub physical_wrapper_candidates_examined: usize,
    /// Shape candidates withheld because CFG/path and inner-callee semantic
    /// authority have not yet been established.
    pub wrapper_semantic_proof_unavailable: usize,
    /// The wrapper candidate scan itself stopped at its work bound.
    pub physical_wrapper_candidate_limit_hit: bool,
    /// Which required dataflow fact each rejected wrapper candidate failed to
    /// establish; the wrapper rule is the dominant geometry frontier, so the
    /// unmet fact is the actionable part of a rejection.
    pub wrapper_shape_rejections: crate::pi_dma::WrapperRejectionCensus,
}

impl StaticRequestDmaReport {
    pub(crate) fn push_open_bounded(&mut self, message: String) {
        if self.open.len() + 1 < MAX_STATIC_REQUEST_DMA_OPEN_ROWS {
            self.open.push(message);
        } else if self.open.len() + 1 == MAX_STATIC_REQUEST_DMA_OPEN_ROWS {
            self.open.push(format!(
                "request-DMA open frontier reached its {}-row reporting bound; additional rows omitted",
                MAX_STATIC_REQUEST_DMA_OPEN_ROWS
            ));
        }
    }
}

/// The largest RDRAM a retail console reaches (Expansion Pak). Used only to
/// bound destination sanity in the slicer; VA truth is judged downstream.
const SCAN_RDRAM_LEN: u32 = 0x0080_0000;
const MAX_STATIC_REQUEST_DMA_BANKS: usize = 4096;
const MAX_STATIC_REQUEST_DMA_INPUTS: usize = 64;
const MAX_STATIC_REQUEST_DMA_OPEN_ROWS: usize = 4096;
const MAX_STATIC_REQUEST_DMA_SCANNED_BYTES: usize = 256 * 1024 * 1024;

/// Recover load-image mappings from static operands at direct calls to a
/// cited request wrapper within the proven boot image. Each fully constant
/// (destination, device, size) triple that passes its declared-space
/// validation becomes a `Proven` bank mapping; every other call site is
/// reported open, never guessed. Reachability and completion are recorded as
/// unproven in the evidence note, matching `pi_dma`'s honesty contract.
pub fn scan_static_request_dma(
    rom: &NormalizedRom,
    inputs: &[StaticRequestDmaInput],
    db: &mut FactDb,
) -> StaticRequestDmaReport {
    scan_static_request_dma_bounded(
        rom,
        inputs,
        db,
        crate::file_table::DEFAULT_MAX_DECODED_VROM_FILE_BYTES,
    )
}

/// [`scan_static_request_dma`] with an explicit complete-file VROM decode
/// cap. A virtual request is not published unless its bytes materialize inside
/// that envelope.
pub fn scan_static_request_dma_bounded(
    rom: &NormalizedRom,
    inputs: &[StaticRequestDmaInput],
    db: &mut FactDb,
    max_decoded_vrom_file_bytes: usize,
) -> StaticRequestDmaReport {
    use crate::loaders::VirtualAddress;
    use std::collections::BTreeSet;

    let mut report = StaticRequestDmaReport::default();
    if inputs.is_empty() {
        return report;
    }
    let boot = db.proven_rom_mappings().iter().find_map(|fact| match fact {
        Fact::RomMapping {
            bank,
            rom_start,
            rom_end,
            va_start,
            ..
        } if bank == BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
        _ => None,
    });
    let Some((boot_rom_start, boot_rom_end, boot_va_start)) = boot else {
        report
            .open
            .push("boot bank not proven; request-dma scan skipped".to_string());
        return report;
    };
    let words: Vec<u32> = rom.bytes[boot_rom_start as usize..boot_rom_end as usize]
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
        .collect();

    for input in inputs {
        let slices = match crate::pi_dma::slice_load_request_calls(
            &words,
            VirtualAddress::new(boot_va_start),
            VirtualAddress::new(input.callee_va),
            SCAN_RDRAM_LEN,
            input.dram_arg_register,
            input.device_arg_register,
            input.size_arg_register,
        ) {
            Ok(slices) => slices,
            Err(error) => {
                report.open.push(format!(
                    "{}: slicer rejected boot image: {error:?}",
                    input.name
                ));
                continue;
            }
        };
        if slices.is_empty() {
            report.open.push(format!(
                "{}: no direct calls to cited callee 0x{:x} in the boot image",
                input.name, input.callee_va
            ));
            continue;
        }
        let mut seen: BTreeSet<(u32, u32, u32)> = BTreeSet::new();
        let mut index = 0u32;
        for slice in slices {
            let call_pc = slice.call_pc.get();
            let (Some(candidate), Some(dram_pointer)) =
                (slice.candidate(), slice.dram_pointer.proven().copied())
            else {
                report.open.push(format!(
                    "{}: call at 0x{call_pc:x} has open operands",
                    input.name
                ));
                continue;
            };
            let device = candidate.device_address.get();
            // In end-address mode the slicer's byte_count carries the raw
            // end operand (its rdram bound check then over-reserves by the
            // device offset — a conservative ceiling, never an undercheck).
            let length = if input.size_is_end_address {
                match candidate.byte_count.get().checked_sub(device) {
                    Some(length) if length > 0 => length,
                    _ => {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} end address 0x{:x} is not \
                             beyond device start 0x{device:x}",
                            input.name,
                            candidate.byte_count.get()
                        ));
                        continue;
                    }
                }
            } else {
                candidate.byte_count.get()
            };
            let va_start = dram_pointer.get();
            if !seen.insert((device, va_start, length)) {
                continue;
            }
            let (Some(device_end), Some(va_end)) =
                (device.checked_add(length), va_start.checked_add(length))
            else {
                report.open.push(format!(
                    "{}: call at 0x{call_pc:x} has an overflowing range",
                    input.name
                ));
                continue;
            };
            match input.device_space {
                RomAddressSpace::Physical => {
                    if device_end as usize > rom.len() {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} physical range \
                             0x{device:x}..0x{device_end:x} exceeds the ROM",
                            input.name
                        ));
                        continue;
                    }
                }
                RomAddressSpace::Virtual => {
                    let containing = db
                        .proven_vrom_file_mappings()
                        .iter()
                        .filter(|(_, fact)| {
                            matches!(fact, Fact::LoadImageTableRecord {
                                source_start,
                                source_end,
                                ..
                            } if device >= *source_start && device_end <= *source_end)
                        })
                        .count();
                    if containing != 1 {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} VROM range \
                             0x{device:x}..0x{device_end:x} has {containing} proven file \
                             mappings; expected exactly one",
                            input.name
                        ));
                        continue;
                    }
                    if let Err(error) = materialize_rom_range_bounded(
                        rom,
                        db,
                        RomAddressSpace::Virtual,
                        device,
                        device_end,
                        max_decoded_vrom_file_bytes,
                    ) {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} VROM range 0x{device:x}..0x{device_end:x} is unavailable within the decode limit: {error}",
                            input.name
                        ));
                        continue;
                    }
                }
            }
            let bank = input.bank_name.name(index);
            index += 1;
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.clone(),
                rom_space: input.device_space,
                rom_start: device,
                rom_end: device_end,
                va_start,
                va_end,
            });
            let evidence = db.insert(Fact::Evidence {
                subject: BankAddr::new(&bank, va_start),
                note: format!(
                    "static request-DMA operands at call 0x{call_pc:x} to cited {} \
                     (0x{:x}): device 0x{device:x}+0x{length:x} -> VA 0x{va_start:x}; \
                     instruction bytes do not prove reachability or completion",
                    input.name, input.callee_va
                ),
            });
            db.conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping, evidence],
                "static_request_dma_operands",
            )
            .expect("request-dma bank names are freshly generated");
            report.proven_banks.push(bank);
        }
    }
    report
}

/// Recover exact whole-file request-DMA loads to a bounded fixed point over
/// every proven, materializable bank.
///
/// Unlike [`scan_static_request_dma_bounded`], this production-auto path does
/// not treat a contained VROM slice as a new load image. Each virtual request
/// must equal exactly one proven file-table record. Newly recovered images are
/// scanned in later rounds, which admits loader calls made by resident code
/// loaded from the boot image without relying on a title-specific call site.
pub fn scan_static_request_dma_fixed_point_bounded(
    rom: &NormalizedRom,
    inputs: &[StaticRequestDmaInput],
    db: &mut FactDb,
    max_decoded_vrom_file_bytes: usize,
) -> StaticRequestDmaReport {
    use crate::loaders::VirtualAddress;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Debug, Clone)]
    struct PendingLoad {
        input_index: usize,
        source_bank: String,
        call_pc: u32,
        device: u32,
        device_end: u32,
        va_start: u32,
        va_end: u32,
    }

    type Geometry = (RomAddressSpace, u32, u32, u32, u32);

    let mut report = StaticRequestDmaReport::default();
    if inputs.is_empty() {
        return report;
    }
    let inputs = if inputs.len() > MAX_STATIC_REQUEST_DMA_INPUTS {
        report.input_limit_hit = true;
        report.push_open_bounded(format!(
            "request-DMA fixed point has {} loader inputs; scanning the deterministic first {MAX_STATIC_REQUEST_DMA_INPUTS} and withholding the remainder",
            inputs.len()
        ));
        &inputs[..MAX_STATIC_REQUEST_DMA_INPUTS]
    } else {
        inputs
    };
    let mut exact_vrom_files: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (_, fact) in db.proven_vrom_file_mappings() {
        if let Fact::LoadImageTableRecord {
            source_start,
            source_end,
            ..
        } = fact
        {
            *exact_vrom_files
                .entry((*source_start, *source_end))
                .or_default() += 1;
        }
    }

    let mut known_geometries: BTreeSet<Geometry> = db
        .proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping {
                rom_space,
                rom_start,
                rom_end,
                va_start,
                va_end,
                ..
            } => Some((*rom_space, *rom_start, *rom_end, *va_start, *va_end)),
            _ => None,
        })
        .collect();
    let mut scanned_banks: BTreeSet<(String, Geometry)> = BTreeSet::new();
    let mut scanned_bytes = 0usize;
    let mut next_bank_indices = vec![0u32; inputs.len()];

    loop {
        let mut sources: Vec<_> = db
            .proven_rom_mappings()
            .into_iter()
            .filter_map(|fact| match fact {
                Fact::RomMapping {
                    bank,
                    rom_space,
                    rom_start,
                    rom_end,
                    va_start,
                    va_end,
                } => Some((
                    bank.clone(),
                    (*rom_space, *rom_start, *rom_end, *va_start, *va_end),
                )),
                _ => None,
            })
            .filter(|source| !scanned_banks.contains(source))
            .collect();
        sources.sort();
        if sources.is_empty() {
            break;
        }
        if scanned_banks.len().saturating_add(sources.len()) > MAX_STATIC_REQUEST_DMA_BANKS {
            report.push_open_bounded(
                format!(
                    "request-DMA fixed point exceeds its {MAX_STATIC_REQUEST_DMA_BANKS}-bank scan bound"
                ),
            );
            break;
        }

        let mut pending: BTreeMap<Geometry, PendingLoad> = BTreeMap::new();
        for (source_bank, geometry) in sources {
            scanned_banks.insert((source_bank.clone(), geometry));
            let (source_space, source_rom_start, source_rom_end, source_va_start, _) = geometry;
            let materialized = match materialize_rom_range_bounded(
                rom,
                db,
                source_space,
                source_rom_start,
                source_rom_end,
                max_decoded_vrom_file_bytes,
            ) {
                Ok(materialized) => materialized,
                Err(error) => {
                    report.push_open_bounded(
                        format!(
                            "{source_bank}: proven request-DMA scan source is not materializable: {error}"
                        ),
                    );
                    continue;
                }
            };
            scanned_bytes = match scanned_bytes.checked_add(materialized.bytes.len()) {
                Some(total) if total <= MAX_STATIC_REQUEST_DMA_SCANNED_BYTES => total,
                _ => {
                    report.push_open_bounded(
                        format!(
                            "request-DMA fixed point exceeds its {MAX_STATIC_REQUEST_DMA_SCANNED_BYTES}-byte aggregate scan bound"
                        ),
                    );
                    return report;
                }
            };
            let words: Vec<u32> = materialized
                .bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                .collect();

            for (input_index, input) in inputs.iter().enumerate() {
                let slices = match crate::pi_dma::slice_load_request_calls(
                    &words,
                    VirtualAddress::new(source_va_start),
                    VirtualAddress::new(input.callee_va),
                    SCAN_RDRAM_LEN,
                    input.dram_arg_register,
                    input.device_arg_register,
                    input.size_arg_register,
                ) {
                    Ok(slices) => slices,
                    Err(error) => {
                        report.push_open_bounded(format!(
                            "{}: slicer rejected source bank {source_bank}: {error:?}",
                            input.name
                        ));
                        continue;
                    }
                };
                for slice in slices {
                    let call_pc = slice.call_pc.get();
                    let (Some(candidate), Some(dram_pointer)) =
                        (slice.candidate(), slice.dram_pointer.proven().copied())
                    else {
                        report.push_open_bounded(format!(
                            "{}: call at {source_bank}:0x{call_pc:x} has open operands",
                            input.name
                        ));
                        continue;
                    };
                    let device = candidate.device_address.get();
                    let length = if input.size_is_end_address {
                        match candidate.byte_count.get().checked_sub(device) {
                            Some(length) if length > 0 => length,
                            _ => {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} has an invalid end-address operand",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                        }
                    } else {
                        candidate.byte_count.get()
                    };
                    let va_start = dram_pointer.get();
                    let (Some(device_end), Some(va_end)) =
                        (device.checked_add(length), va_start.checked_add(length))
                    else {
                        report.push_open_bounded(format!(
                            "{}: call at {source_bank}:0x{call_pc:x} has an overflowing range",
                            input.name
                        ));
                        continue;
                    };
                    let target_geometry =
                        (input.device_space, device, device_end, va_start, va_end);
                    if known_geometries.contains(&target_geometry)
                        || pending.contains_key(&target_geometry)
                    {
                        continue;
                    }

                    match input.device_space {
                        RomAddressSpace::Physical => {
                            if device_end as usize > rom.len() {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} physical range 0x{device:x}..0x{device_end:x} exceeds the ROM",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                        }
                        RomAddressSpace::Virtual => {
                            let exact_records = exact_vrom_files
                                .get(&(device, device_end))
                                .copied()
                                .unwrap_or(0);
                            if exact_records != 1 {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} VROM range 0x{device:x}..0x{device_end:x} has {exact_records} exact proven file records; expected exactly one",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                            if let Err(error) = materialize_rom_range_bounded(
                                rom,
                                db,
                                RomAddressSpace::Virtual,
                                device,
                                device_end,
                                max_decoded_vrom_file_bytes,
                            ) {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} exact VROM file 0x{device:x}..0x{device_end:x} is unavailable within the decode limit: {error}",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                        }
                    }
                    pending.insert(
                        target_geometry,
                        PendingLoad {
                            input_index,
                            source_bank: source_bank.clone(),
                            call_pc,
                            device,
                            device_end,
                            va_start,
                            va_end,
                        },
                    );
                }
            }
        }

        if pending.is_empty() {
            continue;
        }
        for (geometry, load) in pending {
            if known_geometries.len() >= MAX_STATIC_REQUEST_DMA_BANKS {
                report.push_open_bounded(
                    format!(
                        "request-DMA fixed point reached its {MAX_STATIC_REQUEST_DMA_BANKS}-mapping bound"
                    ),
                );
                return report;
            }
            let input = &inputs[load.input_index];
            let bank = loop {
                let index = next_bank_indices[load.input_index];
                let Some(next) = index.checked_add(1) else {
                    report.push_open_bounded(format!(
                        "{}: request-DMA bank-name index overflow",
                        input.name
                    ));
                    return report;
                };
                next_bank_indices[load.input_index] = next;
                let candidate = input.bank_name.name(index);
                if !db.facts().iter().any(|fact| {
                        matches!(fact, Fact::RomMapping { bank, .. } if bank.as_str() == candidate)
                    }) {
                        break candidate;
                    }
            };
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.clone(),
                rom_space: input.device_space,
                rom_start: load.device,
                rom_end: load.device_end,
                va_start: load.va_start,
                va_end: load.va_end,
            });
            let evidence = db.insert(Fact::Evidence {
                subject: BankAddr::new(&load.source_bank, load.call_pc),
                note: format!(
                    "exact whole-file request-DMA operands at {}:0x{:x} to {} (0x{:x}): device 0x{:x}+0x{:x} -> VA 0x{:x}; instruction bytes do not prove reachability or completion",
                    load.source_bank,
                    load.call_pc,
                    input.name,
                    input.callee_va,
                    load.device,
                    load.device_end - load.device,
                    load.va_start
                ),
            });
            db.conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping, evidence],
                "static_request_dma_whole_file_fixed_point",
            )
            .expect("fixed-point request-DMA bank names are freshly generated");
            known_geometries.insert(geometry);
            report.proven_banks.push(bank);
        }
    }
    report
}

/// One mechanically recovered DMA-request routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredRequestDmaCallee {
    pub callee_va: u32,
    pub corroborated_sites: usize,
    pub resolved_sites: usize,
}

/// What mechanical request-DMA callee recovery admitted and left open.
#[derive(Debug, Default)]
pub struct RequestDmaCalleeRecovery {
    pub admitted: Vec<RecoveredRequestDmaCallee>,
    pub open: Vec<String>,
}

/// Recover, from ROM bytes alone, the routine a game uses to DMA a resident
/// image into RDRAM.
///
/// A resident code image is loaded by an explicit `RequestSync(ram, vrom,
/// size)`-shaped call rather than named by any descriptor table, so its VRAM
/// destination is invisible to table recovery. Rather than cite that routine's
/// address, admit it on machine-checkable evidence: a candidate IS the
/// DMA-request routine when the constant `(vrom, size)` operands recovered at
/// its direct call sites land exactly on file-table records already proven
/// from this ROM.
///
/// The rule is deliberately unforgiving. A candidate with ANY resolved call
/// site whose operands name no proven record is rejected outright: a real
/// loader's arguments describe real files, so one contradiction means the
/// shape matched something else.
pub fn recover_request_dma_callees(
    rom: &NormalizedRom,
    db: &FactDb,
    min_corroborated_sites: usize,
) -> RequestDmaCalleeRecovery {
    use crate::loaders::VirtualAddress;
    use std::collections::{BTreeMap, BTreeSet};

    let mut recovery = RequestDmaCalleeRecovery::default();
    let Some((boot_rom_start, boot_rom_end, boot_va_start)) =
        db.proven_rom_mappings().iter().find_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank == BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
            _ => None,
        })
    else {
        recovery
            .open
            .push("boot bank not proven; request-dma callee recovery skipped".to_string());
        return recovery;
    };
    let Some(image) = rom
        .bytes
        .get(boot_rom_start as usize..boot_rom_end as usize)
    else {
        recovery
            .open
            .push("boot bank ROM interval is outside the normalized image".to_string());
        return recovery;
    };
    let words: Vec<u32> = image
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
        .collect();

    // The corroboration set: every (vrom_start, length) a proven file-table
    // record already describes. Recovered operands must hit one exactly.
    let mut records: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (_, fact) in db.proven_vrom_file_mappings() {
        if let Fact::LoadImageTableRecord {
            source_start,
            source_end,
            ..
        } = fact
        {
            if let Some(len) = source_end.checked_sub(*source_start) {
                records.insert((*source_start, len));
            }
        }
    }
    if records.is_empty() {
        recovery
            .open
            .push("no proven file-table records to corroborate against".to_string());
        return recovery;
    }

    // Count direct call sites per target so the scan can skip targets with
    // fewer sites than the rule requires. This is only a bound on work: the
    // admission evidence is the exact record match below, never the count.
    let mut sites_per_target: BTreeMap<u32, usize> = BTreeMap::new();
    for (index, word) in words.iter().enumerate() {
        if word >> 26 != 0x03 {
            continue;
        }
        let pc = boot_va_start.wrapping_add((index as u32) * 4);
        let target = (pc & 0xF000_0000) | ((word & 0x03FF_FFFF) << 2);
        *sites_per_target.entry(target).or_default() += 1;
    }

    for (callee_va, site_count) in sites_per_target {
        if site_count < min_corroborated_sites {
            continue;
        }
        // o32 `RequestSync(ram, vrom, size)`: $a0/$a1/$a2.
        let Ok(slices) = crate::pi_dma::slice_load_request_calls(
            &words,
            VirtualAddress::new(boot_va_start),
            VirtualAddress::new(callee_va),
            SCAN_RDRAM_LEN,
            4,
            5,
            6,
        ) else {
            continue;
        };
        let mut corroborated = 0usize;
        let mut resolved = 0usize;
        let mut contradicted = false;
        for slice in &slices {
            let (Some(device), Some(bytes)) =
                (slice.device_address.proven(), slice.byte_count.proven())
            else {
                continue;
            };
            resolved += 1;
            if records.contains(&(device.get(), bytes.get())) {
                corroborated += 1;
            } else {
                contradicted = true;
                break;
            }
        }
        if contradicted || corroborated < min_corroborated_sites {
            continue;
        }
        recovery.admitted.push(RecoveredRequestDmaCallee {
            callee_va,
            corroborated_sites: corroborated,
            resolved_sites: resolved,
        });
    }
    if recovery.admitted.is_empty() {
        recovery.open.push(
            "no callee's call-site operands corroborated against proven file records".to_string(),
        );
    }
    recovery
}

#[cfg(test)]
mod request_dma_recovery_tests {
    use super::*;
    use crate::rom::normalize;

    /// Place `words` at the start of the IPL3 boot image of a synthetic z64.
    fn rom_with_boot_words(entry: u32, words: &[u32]) -> NormalizedRom {
        let mut buf = vec![0u8; BOOT_COPY_ROM_START as usize + BOOT_COPY_SIZE as usize];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&entry.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        for (index, word) in words.iter().enumerate() {
            let offset = BOOT_COPY_ROM_START as usize + index * 4;
            buf[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        normalize(&buf).expect("valid synthetic z64")
    }

    /// Boot image that materializes `(ram, vrom, size)` in $a0/$a1/$a2 and
    /// calls `loader`, mirroring an o32 `RequestSync(ram, vrom, size)` site.
    fn boot_calling_loader(vrom: u32, size: u32, loader: u32) -> Vec<u32> {
        vec![
            0x2405_0000 | (vrom & 0xFFFF),               // addiu $a1, $zero, vrom
            0x2406_0000 | (size & 0xFFFF),               // addiu $a2, $zero, size
            0x3C04_8000,                                 // lui   $a0, 0x8000
            0x0C00_0000 | ((loader & 0x0FFF_FFFF) >> 2), // jal   loader
            0x0000_0000,                                 // nop
        ]
    }

    fn request_call(vrom: u32, size: u32, destination: u32, loader: u32) -> Vec<u32> {
        vec![
            0x3c05_0000 | (vrom >> 16),
            0x34a5_0000 | (vrom & 0xffff),
            0x3c06_0000 | (size >> 16),
            0x34c6_0000 | (size & 0xffff),
            0x3c04_0000 | (destination >> 16),
            0x3484_0000 | (destination & 0xffff),
            0x0c00_0000 | ((loader & 0x0fff_ffff) >> 2),
            0,
        ]
    }

    fn fixed_point_rom(boot_words: &[u32], first_file_words: &[u32]) -> NormalizedRom {
        const FIRST_PHYSICAL: usize = 0x102000;
        const SECOND_PHYSICAL: usize = 0x103000;
        let mut buf = vec![0u8; SECOND_PHYSICAL + 0x40];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        for (index, word) in boot_words.iter().enumerate() {
            let offset = BOOT_COPY_ROM_START as usize + index * 4;
            buf[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        for (index, word) in first_file_words.iter().enumerate() {
            let offset = FIRST_PHYSICAL + index * 4;
            buf[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        normalize(&buf).expect("valid fixed-point synthetic z64")
    }

    fn add_file_record(
        db: &mut FactDb,
        table: &str,
        index: u32,
        vrom: u32,
        size: u32,
        physical: u32,
    ) {
        let fact = db.insert(Fact::LoadImageTableRecord {
            table: table.to_string(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index,
            source_space: crate::facts::MappingAddressSpace::VirtualRom,
            source_start: vrom,
            source_end: vrom + size,
            destination_space: crate::facts::MappingAddressSpace::PhysicalRom,
            destination_start: physical,
            destination_end: physical + size,
        });
        db.conclude(
            load_image_table_record_subject(table, index),
            ProofState::Proven,
            vec![fact],
            "fixed-point test fixture",
        )
        .expect("fresh file record");
    }

    fn fixed_point_input() -> StaticRequestDmaInput {
        StaticRequestDmaInput {
            name: "request_sync".to_string(),
            callee_va: FIXTURE_LOADER,
            dram_arg_register: 4,
            device_arg_register: 5,
            size_arg_register: 6,
            size_is_end_address: false,
            device_space: RomAddressSpace::Virtual,
            bank_name: BankNamePattern::new("request_dma_", 0, ""),
        }
    }

    #[test]
    fn physical_end_address_contract_publishes_the_exact_range() {
        const PHYSICAL_START: u32 = 0x20;
        const PHYSICAL_END: u32 = 0x60;
        const DESTINATION: u32 = 0x8010_0000;
        let rom = rom_with_boot_words(
            0x8000_0400,
            &request_call(PHYSICAL_START, PHYSICAL_END, DESTINATION, FIXTURE_LOADER),
        );
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        let input = StaticRequestDmaInput {
            name: "physical_end".to_string(),
            callee_va: FIXTURE_LOADER,
            dram_arg_register: 4,
            device_arg_register: 5,
            size_arg_register: 6,
            size_is_end_address: true,
            device_space: RomAddressSpace::Physical,
            bank_name: BankNamePattern::new("request_dma_", 0, ""),
        };

        let report = scan_static_request_dma_fixed_point_bounded(&rom, &[input], &mut db, 1024);

        assert_eq!(report.proven_banks, ["request_dma_0"]);
        assert!(db.proven_rom_mappings().iter().any(|fact| matches!(
            fact,
            Fact::RomMapping {
                bank,
                rom_space: RomAddressSpace::Physical,
                rom_start: PHYSICAL_START,
                rom_end: PHYSICAL_END,
                va_start: DESTINATION,
                va_end: 0x8010_0040,
            } if bank == "request_dma_0"
        )));
    }

    fn db_with_proven_record(rom: &NormalizedRom, vrom: u32, size: u32) -> FactDb {
        let mut db = FactDb::new();
        let _outcome = publish_boot_bank(
            rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        let fact = db.insert(Fact::LoadImageTableRecord {
            table: "t".to_string(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index: 0,
            source_space: crate::facts::MappingAddressSpace::VirtualRom,
            source_start: vrom,
            source_end: vrom + size,
            destination_space: crate::facts::MappingAddressSpace::PhysicalRom,
            destination_start: 0,
            destination_end: size,
        });
        db.conclude(
            load_image_table_record_subject("t", 0),
            ProofState::Proven,
            vec![fact],
            "test fixture",
        )
        .expect("fresh conclusion");
        db
    }

    const FIXTURE_VROM: u32 = 0x1060;
    const FIXTURE_SIZE: u32 = 0x63d0;
    const FIXTURE_LOADER: u32 = 0x8000_0500;

    #[test]
    fn whole_file_request_dma_reaches_a_two_hop_fixed_point() {
        const FIRST_VROM: u32 = 0x0020_0000;
        const SECOND_VROM: u32 = 0x0021_0000;
        const FILE_SIZE: u32 = 0x40;
        const FIRST_VA: u32 = 0x8010_0000;
        const SECOND_VA: u32 = 0x8020_0000;
        let rom = fixed_point_rom(
            &request_call(FIRST_VROM, FILE_SIZE, FIRST_VA, FIXTURE_LOADER),
            &request_call(SECOND_VROM, FILE_SIZE, SECOND_VA, FIXTURE_LOADER),
        );
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut db, "files", 0, FIRST_VROM, FILE_SIZE, 0x102000);
        add_file_record(&mut db, "files", 1, SECOND_VROM, FILE_SIZE, 0x103000);

        let report = scan_static_request_dma_fixed_point_bounded(
            &rom,
            &[fixed_point_input()],
            &mut db,
            1024,
        );

        assert_eq!(report.proven_banks, ["request_dma_0", "request_dma_1"]);
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::RomMapping {
                bank,
                rom_start: SECOND_VROM,
                rom_end: 0x0021_0040,
                va_start: SECOND_VA,
                va_end: 0x8020_0040,
                ..
            } if bank == "request_dma_1"
        )));
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::Evidence { subject, note }
                if subject.bank == "request_dma_0"
                    && subject.pc == FIRST_VA + 24
                    && note.contains("request_dma_0:0x80100018")
        )));
    }

    #[test]
    fn fixed_point_scans_a_deterministic_prefix_when_loader_input_limit_is_hit() {
        const VROM: u32 = 0x0020_0000;
        const FILE_SIZE: u32 = 0x40;
        const VA: u32 = 0x8010_0000;
        let rom = fixed_point_rom(&request_call(VROM, FILE_SIZE, VA, FIXTURE_LOADER), &[]);
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut db, "files", 0, VROM, FILE_SIZE, 0x102000);
        let inputs = vec![fixed_point_input(); MAX_STATIC_REQUEST_DMA_INPUTS + 1];

        let report = scan_static_request_dma_fixed_point_bounded(&rom, &inputs, &mut db, 1024);

        assert!(report.input_limit_hit);
        assert_eq!(report.proven_banks, ["request_dma_0"]);
        assert!(report
            .open
            .iter()
            .any(|row| row.contains("scanning the deterministic first 64")));
    }

    #[test]
    fn fixed_point_rejects_contained_and_ambiguous_vrom_requests() {
        const VROM: u32 = 0x0020_0000;
        const FILE_SIZE: u32 = 0x40;
        let contained_rom = fixed_point_rom(
            &request_call(VROM + 4, FILE_SIZE - 4, 0x8010_0000, FIXTURE_LOADER),
            &[],
        );
        let mut contained_db = FactDb::new();
        let _ = publish_boot_bank(
            &contained_rom,
            &mut contained_db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut contained_db, "files", 0, VROM, FILE_SIZE, 0x102000);
        let contained = scan_static_request_dma_fixed_point_bounded(
            &contained_rom,
            &[fixed_point_input()],
            &mut contained_db,
            1024,
        );
        assert!(contained.proven_banks.is_empty());
        assert!(contained
            .open
            .iter()
            .any(|row| row.contains("has 0 exact proven file records")));

        let ambiguous_rom = fixed_point_rom(
            &request_call(VROM, FILE_SIZE, 0x8010_0000, FIXTURE_LOADER),
            &[],
        );
        let mut ambiguous_db = FactDb::new();
        let _ = publish_boot_bank(
            &ambiguous_rom,
            &mut ambiguous_db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut ambiguous_db, "files_a", 0, VROM, FILE_SIZE, 0x102000);
        add_file_record(&mut ambiguous_db, "files_b", 0, VROM, FILE_SIZE, 0x103000);
        let ambiguous = scan_static_request_dma_fixed_point_bounded(
            &ambiguous_rom,
            &[fixed_point_input()],
            &mut ambiguous_db,
            1024,
        );
        assert!(ambiguous.proven_banks.is_empty());
        assert!(ambiguous
            .open
            .iter()
            .any(|row| row.contains("has 2 exact proven file records")));
    }

    #[test]
    fn fixed_point_deduplicates_repeated_exact_requests() {
        const VROM: u32 = 0x0020_0000;
        const FILE_SIZE: u32 = 0x40;
        const VA: u32 = 0x8010_0000;
        let call = request_call(VROM, FILE_SIZE, VA, FIXTURE_LOADER);
        let boot_words: Vec<_> = call.iter().chain(&call).copied().collect();
        let rom = fixed_point_rom(&boot_words, &[]);
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut db, "files", 0, VROM, FILE_SIZE, 0x102000);

        let report = scan_static_request_dma_fixed_point_bounded(
            &rom,
            &[fixed_point_input()],
            &mut db,
            1024,
        );

        assert_eq!(report.proven_banks, ["request_dma_0"]);
        assert_eq!(
            db.proven_rom_mappings()
                .into_iter()
                .filter(|fact| matches!(
                    fact,
                    Fact::RomMapping {
                        rom_start: VROM,
                        rom_end: 0x0020_0040,
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn request_dma_callee_is_recovered_when_operands_hit_a_proven_record() {
        let rom = rom_with_boot_words(
            0x8000_0400,
            &boot_calling_loader(FIXTURE_VROM, FIXTURE_SIZE, FIXTURE_LOADER),
        );
        let db = db_with_proven_record(&rom, FIXTURE_VROM, FIXTURE_SIZE);

        let recovery = recover_request_dma_callees(&rom, &db, 1);
        assert_eq!(
            recovery.admitted.len(),
            1,
            "exactly the loader should be admitted, got {:?}",
            recovery.admitted
        );
        assert_eq!(recovery.admitted[0].callee_va, FIXTURE_LOADER);
        assert_eq!(recovery.admitted[0].corroborated_sites, 1);
    }

    #[test]
    fn request_dma_callee_is_rejected_when_operands_name_no_proven_record() {
        // Identical call site, but the proven record describes a different
        // length. One contradicting site must reject the candidate outright --
        // this is what stops an arbitrary three-argument call from being
        // mistaken for a loader.
        let rom = rom_with_boot_words(
            0x8000_0400,
            &boot_calling_loader(FIXTURE_VROM, FIXTURE_SIZE, FIXTURE_LOADER),
        );
        let db = db_with_proven_record(&rom, FIXTURE_VROM, FIXTURE_SIZE + 0x10);

        let recovery = recover_request_dma_callees(&rom, &db, 1);
        assert!(
            recovery.admitted.is_empty(),
            "a size that matches no record must not be admitted, got {:?}",
            recovery.admitted
        );
    }

    #[test]
    fn bounded_request_dma_refuses_a_slice_from_an_oversized_vrom_file() {
        const DECODED_FILE_BYTES: u32 = 0x0010_0000;
        const PHYSICAL_START: usize = 0x2000;
        let mut rom = rom_with_boot_words(
            0x8000_0400,
            &boot_calling_loader(FIXTURE_VROM, 4, FIXTURE_LOADER),
        );
        rom.bytes[PHYSICAL_START..PHYSICAL_START + 4].copy_from_slice(b"Yaz0");
        rom.bytes[PHYSICAL_START + 4..PHYSICAL_START + 8]
            .copy_from_slice(&DECODED_FILE_BYTES.to_be_bytes());
        let mut db = FactDb::new();
        let _outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        let record = db.insert(Fact::LoadImageTableRecord {
            table: "oversized".to_string(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index: 0,
            source_space: crate::facts::MappingAddressSpace::VirtualRom,
            source_start: FIXTURE_VROM,
            source_end: FIXTURE_VROM + DECODED_FILE_BYTES,
            destination_space: crate::facts::MappingAddressSpace::PhysicalRom,
            destination_start: PHYSICAL_START as u32,
            destination_end: (PHYSICAL_START + 16) as u32,
        });
        db.conclude(
            load_image_table_record_subject("oversized", 0),
            ProofState::Proven,
            vec![record],
            "test fixture",
        )
        .expect("fresh conclusion");
        let input = StaticRequestDmaInput {
            name: "oversized_request".to_string(),
            callee_va: FIXTURE_LOADER,
            dram_arg_register: 4,
            device_arg_register: 5,
            size_arg_register: 6,
            size_is_end_address: false,
            device_space: RomAddressSpace::Virtual,
            bank_name: BankNamePattern::new("request_dma_", 0, ""),
        };

        let report = scan_static_request_dma_bounded(&rom, &[input], &mut db, 1024);
        assert!(report.proven_banks.is_empty());
        assert!(report
            .open
            .iter()
            .any(|reason| reason.contains("exceeds transient limit 1024")));
        assert!(!db.proven_rom_mappings().iter().any(
            |fact| matches!(fact, Fact::RomMapping { bank, .. } if bank.starts_with("request_dma_"))
        ));
        crate::harvest::harvest_discovered_candidates_bounded(&rom, &mut db, 1024)
            .expect("the rejected request mapping cannot reach harvest");
    }
}
