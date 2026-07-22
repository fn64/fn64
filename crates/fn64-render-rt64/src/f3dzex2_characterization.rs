//! Explicitly opt-in native F3DZEX2 black-box characterization transport.
//!
//! This module is separate from production `RenderBackend` admission: its
//! public method is evidence-named, feature-gated, and accepts no caller-
//! selected microcode identity. Pinned raw-pair recognition derives the exact
//! variant before the native context can be borrowed or guest memory mutated.

use fn64_render::{
    F3dzex2Variant, MicrocodeDataImageIdentity, OsTask, RenderError, TaskAdmissionGeneration,
    TaskAdmissionRawWindow, TaskAdmissionSource, TaskAdmissionUcode, UcodeDigest,
};
use sha2::{Digest, Sha256};

use crate::transaction::{NativeContextLease, NativeTaskMemoryRollback};
use crate::{
    ffi, Rt64Backend, Rt64TaskAdmission, RT64_GBI_DATA_RECOGNITION_BYTES,
    RT64_GBI_TEXT_RECOGNITION_BYTES,
};

const EVIDENCE_BACKEND: &str = "rt64-f3dzex2-characterization-evidence";

/// One native microcode text/data address pair observed by RT64.
///
/// This value contains addresses only. It never carries private microcode
/// bytes or game output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64F3dzex2UcodeAddresses {
    pub text: u32,
    pub data: u32,
}

/// Bounded result of one explicitly requested F3DZEX2 characterization task.
///
/// The result deliberately omits content-derived identities. The existing
/// native result decoder compares the plan digest before this value can be
/// constructed, while the local runner retains no private digest.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64F3dzex2CharacterizationEvidence {
    pub variant: F3dzex2Variant,
    pub planned_generation_count: u32,
    pub observed_generation_count: u32,
    pub full_sync_count: u64,
    pub initial_ucode: Rt64F3dzex2UcodeAddresses,
    pub final_ucode: Rt64F3dzex2UcodeAddresses,
}

fn characterization_error(reason: impl Into<String>) -> RenderError {
    RenderError::Backend {
        backend: EVIDENCE_BACKEND,
        reason: reason.into(),
    }
}

fn validate_rdram_range(offset: u32, len: usize, rdram_len: usize) -> Result<(), RenderError> {
    let start = usize::try_from(offset).expect("u32 RDRAM offset fits usize");
    let Some(end) = start.checked_add(len) else {
        return Err(RenderError::InvalidTaskBounds {
            offset,
            len: u32::try_from(len).unwrap_or(u32::MAX),
            rdram_len,
        });
    };
    if end > rdram_len {
        return Err(RenderError::InvalidTaskBounds {
            offset,
            len: u32::try_from(len).unwrap_or(u32::MAX),
            rdram_len,
        });
    }
    Ok(())
}

fn capture_entry(
    rdram: &[u8],
    task: &OsTask,
) -> Result<(u32, u32, TaskAdmissionRawWindow), RenderError> {
    if rdram.len() < fn64_runtime::rdram::DEFAULT_RDRAM_SIZE {
        return Err(characterization_error(format!(
            "characterization RDRAM has {:#x} bytes, below the required {:#x}",
            rdram.len(),
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE
        )));
    }
    if task.task_type != fn64_render::M_GFXTASK {
        return Err(characterization_error(format!(
            "characterization requires M_GFXTASK, received task type {}",
            task.task_type
        )));
    }
    for (name, address) in [
        ("ucode", task.ucode),
        ("ucode data", task.ucode_data),
        ("display list", task.data_ptr),
    ] {
        if address & 0xff00_0000 != 0 {
            return Err(characterization_error(format!(
                "characterization {name} address {address:#010x} is not a physical RDRAM offset"
            )));
        }
    }
    if task.ucode_size != fn64_runtime::RSP_MEMORY_BANK_SIZE as u32 {
        return Err(characterization_error(format!(
            "characterization microcode text size {} must be exactly 4 KiB",
            task.ucode_size
        )));
    }
    let text_address = task.ucode;
    let data_address = task.ucode_data;
    if !text_address.is_multiple_of(8) || !data_address.is_multiple_of(8) {
        return Err(characterization_error(format!(
            "task microcode text/data addresses {text_address:#010x}/{data_address:#010x} must be 64-bit aligned"
        )));
    }
    let data_bytes =
        usize::try_from(task.ucode_data_size).expect("OSTask microcode data size fits usize");
    if data_bytes != RT64_GBI_DATA_RECOGNITION_BYTES {
        return Err(characterization_error(format!(
            "characterization microcode data size {data_bytes} must equal the admitted {RT64_GBI_DATA_RECOGNITION_BYTES:#x}-byte raw window"
        )));
    }

    validate_rdram_range(text_address, RT64_GBI_TEXT_RECOGNITION_BYTES, rdram.len())?;
    validate_rdram_range(data_address, RT64_GBI_DATA_RECOGNITION_BYTES, rdram.len())?;
    validate_rdram_range(data_address, data_bytes, rdram.len())?;
    let raw_window = fn64_render::capture_task_admission_raw_window(
        rdram,
        fn64_runtime::RdramAddr::from_offset(text_address),
        fn64_runtime::RdramAddr::from_offset(data_address),
        fn64_render::TaskAdmissionRawWindowSize {
            text: RT64_GBI_TEXT_RECOGNITION_BYTES,
            data: RT64_GBI_DATA_RECOGNITION_BYTES,
        },
    )
    .ok_or_else(|| {
        characterization_error("validated task microcode recognition windows could not be captured")
    })?;
    Ok((text_address, data_address, raw_window))
}

fn build_admission(
    rdram: &[u8],
    rsp_memory: &fn64_runtime::RspMemory,
    task: &OsTask,
    text_address: u32,
    data_address: u32,
    raw_window: TaskAdmissionRawWindow,
    variant: F3dzex2Variant,
) -> Result<Rt64TaskAdmission, RenderError> {
    let mut logical_text = vec![0; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(text_address),
        &mut logical_text,
    );
    let live_text = rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem);
    let live_text_sha256 = UcodeDigest::from_text(live_text);
    if logical_text.as_slice() != live_text {
        return Err(RenderError::RequiresLle {
            ucode_sha256: live_text_sha256.as_bytes(),
        });
    }

    let data_bytes = usize::try_from(task.ucode_data_size)
        .expect("validated OSTask microcode data size fits usize");
    let mut logical_data = vec![0; data_bytes];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(data_address),
        &mut logical_data,
    );
    let entry = TaskAdmissionGeneration {
        source: TaskAdmissionSource::TaskEntry,
        text_address,
        data_address,
        text_sha256: live_text_sha256,
        data: MicrocodeDataImageIdentity {
            bytes: task.ucode_data_size,
            sha256: Sha256::digest(logical_data).into(),
        },
        ucode: TaskAdmissionUcode::F3dzex2(variant),
    };
    Ok(Rt64TaskAdmission {
        plan: fn64_render::TaskAdmissionPlan::new(entry, []),
        raw_windows: vec![raw_window].into_boxed_slice(),
    })
}

fn prepare_admission(
    rdram: &[u8],
    rsp_memory: &fn64_runtime::RspMemory,
    task: &OsTask,
) -> Result<(F3dzex2Variant, Rt64TaskAdmission), RenderError> {
    let (text_address, data_address, raw_window) = capture_entry(rdram, task)?;
    let variant =
        fn64_render::identify_f3dzex2(&raw_window).ok_or_else(|| RenderError::RequiresLle {
            ucode_sha256: UcodeDigest::from_text(
                rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem),
            )
            .as_bytes(),
        })?;
    let admission = build_admission(
        rdram,
        rsp_memory,
        task,
        text_address,
        data_address,
        raw_window,
        variant,
    )?;
    Ok((variant, admission))
}

fn validate_complete_result(
    entry: &TaskAdmissionGeneration,
    result: &ffi::NativeTaskResult,
    expected_full_sync_count: u64,
) -> Result<(), RenderError> {
    let expected_addresses = (entry.text_address, entry.data_address);
    if result.initial_ucode_addresses != expected_addresses
        || result.final_ucode_addresses != expected_addresses
        || result.full_sync_count != expected_full_sync_count
    {
        return Err(characterization_error(format!(
            "native result changed the entry-only characterization contract: addresses {:#010x}/{:#010x} -> {:#010x}/{:#010x}, FullSync count {} (expected {})",
            result.initial_ucode_addresses.0,
            result.initial_ucode_addresses.1,
            result.final_ucode_addresses.0,
            result.final_ucode_addresses.1,
            result.full_sync_count,
            expected_full_sync_count,
        )));
    }
    Ok(())
}

impl Rt64Backend {
    /// Execute one locally supplied F3DZEX2 task through pinned RT64 solely
    /// for black-box characterization evidence.
    ///
    /// This method derives the exact 2.06H, 2.08I, or 2.08J variant from the
    /// raw task-entry text/data pair and admits only that single generation.
    /// It deliberately bypasses the production geometry catalog but retains
    /// native recognition, typed-plan validation, context poisoning, and
    /// guest-memory rollback. Calling it can commit RDRAM/RSP/native renderer
    /// mutations on success; it is not a read-only probe.
    /// `expected_full_sync_count` is part of the controlled vector contract;
    /// a mismatch destroys the native context and rolls guest memory back
    /// before the observation can escape.
    ///
    /// Enabling this API does not change `RenderBackend::process_task`,
    /// `RenderBackend::supported_ucodes`, or any `with_*ucode*` builder.
    pub fn process_f3dzex2_task_for_characterization_evidence(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
        expected_full_sync_count: u64,
    ) -> Result<Rt64F3dzex2CharacterizationEvidence, RenderError> {
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        let (variant, admission_plan) = prepare_admission(rdram, rsp_memory, task)?;
        crate::ingress::validate_task_ingress(
            rdram.len(),
            task,
            output_addr,
            self.active_surface_size,
        )?;

        let mut context = NativeContextLease::take(&mut self.context)
            .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?;
        let mut transaction =
            NativeTaskMemoryRollback::new(rdram, rsp_memory, &mut self.native_rdram_preimage);
        let native_call = {
            let (native_rdram, native_rsp) = transaction.memories_mut();
            context.context_mut().process_task(
                native_rdram,
                native_rsp,
                task,
                output_addr,
                &admission_plan,
            )
        };
        let native_outcome = match native_call {
            Ok(outcome) => outcome,
            Err(reason) => {
                drop(context);
                drop(transaction);
                self.invalidate_native_state();
                return Err(RenderError::Backend {
                    backend: EVIDENCE_BACKEND,
                    reason,
                });
            }
        };
        let native_result = match native_outcome {
            ffi::NativeTaskOutcome::Complete(result) => result,
            ffi::NativeTaskOutcome::NeedsLle {
                rejected_generation,
                plan_sha256: _,
            } => {
                let generation = admission_plan
                    .plan
                    .generations()
                    .get(rejected_generation as usize)
                    .expect("schema-checked native rejection index is in the admission plan");
                let ucode_sha256 = generation.text_sha256.as_bytes();
                if !transaction.unchanged() {
                    drop(context);
                    drop(transaction);
                    self.invalidate_native_state();
                    return Err(RenderError::Backend {
                        backend: EVIDENCE_BACKEND,
                        reason: format!(
                            "native RT64 mutated guest memory during precommit NeedsLle for generation {rejected_generation}"
                        ),
                    });
                }
                transaction.commit();
                context.restore();
                return Err(RenderError::RequiresLle { ucode_sha256 });
            }
        };

        if let Err(error) = validate_complete_result(
            &admission_plan.plan.entry(),
            &native_result,
            expected_full_sync_count,
        ) {
            drop(context);
            drop(transaction);
            self.invalidate_native_state();
            return Err(error);
        }

        let evidence = Rt64F3dzex2CharacterizationEvidence {
            variant,
            planned_generation_count: native_result.planned_generation_count,
            observed_generation_count: native_result.observed_generation_count,
            full_sync_count: native_result.full_sync_count,
            initial_ucode: Rt64F3dzex2UcodeAddresses {
                text: native_result.initial_ucode_addresses.0,
                data: native_result.initial_ucode_addresses.1,
            },
            final_ucode: Rt64F3dzex2UcodeAddresses {
                text: native_result.final_ucode_addresses.0,
                data: native_result.final_ucode_addresses.1,
            },
        };
        transaction.commit();
        context.restore();
        Ok(evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_is_derived_from_live_entry_images() {
        const TEXT: u32 = 0x1000;
        const DATA: u32 = 0x3000;
        let logical_text = (0..fn64_runtime::RSP_MEMORY_BANK_SIZE)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let logical_data = (0..RT64_GBI_DATA_RECOGNITION_BYTES)
            .map(|index| (index % 239) as u8)
            .collect::<Vec<_>>();
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(TEXT), &logical_text);
            view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(DATA), &logical_data);
        }
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &logical_text,
            )
            .unwrap();
        let task = OsTask {
            task_type: fn64_render::M_GFXTASK,
            ucode: TEXT,
            ucode_size: fn64_runtime::RSP_MEMORY_BANK_SIZE as u32,
            ucode_data: DATA,
            ucode_data_size: logical_data.len() as u32,
            data_ptr: 0x6000,
            data_size: 8,
            ..OsTask::default()
        };

        let (text_address, data_address, raw_window) = capture_entry(&rdram, &task).unwrap();
        assert_eq!(text_address, TEXT);
        assert_eq!(data_address, DATA);
        assert_eq!(raw_window.text.len(), RT64_GBI_TEXT_RECOGNITION_BYTES);
        assert_eq!(raw_window.data.len(), RT64_GBI_DATA_RECOGNITION_BYTES);
        let admission = build_admission(
            &rdram,
            &rsp_memory,
            &task,
            text_address,
            data_address,
            raw_window,
            F3dzex2Variant::NoNFifo208I,
        )
        .unwrap();

        assert_eq!(admission.plan.len(), 1);
        let entry = admission.plan.entry();
        assert_eq!(entry.source, TaskAdmissionSource::TaskEntry);
        assert_eq!(entry.text_address, TEXT);
        assert_eq!(entry.data_address, DATA);
        assert_eq!(entry.text_sha256, UcodeDigest::from_text(&logical_text));
        assert_eq!(entry.data.bytes, logical_data.len() as u32);
        let expected_data_sha256: [u8; 32] = Sha256::digest(&logical_data).into();
        assert_eq!(entry.data.sha256, expected_data_sha256);
        assert_eq!(
            entry.ucode,
            TaskAdmissionUcode::F3dzex2(F3dzex2Variant::NoNFifo208I)
        );
        assert!(admission.plan.self_loads().is_empty());
    }

    #[test]
    fn preflight_rejects_before_mutation() {
        const TEXT: u32 = 0x1000;
        const DATA: u32 = 0x3000;
        let task = OsTask {
            task_type: fn64_render::M_GFXTASK,
            ucode: TEXT,
            ucode_size: fn64_runtime::RSP_MEMORY_BANK_SIZE as u32,
            ucode_data: DATA,
            ucode_data_size: RT64_GBI_DATA_RECOGNITION_BYTES as u32,
            data_ptr: 0x6000,
            data_size: 8,
            ..OsTask::default()
        };
        let rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let rdram_before = rdram.clone();
        let rsp_before = rsp_memory.clone();

        assert!(matches!(
            prepare_admission(&rdram, &rsp_memory, &task),
            Err(RenderError::RequiresLle { .. })
        ));
        assert_eq!(rdram, rdram_before);
        assert_eq!(rsp_memory, rsp_before);

        let short_window = OsTask {
            ucode: (fn64_runtime::rdram::DEFAULT_RDRAM_SIZE - RT64_GBI_TEXT_RECOGNITION_BYTES + 8)
                as u32,
            ..task
        };
        assert!(matches!(
            capture_entry(&rdram, &short_window),
            Err(RenderError::InvalidTaskBounds { .. })
        ));
        let malformed = OsTask {
            ucode_data_size: 7,
            ..task
        };
        assert!(matches!(
            capture_entry(&rdram, &malformed),
            Err(RenderError::Backend {
                backend: EVIDENCE_BACKEND,
                ..
            })
        ));
        for malformed in [
            OsTask {
                task_type: 0,
                ..task
            },
            OsTask {
                ucode: 0x8000_1000,
                ..task
            },
            OsTask {
                ucode_size: 0x0ff8,
                ..task
            },
        ] {
            assert!(matches!(
                capture_entry(&rdram, &malformed),
                Err(RenderError::Backend {
                    backend: EVIDENCE_BACKEND,
                    ..
                })
            ));
        }
        assert!(matches!(
            capture_entry(&rdram[..rdram.len() - 1], &task),
            Err(RenderError::Backend {
                backend: EVIDENCE_BACKEND,
                ..
            })
        ));

        let (text_address, data_address, raw_window) = capture_entry(&rdram, &task).unwrap();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &[0x55; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            )
            .unwrap();
        let mismatched_rsp_before = rsp_memory.clone();
        assert!(matches!(
            build_admission(
                &rdram,
                &rsp_memory,
                &task,
                text_address,
                data_address,
                raw_window,
                F3dzex2Variant::NoNFifo206H,
            ),
            Err(RenderError::RequiresLle { .. })
        ));
        assert_eq!(rdram, rdram_before);
        assert_eq!(rsp_memory, mismatched_rsp_before);
    }

    #[test]
    fn complete_result_must_preserve_entry_identity_and_expected_full_sync_count() {
        let entry = TaskAdmissionGeneration {
            source: TaskAdmissionSource::TaskEntry,
            text_address: 0x1000,
            data_address: 0x3000,
            text_sha256: UcodeDigest::from_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]),
            data: MicrocodeDataImageIdentity {
                bytes: RT64_GBI_DATA_RECOGNITION_BYTES as u32,
                sha256: [0; 32],
            },
            ucode: TaskAdmissionUcode::F3dzex2(F3dzex2Variant::NoNFifo208I),
        };
        let result = ffi::NativeTaskResult {
            dp_full_sync: fn64_render::DpFullSyncStatus::NotReached,
            full_sync_count: 0,
            initial_ucode_addresses: (entry.text_address, entry.data_address),
            final_ucode_addresses: (entry.text_address, entry.data_address),
            planned_generation_count: 1,
            observed_generation_count: 1,
            plan_sha256: [0; 32],
        };
        validate_complete_result(&entry, &result, 0).unwrap();
        assert!(validate_complete_result(
            &entry,
            &ffi::NativeTaskResult {
                final_ucode_addresses: (0x5000, 0x7000),
                ..result
            },
            0,
        )
        .is_err());
        assert!(validate_complete_result(&entry, &result, 1).is_err());
    }
}
