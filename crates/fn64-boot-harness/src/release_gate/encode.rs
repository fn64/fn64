#![allow(clippy::module_inception)]
use super::*;

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

pub(crate) fn decode_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut out = [0; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)? as u8;
        let low = (pair[1] as char).to_digit(16)? as u8;
        out[index] = (high << 4) | low;
    }
    Some(out)
}

pub(crate) fn recompute_digest_root(
    guest_cycle: u64,
    artifacts: &[ArtifactDigest],
) -> Result<String, GateError> {
    let mut root = Sha256::new();
    root.update(REPORT_SCHEMA.as_bytes());
    root.update(guest_cycle.to_be_bytes());
    for artifact in artifacts {
        root.update(artifact.kind.tag());
        root.update(artifact.bytes.to_be_bytes());
        root.update(
            decode_sha256(&artifact.sha256)
                .ok_or(GateError::InvalidReportSha256("digest.artifacts[].sha256"))?,
        );
    }
    Ok(hex(&root.finalize()))
}

pub(crate) fn validate_artifact_observation_bytes(
    digest: &DeterministicDigest,
    observations: &ReleaseObservationGeometry,
) -> Result<(), GateError> {
    let framebuffer = &digest.artifacts[0];
    let expected_framebuffer = observations
        .expected_framebuffer_artifact_bytes()
        .map_err(GateError::InvalidObservationGeometry)?;
    if framebuffer.bytes != expected_framebuffer {
        return Err(GateError::ArtifactObservationByteMismatch {
            kind: ArtifactKind::Framebuffer,
            expected: expected_framebuffer,
            observed: framebuffer.bytes,
        });
    }
    let memory = &digest.artifacts[2];
    if memory.bytes != observations.memory.payload_bytes {
        return Err(GateError::ArtifactObservationByteMismatch {
            kind: ArtifactKind::Memory,
            expected: observations.memory.payload_bytes,
            observed: memory.bytes,
        });
    }
    Ok(())
}

pub(crate) fn validate_closure_paths(paths: &[ClosurePath]) -> Result<(), GateError> {
    let mut names = std::collections::BTreeSet::new();
    for path in paths {
        if path.name.is_empty() {
            return Err(GateError::EmptyPathName);
        }
        if !names.insert(path.name.as_str()) {
            return Err(GateError::DuplicateClosurePath(path.name.clone()));
        }
        let valid = match path.status {
            ClosurePathStatus::Unexercised => path.observations == 0 && path.unsupported.is_empty(),
            ClosurePathStatus::ExercisedZeroUnsupported => {
                path.observations > 0 && path.unsupported.is_empty()
            }
            ClosurePathStatus::ExercisedUnsupported => {
                path.observations > 0
                    && !path.unsupported.is_empty()
                    && path.observations >= path.unsupported.len() as u64
            }
        };
        if !valid {
            let detail = match path.status {
                ClosurePathStatus::Unexercised => {
                    "unexercised requires zero observations and no unsupported events"
                }
                ClosurePathStatus::ExercisedZeroUnsupported => {
                    "zero-unsupported requires a positive observation count and no unsupported events"
                }
                ClosurePathStatus::ExercisedUnsupported => {
                    "unsupported requires a positive count, at least one event, and count >= event count"
                }
            };
            return Err(GateError::InvalidClosurePath {
                name: path.name.clone(),
                detail,
            });
        }
    }
    Ok(())
}

pub(crate) fn validate_canonical_closure_order(paths: &[ClosurePath]) -> Result<(), GateError> {
    if let Some(pair) = paths.windows(2).find(|pair| pair[0].name >= pair[1].name) {
        return Err(GateError::NonCanonicalClosureOrder {
            previous: pair[0].name.clone(),
            next: pair[1].name.clone(),
        });
    }
    Ok(())
}

pub(super) fn validate_rsp_rdp_closure(
    paths: &[ClosurePath],
    evidence: &RspRdpEvidence,
) -> Result<(), GateError> {
    let graphics_exercised = paths.iter().any(|path| {
        path.name == "rsp.graphics-task"
            && matches!(
                path.status,
                ClosurePathStatus::ExercisedZeroUnsupported
                    | ClosurePathStatus::ExercisedUnsupported
            )
    });
    let recognition_observed = evidence.ordered.iter().any(|event| {
        matches!(
            event.observation,
            RspRdpObservationKindEvidence::MicrocodeRecognition { .. }
        )
    });
    if graphics_exercised && !recognition_observed {
        return Err(GateError::MissingGraphicsMicrocodeRecognition);
    }
    Ok(())
}

pub(super) fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(super) fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(super) fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(super) fn encode_guest_device_snapshot(out: &mut Vec<u8>, snapshot: DeviceSnapshot) {
    push_u64(out, snapshot.now.get());
    for value in [
        snapshot.pi_dram_addr.offset(),
        snapshot.pi_cart_addr,
        snapshot.pi_status,
        snapshot.ai_status,
        snapshot.ai_length,
        snapshot.si_dram_addr.offset(),
        snapshot.si_status,
        snapshot.vi_current,
        snapshot.vi_intr,
        snapshot.vi_v_sync,
    ] {
        push_u32(out, value);
    }
    push_u32(out, snapshot.ai_dram_addr.offset());
    for value in [
        snapshot.ai_control,
        snapshot.ai_dacrate,
        snapshot.ai_bitrate,
    ] {
        push_u32(out, value);
    }
    push_u32(out, snapshot.tv_type.map_or(u32::MAX, |tv| tv as u32));
    push_u64(
        out,
        snapshot
            .vi_field_interval
            .map_or(u64::MAX, |cycles| cycles.get()),
    );
    out.push(snapshot.sp_busy as u8);
    push_u32(out, snapshot.sp_status);
    push_u32(
        out,
        u32::try_from(snapshot.sp_mem_addr.offset()).expect("RSP offset fits u32"),
    );
    push_u32(out, snapshot.sp_dram_addr.offset());
    push_u64(out, snapshot.sp_imem_generation);
    out.push(snapshot.dp_busy as u8);
    for value in [
        snapshot.dpc_start,
        snapshot.dpc_end,
        snapshot.dpc_current,
        snapshot.dpc_status,
        snapshot.dpc_clock,
        snapshot.dpc_busy,
        snapshot.dpc_pipe_busy,
        snapshot.dpc_tmem_busy,
    ] {
        push_u32(out, value);
    }
    match snapshot.pending_dpc {
        Some(submission) => {
            out.push(1);
            encode_dpc_submission(out, submission);
        }
        None => out.push(0),
    }
    for value in [snapshot.mi_pending, snapshot.mi_mask] {
        push_u32(out, value);
    }
    for timing in [snapshot.pi_domain1, snapshot.pi_domain2] {
        out.extend_from_slice(&[
            timing.latency,
            timing.pulse_width,
            timing.page_size,
            timing.release,
        ]);
    }
}

pub(super) fn encode_pi_request(out: &mut Vec<u8>, request: fn64_runtime::PiDmaRequest) {
    out.push(match request.direction {
        DmaDirection::ToRdram => 0,
        DmaDirection::FromRdram => 1,
    });
    push_u32(out, request.dram_addr.offset());
    encode_pi_device_address(out, request.device);
    push_u32(out, request.len);
}

pub(super) fn encode_pi_device_address(out: &mut Vec<u8>, device: PiDeviceAddress) {
    match device {
        PiDeviceAddress::RomOffset(offset) => {
            out.push(0);
            push_u32(out, offset);
        }
        PiDeviceAddress::SramOffset(offset) => {
            out.push(1);
            push_u32(out, offset);
        }
    }
}

pub(super) fn encode_ai_request(out: &mut Vec<u8>, request: fn64_runtime::AiDmaRequest) {
    push_u32(out, request.dram_addr.offset());
    push_u32(out, request.len);
    push_u32(out, request.sample_rate_hz);
}

pub(super) fn encode_dpc_submission(out: &mut Vec<u8>, submission: fn64_runtime::DpcSubmission) {
    push_u64(out, submission.token);
    out.push(match submission.source {
        fn64_runtime::DpcSubmissionSource::Rdram => 0,
        fn64_runtime::DpcSubmissionSource::Dmem => 1,
    });
    push_u32(out, submission.start);
    push_u32(out, submission.end);
}

pub(super) fn encode_si_request(out: &mut Vec<u8>, request: fn64_runtime::SiDmaRequest) {
    out.push(match request.kind {
        SiDmaKind::DramToPif => 0,
        SiDmaKind::PifToDram => 1,
        SiDmaKind::ControllerQuery => 2,
        SiDmaKind::ControllerRead => 3,
    });
    push_u32(out, request.dram_addr.offset());
}

pub(super) fn encode_sp_dma_request(out: &mut Vec<u8>, request: fn64_runtime::SpDmaRequest) {
    out.push(match request.direction {
        SpDmaDirection::RdramToRsp => 0,
        SpDmaDirection::RspToRdram => 1,
    });
    push_u32(
        out,
        u32::try_from(request.mem_addr.offset()).expect("RSP DMA offset fits u32"),
    );
    push_u32(out, request.dram_addr.offset());
    push_u32(out, request.encoded_len);
}

pub(super) fn push_option_u32(out: &mut Vec<u8>, value: Option<u32>) {
    match value {
        Some(value) => {
            out.push(1);
            push_u32(out, value);
        }
        None => out.push(0),
    }
}

pub(super) fn push_option_u16(out: &mut Vec<u8>, value: Option<u16>) {
    match value {
        Some(value) => {
            out.push(1);
            push_u16(out, value);
        }
        None => out.push(0),
    }
}

pub(super) fn push_option_bool(out: &mut Vec<u8>, value: Option<bool>) {
    out.push(match value {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    });
}

pub(super) fn encode_port_state(state: PortState) -> u8 {
    match state {
        PortState::StandardControllerNoPak => 0,
        PortState::StandardControllerControllerPak => 1,
        PortState::StandardControllerRumblePak => 2,
        PortState::StandardControllerTransferPak => 3,
        PortState::VoiceRecognitionUnit => 4,
        PortState::Absent => 5,
    }
}

pub(super) fn encode_controller_pak(out: &mut Vec<u8>, snapshot: fn64_runtime::ControllerPakEvidenceSnapshot) {
    out.extend_from_slice(&[snapshot.bank_count, snapshot.active_bank]);
    for note in snapshot.notes {
        match note {
            Some(note) => {
                out.push(1);
                push_u16(out, note.key.company_code);
                push_u32(out, note.key.game_code);
                out.extend_from_slice(&note.key.game_name);
                out.extend_from_slice(&note.key.ext_name);
                push_u64(out, note.pages.len() as u64);
                for page in note.pages {
                    push_u16(out, page);
                }
            }
            None => out.push(0),
        }
    }
    push_bytes(out, &snapshot.raw);
}

pub(super) fn encode_game_boy_mapper(out: &mut Vec<u8>, mapper: GameBoyMapperEvidenceSnapshot) {
    match mapper {
        GameBoyMapperEvidenceSnapshot::RomOnly => out.push(0),
        GameBoyMapperEvidenceSnapshot::Mbc1 {
            ram_enabled,
            rom_low5,
            upper2,
            ram_mode,
        } => out.extend_from_slice(&[1, ram_enabled as u8, rom_low5, upper2, ram_mode as u8]),
        GameBoyMapperEvidenceSnapshot::Mbc2 {
            ram_enabled,
            rom_bank,
        } => out.extend_from_slice(&[2, ram_enabled as u8, rom_bank]),
        GameBoyMapperEvidenceSnapshot::Mbc3 {
            timer_present,
            ram_enabled,
            rom_bank,
            select,
            latch_armed,
            rtc,
            latched_rtc,
            subsecond_cycles,
        } => {
            out.extend_from_slice(&[
                3,
                timer_present as u8,
                ram_enabled as u8,
                rom_bank,
                select,
                latch_armed as u8,
            ]);
            out.extend_from_slice(&rtc);
            out.extend_from_slice(&latched_rtc);
            push_u64(out, subsecond_cycles);
        }
        GameBoyMapperEvidenceSnapshot::Mbc5 {
            ram_enabled,
            rom_bank,
            ram_bank,
            rumble_variant,
        } => {
            out.extend_from_slice(&[4, ram_enabled as u8]);
            push_u16(out, rom_bank);
            out.extend_from_slice(&[ram_bank, rumble_variant as u8]);
        }
    }
}

pub(super) fn encode_transfer_pak(out: &mut Vec<u8>, snapshot: fn64_runtime::TransferPakEvidenceSnapshot) {
    push_u64(out, snapshot.now.get());
    out.extend_from_slice(&[
        snapshot.enabled as u8,
        snapshot.transfer_bank,
        snapshot.access_mode,
        snapshot.cartridge_pulled as u8,
        snapshot.reset_detected as u8,
    ]);
    match snapshot.cartridge {
        Some(cartridge) => {
            out.push(1);
            push_bytes(out, &cartridge.rom);
            push_bytes(out, &cartridge.ram);
            encode_game_boy_mapper(out, cartridge.mapper);
        }
        None => out.push(0),
    }
}

pub(super) fn encode_voice_data(out: &mut Vec<u8>, data: fn64_runtime::VoiceData) {
    for value in [
        data.warning,
        data.answer_num,
        data.voice_level,
        data.voice_sn,
        data.voice_time,
    ] {
        push_u16(out, value);
    }
    for value in data.answer.into_iter().chain(data.distance) {
        push_u16(out, value);
    }
}

pub(super) fn encode_voice_unit(out: &mut Vec<u8>, snapshot: fn64_runtime::VoiceEvidenceSnapshot) {
    out.extend_from_slice(&[snapshot.initialized as u8, snapshot.raw_init_step]);
    match snapshot.expected_words {
        Some(words) => out.extend_from_slice(&[1, words]),
        None => out.push(0),
    }
    push_u64(out, snapshot.words.len() as u64);
    for word in snapshot.words {
        push_bytes(out, &word);
    }
    push_bytes(out, &snapshot.mask);
    out.extend_from_slice(&[snapshot.analog_gain, snapshot.digital_gain, snapshot.status]);
    match snapshot.pending_result {
        Some(data) => {
            out.push(1);
            encode_voice_data(out, data);
        }
        None => out.push(0),
    }
}

pub(super) fn encode_vi_manager(out: &mut Vec<u8>, snapshot: fn64_runtime::ViEvidenceSnapshot) {
    push_option_u32(out, snapshot.mode_ptr);
    push_option_u32(out, snapshot.next_mode_ptr);
    out.push(snapshot.next_mode_resets_overrides as u8);
    for value in [
        snapshot.special_features,
        snapshot.next_special_features,
        snapshot.x_scale_bits,
        snapshot.y_scale_bits,
        snapshot.next_x_scale_bits,
        snapshot.next_y_scale_bits,
    ] {
        push_option_u32(out, value);
    }
    out.push(snapshot.blanked as u8);
    push_option_bool(out, snapshot.next_blanked);
    push_option_u16(out, snapshot.fade);
    match snapshot.next_fade {
        PendingViFade::Unchanged => out.push(0),
        PendingViFade::Disabled => out.push(1),
        PendingViFade::Factor(factor) => {
            out.push(2);
            push_u16(out, factor);
        }
    }
    out.push(snapshot.repeat_line as u8);
    push_option_bool(out, snapshot.next_repeat_line);
    push_option_u32(out, snapshot.current_framebuffer);
    push_option_u32(out, snapshot.next_framebuffer);
    push_u64(out, snapshot.swap_count);
    match snapshot.retrace_target {
        Some((queue, message)) => {
            out.push(1);
            push_u32(out, queue);
            push_u32(out, message);
        }
        None => out.push(0),
    }
    push_u32(out, snapshot.retrace_count);
    push_u32(out, snapshot.retrace_phase);
}

pub(super) fn encode_vi_mode(out: &mut Vec<u8>, mode: fn64_abi::PendingViModeEvidenceSnapshot) {
    for register in mode.registers {
        push_u32(out, register);
    }
    for field in mode.fields {
        for register in field {
            push_u32(out, register);
        }
    }
}

pub(super) fn encode_pfs_is_plug_transaction(
    out: &mut Vec<u8>,
    transaction: fn64_abi::PfsIsPlugTransactionEvidenceSnapshot,
) {
    push_u32(out, transaction.thread);
    push_u32(out, transaction.queue.offset());
    push_u32(out, transaction.message);
    push_u32(out, transaction.result_addr.offset());
    out.push(transaction.bitpattern);
}

pub(super) fn encode_runtime_peripherals(
    out: &mut Vec<u8>,
    snapshot: fn64_abi::RuntimePeripheralEvidenceSnapshot,
    bind_pending_host_interrupt_routes: bool,
) {
    let peripherals = snapshot.peripherals;
    encode_vi_manager(out, peripherals.vi);
    match peripherals.retrace {
        Some(retrace) => {
            out.push(1);
            push_u64(out, retrace.interval);
            push_u64(out, retrace.next_due);
        }
        None => out.push(0),
    }
    for state in peripherals.pif.ports {
        out.push(encode_port_state(state));
    }
    for input in peripherals.pif.inputs {
        push_u16(out, input.button);
        out.extend_from_slice(&[input.stick_x as u8, input.stick_y as u8]);
    }
    for active in peripherals.pif.rumble_on {
        out.push(active as u8);
    }
    for pak in peripherals.controller_paks {
        match pak {
            Some(pak) => {
                out.push(1);
                encode_controller_pak(out, pak);
            }
            None => out.push(0),
        }
    }
    for pak in peripherals.transfer_paks {
        match pak {
            Some(pak) => {
                out.push(1);
                encode_transfer_pak(out, pak);
            }
            None => out.push(0),
        }
    }
    for voice in peripherals.voice_units {
        match voice {
            Some(voice) => {
                out.push(1);
                encode_voice_unit(out, voice);
            }
            None => out.push(0),
        }
    }

    push_u64(out, snapshot.pending_pi_completions.len() as u64);
    for pending in snapshot.pending_pi_completions {
        encode_pi_request(out, pending.request);
        push_u64(out, pending.rdram_len);
        push_option_u32(out, pending.ret_queue.map(RdramAddr::offset));
        push_u32(out, pending.ret_mesg);
    }
    match snapshot.pending_si_completion {
        Some(pending) => {
            out.push(1);
            encode_si_request(out, pending.request);
            match pending.owner {
                fn64_abi::PendingSiCompletionOwnerEvidenceSnapshot::ProcessRdram { rdram_len } => {
                    out.push(0);
                    push_u64(out, rdram_len);
                }
                fn64_abi::PendingSiCompletionOwnerEvidenceSnapshot::OsEvent => out.push(1),
                fn64_abi::PendingSiCompletionOwnerEvidenceSnapshot::PfsIsPlug(transaction) => {
                    out.push(2);
                    encode_pfs_is_plug_transaction(out, transaction);
                }
            }
        }
        None => out.push(0),
    }
    if bind_pending_host_interrupt_routes {
        push_u64(out, snapshot.pending_host_interrupt_routes.len() as u64);
        for route in snapshot.pending_host_interrupt_routes {
            out.push(match route.source {
                fn64_runtime::InterruptSource::Sp => 0,
                fn64_runtime::InterruptSource::Si => 1,
                fn64_runtime::InterruptSource::Ai => 2,
                fn64_runtime::InterruptSource::Vi => 3,
                fn64_runtime::InterruptSource::Pi => 4,
                fn64_runtime::InterruptSource::Dp => 5,
            });
        }
    }
    push_u64(out, snapshot.completed_pfs_is_plug.len() as u64);
    for transaction in snapshot.completed_pfs_is_plug {
        encode_pfs_is_plug_transaction(out, transaction);
    }
    for mode in [snapshot.vi.pending_mode, snapshot.vi.active_mode] {
        match mode {
            Some(mode) => {
                out.push(1);
                encode_vi_mode(out, mode);
            }
            None => out.push(0),
        }
    }
    for value in [
        snapshot.vi.pending_control,
        snapshot.vi.pending_x_scale_bits,
        snapshot.vi.pending_y_scale_bits,
    ] {
        push_option_u32(out, value);
    }
    push_u32(out, snapshot.vi.active_x_scale_bits);
    push_u32(out, snapshot.vi.active_y_scale_bits);
}

pub(super) fn encode_resume(out: &mut Vec<u8>, resume: fn64_runtime::Resume) {
    match resume {
        fn64_runtime::Resume::Start => out.push(0),
        fn64_runtime::Resume::Continue => out.push(1),
        fn64_runtime::Resume::Delivered(message) => {
            out.push(2);
            push_u32(out, message);
        }
        fn64_runtime::Resume::SendUnblocked => out.push(3),
        fn64_runtime::Resume::WouldBlock => out.push(4),
    }
}

pub(super) fn encode_thread_state(state: fn64_runtime::ThreadState) -> u8 {
    match state {
        fn64_runtime::ThreadState::Stopped => 0,
        fn64_runtime::ThreadState::Runnable => 1,
        fn64_runtime::ThreadState::Running => 2,
        fn64_runtime::ThreadState::BlockedOnRecv => 3,
        fn64_runtime::ThreadState::BlockedOnSend => 4,
        fn64_runtime::ThreadState::Dead => 5,
    }
}

pub(super) fn encode_executor_control(
    out: &mut Vec<u8>,
    snapshot: fn64_runtime::ExecutorControlEvidenceSnapshot,
) {
    out.push(match snapshot.kernel_authority {
        fn64_runtime::KernelAuthorityEvidenceSnapshot::HostKernel => 0,
        fn64_runtime::KernelAuthorityEvidenceSnapshot::GuestKernel => 1,
    });
    encode_executor_control_v1_body(out, snapshot);
}

fn encode_executor_control_v1_body(
    out: &mut Vec<u8>,
    snapshot: fn64_runtime::ExecutorControlEvidenceSnapshot,
) {
    match snapshot.rdram {
        fn64_runtime::RdramRegistrationEvidenceSnapshot::Absent => out.push(0),
        fn64_runtime::RdramRegistrationEvidenceSnapshot::LegacyUnbounded => out.push(1),
        fn64_runtime::RdramRegistrationEvidenceSnapshot::Present { len } => {
            out.push(2);
            push_u64(out, len);
        }
    }
    push_u64(out, snapshot.threads.len() as u64);
    for thread in snapshot.threads {
        push_u32(out, thread.id);
        push_u32(out, thread.priority as u32);
        out.push(encode_thread_state(thread.state));
        out.push(thread.started as u8);
    }
    push_u64(out, snapshot.run_queue.len() as u64);
    for thread in snapshot.run_queue {
        push_u32(out, thread);
    }
    push_u64(out, snapshot.pending_resumes.len() as u64);
    for pending in snapshot.pending_resumes {
        push_u32(out, pending.thread);
        encode_resume(out, pending.resume);
    }
    push_u64(out, snapshot.queues.len() as u64);
    for queue in snapshot.queues {
        push_u32(out, queue.address.offset());
        push_u64(out, queue.queue.capacity);
        push_u64(out, queue.queue.first);
        push_u64(out, queue.queue.messages.len() as u64);
        for message in queue.queue.messages {
            push_u32(out, message);
        }
        push_u64(out, queue.queue.blocked_receivers.len() as u64);
        for receiver in queue.queue.blocked_receivers {
            push_u32(out, receiver.id);
            push_u32(out, receiver.priority as u32);
        }
        push_u64(out, queue.queue.blocked_senders.len() as u64);
        for sender in queue.queue.blocked_senders {
            push_u32(out, sender.id);
            push_u32(out, sender.priority as u32);
            push_u32(out, sender.msg);
            out.push(match sender.placement {
                fn64_runtime::SendPlacement::Tail => 0,
                fn64_runtime::SendPlacement::Head => 1,
            });
        }
    }
    push_u32(out, snapshot.timers.next_id);
    push_u64(out, snapshot.timers.firing_order.len() as u64);
    for timer in snapshot.timers.firing_order {
        push_u32(out, timer.id);
        push_u64(out, timer.deadline);
        push_u64(out, timer.interval);
        push_u32(out, timer.queue_addr.offset());
        push_u32(out, timer.msg);
        push_u32(out, timer.armed_by);
    }
    push_u64(out, snapshot.event_table.len() as u64);
    for registration in snapshot.event_table {
        push_u32(out, registration.event);
        push_u32(out, registration.queue_addr.offset());
        push_u32(out, registration.msg);
    }
    match snapshot.running {
        fn64_runtime::ExecutorRunningEvidenceSnapshot::Quiescent => out.push(0),
        fn64_runtime::ExecutorRunningEvidenceSnapshot::Active(thread) => {
            out.push(1);
            push_u32(out, thread);
        }
    }
    push_u64(out, snapshot.sim_time);
    push_u64(out, snapshot.os_time_bias);
    push_u32(out, snapshot.cp0_count);
    out.push(snapshot.cp0_count_phase);
    push_u32(out, snapshot.cp0_compare);
    out.push(snapshot.cp0_timer_pending as u8);
}

pub(super) fn encode_section_registry(
    out: &mut Vec<u8>,
    snapshot: fn64_runtime::SectionRegistryEvidenceSnapshot,
) {
    push_u64(out, snapshot.sections.len() as u64);
    for section in snapshot.sections {
        push_u32(out, section.rom_addr);
        push_u32(out, section.ram_addr);
        push_u32(out, section.size);
        push_u64(out, section.funcs.len() as u64);
        for function in section.funcs {
            push_u32(out, function.offset);
            push_u32(out, function.rom_size);
        }
    }
    push_u64(out, snapshot.loaded_sections.len() as u64);
    for section in snapshot.loaded_sections {
        push_u64(
            out,
            u64::try_from(section).expect("section index exceeds evidence wire"),
        );
    }
    push_u64(out, snapshot.runtime_loads.len() as u64);
    for load in snapshot.runtime_loads {
        push_u64(
            out,
            u64::try_from(load.section).expect("section index exceeds evidence wire"),
        );
        push_u32(out, load.load_vram);
    }
    match snapshot.static_mirror {
        Some(mirror) => {
            out.push(1);
            push_u64(
                out,
                u64::try_from(mirror.section).expect("section index exceeds evidence wire"),
            );
            push_u32(out, mirror.next_rom);
            push_u32(out, mirror.next_static_off);
        }
        None => out.push(0),
    }
    push_u64(out, snapshot.static_storage_ends.len() as u64);
    for storage in snapshot.static_storage_ends {
        push_u64(
            out,
            u64::try_from(storage.section).expect("section index exceeds evidence wire"),
        );
        push_u32(out, storage.end);
    }
}

pub(super) fn encode_os_task_header(out: &mut Vec<u8>, header: &fn64_runtime::OsTaskHeader) {
    for word in [
        header.task_type,
        header.flags,
        header.ucode_boot,
        header.ucode_boot_size,
        header.ucode,
        header.ucode_size,
        header.ucode_data,
        header.ucode_data_size,
        header.dram_stack,
        header.dram_stack_size,
        header.output_buff,
        header.output_buff_size,
        header.data_ptr,
        header.data_size,
        header.yield_data_ptr,
        header.yield_data_size,
    ] {
        push_u32(out, word);
    }
}

pub(super) fn encode_rsp_task_data_identity(
    out: &mut Vec<u8>,
    identity: Option<fn64_abi::RspTaskDataIdentityEvidenceSnapshot>,
) {
    match identity {
        Some(identity) => {
            out.push(1);
            push_u32(out, identity.rdram_offset);
            push_u32(out, identity.byte_len);
            out.extend_from_slice(&identity.sha256);
        }
        None => out.push(0),
    }
}

fn encode_rsp_dp_submission(
    out: &mut Vec<u8>,
    submission: &fn64_audio::rsp::runtime::RspDpSubmission,
) {
    use fn64_audio::rsp::runtime::RspDpCommandSource;

    push_u32(out, submission.start);
    push_u32(out, submission.end);
    match submission.source() {
        RspDpCommandSource::XbusBytes(bytes) => {
            out.push(1);
            push_bytes(out, bytes);
            push_u64(out, (bytes.len() / core::mem::size_of::<u32>()) as u64);
            for bytes in bytes.chunks_exact(4) {
                push_u32(
                    out,
                    u32::from_be_bytes(bytes.try_into().expect("four XBUS command bytes")),
                );
            }
        }
        RspDpCommandSource::RdramWords(words) => {
            out.push(0);
            push_bytes(out, &[]);
            push_u64(out, words.len() as u64);
            for word in words {
                push_u32(out, *word);
            }
        }
    }
}

macro_rules! encode_rsp_architectural_state {
    ($out:expr, $state:expr) => {{
        let out = $out;
        let state = $state;
        for register in state.gprs() {
            push_u32(out, *register);
        }
        for register in [
            state.dma_dram_address(),
            state.dma_mem_address(),
            state.jump_target(),
            state.resume_address(),
        ] {
            push_u32(out, register);
        }
        out.push(state.resume_delay() as u8);

        let vu = state.vu();
        for register in &vu.regs.r {
            for lane in register {
                push_u16(out, *lane as u16);
            }
        }
        for lane in 0..8 {
            let bits = (vu.acc.signed(lane) as u64) & 0x0000_ffff_ffff_ffff;
            out.extend_from_slice(&bits.to_be_bytes()[2..]);
        }
        push_u16(out, vu.flags.vco);
        push_u16(out, vu.flags.vcc);
        out.push(vu.flags.vce);
        push_u16(out, vu.div_in);
        out.push(vu.div_in_loaded as u8);
        push_u16(out, vu.div_out);

        push_u32(out, state.sp_status());
        out.push(state.sp_semaphore() as u8);
        for register in [
            state.dma_read_length(),
            state.dma_write_length(),
            state.dp_start(),
            state.dp_end(),
            state.dp_current(),
            state.dp_status(),
            state.dp_clock(),
            state.dp_busy(),
            state.dp_pipe_busy(),
            state.dp_tmem_busy(),
        ] {
            push_u32(out, register);
        }
        push_u64(out, state.dp_submissions().len() as u64);
        for submission in state.dp_submissions() {
            encode_rsp_dp_submission(out, submission);
        }
    }};
}

pub(super) fn encode_rsp_interpreter_state(
    out: &mut Vec<u8>,
    state: fn64_abi::RspInterpreterStateEvidenceSnapshot,
) {
    match state {
        fn64_abi::RspInterpreterStateEvidenceSnapshot::Reset => out.push(0),
        fn64_abi::RspInterpreterStateEvidenceSnapshot::Exact(state) => {
            out.push(1);
            encode_rsp_architectural_state!(out, &state);
        }
        fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibility(state) => {
            out.push(2);
            encode_rsp_architectural_state!(out, &state);
        }
        // Tags 3 and 4 are the task-owned encodings and must stay byte-for-byte
        // what they were before ownership became a typed enum: every retained
        // `device_state` digest and `report_sha256` from a task-driven run
        // depends on them. Raw SP kicks get appended tags 5 and 6 instead, so no
        // schema bump is needed and no existing report becomes unreproducible.
        fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable { owner } => {
            match owner {
                fn64_abi::RspInterpreterOwner::Task {
                    offset,
                    admission_generation,
                } => {
                    out.push(3);
                    push_u32(out, offset);
                    push_u64(out, admission_generation.get());
                }
                fn64_abi::RspInterpreterOwner::RawKick {
                    admission_generation,
                } => {
                    out.push(5);
                    push_u64(out, admission_generation.get());
                }
            }
        }
        fn64_abi::RspInterpreterStateEvidenceSnapshot::InFlight { owner } => match owner {
            fn64_abi::RspInterpreterOwner::Task {
                offset,
                admission_generation,
            } => {
                out.push(4);
                push_u32(out, offset);
                push_u64(out, admission_generation.get());
            }
            fn64_abi::RspInterpreterOwner::RawKick {
                admission_generation,
            } => {
                out.push(6);
                push_u64(out, admission_generation.get());
            }
        },
    }
}

pub(super) fn encode_abi_host(out: &mut Vec<u8>, snapshot: fn64_abi::AbiHostEvidenceSnapshot) {
    encode_abi_host_with_interrupt_routes(out, snapshot, true);
}

fn encode_abi_host_with_interrupt_routes(
    out: &mut Vec<u8>,
    snapshot: fn64_abi::AbiHostEvidenceSnapshot,
    bind_pending_host_interrupt_routes: bool,
) {
    encode_runtime_peripherals(
        out,
        snapshot.runtime_peripherals,
        bind_pending_host_interrupt_routes,
    );
    out.push(snapshot.controller_manager.initialized as u8);
    out.push(snapshot.controller_manager.channels);
    match snapshot.flash.write_buffer {
        Some(bytes) => {
            out.push(1);
            out.extend_from_slice(&bytes);
        }
        None => out.push(0),
    }
    out.push(snapshot.flash.erase_complete as u8);
    out.push(snapshot.flash.status);
    push_u32(out, snapshot.flash.identity.flash_type);
    push_u32(out, snapshot.flash.identity.flash_maker);
    encode_section_registry(out, snapshot.sections);
    push_u64(out, snapshot.rsp_boot_images.len() as u64);
    for image in snapshot.rsp_boot_images {
        push_u32(out, image.rdram_offset);
        push_bytes(out, &image.bytes);
    }
    match snapshot.loaded_rsp_task {
        Some(task) => {
            out.push(1);
            push_u32(out, task.task_offset);
            push_u64(out, task.admission_generation);
            encode_os_task_header(out, &task.header);
            encode_rsp_task_data_identity(out, task.resumed_data_identity);
        }
        None => out.push(0),
    }
    push_u64(out, snapshot.rsp_task_lineages.len() as u64);
    for lineage in snapshot.rsp_task_lineages {
        push_u32(out, lineage.task_offset);
        push_u64(out, lineage.admission_generation);
        encode_os_task_header(out, &lineage.original_header);
        encode_rsp_task_data_identity(out, lineage.data_identity);
        out.push(match lineage.phase {
            fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::Running => 0,
            fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized => 1,
            fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded => 2,
        });
    }
    push_u64(out, snapshot.next_rsp_task_admission_generation);
    encode_rsp_interpreter_state(out, snapshot.rsp_interpreter_state);
    match snapshot.audio_task_execution {
        fn64_abi::AudioTaskExecutionPolicy::Unconfigured => out.push(0),
        fn64_abi::AudioTaskExecutionPolicy::Translated { artifact_sha256 } => {
            out.push(1);
            out.extend_from_slice(&artifact_sha256);
        }
        fn64_abi::AudioTaskExecutionPolicy::LleAccuracy => out.push(2),
        fn64_abi::AudioTaskExecutionPolicy::DiagnosticSkip => out.push(3),
    }
    out.push(snapshot.rom_installed as u8);
    match snapshot.installed_rom {
        Some(rom) => {
            out.push(1);
            push_u64(out, rom.byte_len);
            out.extend_from_slice(&rom.sha256);
        }
        None => out.push(0),
    }
    out.push(match snapshot.cartridge_save {
        fn64_abi::CartridgeSaveEvidenceSnapshot::Unidentified => 0,
        fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave => 1,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::Eeprom4k,
        ) => 2,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::Eeprom16k,
        ) => 3,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::SramBanked,
        ) => 4,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::FlashRam,
        ) => 5,
    });
    push_option_u32(out, snapshot.cart_rom_handle_vram);
    push_option_u32(out, snapshot.flash_handle_vram);
    match snapshot.leo_disk {
        Some(disk) => {
            out.push(1);
            push_u32(out, disk.handle_vram);
            out.extend_from_slice(&[disk.latency, disk.page_size, disk.release, disk.pulse_width]);
        }
        None => out.push(0),
    }
    push_u64(out, snapshot.thread_handles.len() as u64);
    for handle in snapshot.thread_handles {
        push_u32(out, handle.osthread_offset);
        push_u32(out, handle.executor_thread_id);
    }
    push_u64(out, snapshot.thread_guest_ids.len() as u64);
    for guest in snapshot.thread_guest_ids {
        push_u32(out, guest.executor_thread_id);
        push_u32(out, guest.guest_os_id);
    }
    push_u64(out, snapshot.timer_handles.len() as u64);
    for handle in snapshot.timer_handles {
        push_u32(out, handle.ostimer_offset);
        push_u32(out, handle.timer_id);
    }
    push_u32(out, snapshot.next_synthetic_thread_id);
    out.push(snapshot.registered_rdram.present as u8);
    push_u64(out, snapshot.registered_rdram.byte_len);
    out.push(match snapshot.debug_hardware {
        fn64_abi::DebugHardware::None => 0,
        fn64_abi::DebugHardware::Msp => 1,
        fn64_abi::DebugHardware::Kmc => 2,
        fn64_abi::DebugHardware::Isv => 3,
    });
}

#[cfg(feature = "recomp-rs")]
pub(super) fn encode_program_identity(
    out: &mut Vec<u8>,
    identity: fn64_cpu_runtime::ProgramIdentityEvidenceSnapshot,
) {
    out.extend_from_slice(&identity.identity.bytes());
    out.push(match identity.source {
        fn64_cpu_runtime::ProgramIdentitySource::CallerSupplied => 0,
        fn64_cpu_runtime::ProgramIdentitySource::CanonicalBlockProgramSha256 => 1,
    });
}

pub(super) fn encode_program(out: &mut Vec<u8>, snapshot: crate::ProgramEvidenceSnapshot) {
    match snapshot {
        crate::ProgramEvidenceSnapshot::NoProgram => out.push(0),
        crate::ProgramEvidenceSnapshot::UnidentifiedNativeProgram => out.push(1),
        crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(identity) => {
            out.push(2);
            out.extend_from_slice(&identity.bytes());
        }
        #[cfg(feature = "recomp-rs")]
        crate::ProgramEvidenceSnapshot::TypedRust(program) => match program {
            fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot::Function { identity } => {
                out.push(3);
                encode_program_identity(out, identity);
            }
            fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot::Block {
                program,
                dispatch_artifact_identity,
                instruction_budget,
                executable_regions,
                pending_executable_writes,
            } => {
                out.push(4);
                encode_program_identity(out, program.identity);
                push_u64(out, program.banks.len() as u64);
                for bank in program.banks {
                    push_u64(out, bank.id.get());
                    out.extend_from_slice(&bank.runner_artifact_identity.bytes());
                    push_u64(out, bank.spans.len() as u64);
                    for span in bank.spans {
                        push_u32(out, span.vram_start.get());
                        push_u64(out, span.words.len() as u64);
                        for word in span.words {
                            push_u32(out, word);
                        }
                    }
                }
                push_u64(out, program.physical_banks.len() as u64);
                for bank in program.physical_banks {
                    push_u64(out, bank.id.get());
                    push_u64(out, bank.spans.len() as u64);
                    for span in bank.spans {
                        push_u32(out, span.physical_start);
                        push_u64(out, span.words.len() as u64);
                        for word in span.words {
                            push_u32(out, word);
                        }
                    }
                }
                push_u64(out, program.mapped_aot.len() as u64);
                for block in program.mapped_aot {
                    push_u64(out, block.entry.bank.get());
                    push_u32(out, block.entry.pc.get());
                    push_u64(out, block.instructions.len() as u64);
                    for instruction in block.instructions {
                        push_u64(out, instruction.bank.get());
                        push_u32(out, instruction.physical_address);
                    }
                    push_u64(out, block.expected_words.len() as u64);
                    for word in block.expected_words {
                        push_u32(out, word);
                    }
                    out.extend_from_slice(&block.runner_artifact_identity.bytes());
                }
                out.extend_from_slice(&dispatch_artifact_identity.bytes());
                push_u32(out, instruction_budget);
                push_u64(out, executable_regions.len() as u64);
                for region in executable_regions {
                    push_u32(out, region.physical_start);
                    push_u32(out, region.physical_end);
                    push_u32(out, region.virtual_start.get());
                    push_u32(out, region.virtual_end.get());
                    push_u64(out, region.active_bank.get());
                    push_u64(out, region.active_generation);
                    push_u64(out, region.next_generation);
                    out.extend_from_slice(&region.builder_artifact_identity.bytes());
                    out.push(match region.activation {
                        fn64_abi::recompiled::ExecutableActivationEvidence::EagerPublication => 0,
                        fn64_abi::recompiled::ExecutableActivationEvidence::FetchBoundary => 1,
                    });
                }
                push_u64(out, pending_executable_writes.len() as u64);
                for write in pending_executable_writes {
                    push_u32(out, write.physical_start);
                    push_u32(out, write.physical_end);
                }
            }
        },
    }
}

pub(super) fn try_encode_device_component_v16(snapshot: DeviceEvidenceSnapshot) -> Result<Vec<u8>, GateError> {
    const DPC_COUNTER_MASK: u32 = 0x00ff_ffff;
    for (register, value) in [
        ("DPC_CLOCK", snapshot.guest.dpc_clock),
        ("DPC_BUFBUSY", snapshot.guest.dpc_busy),
        ("DPC_PIPEBUSY", snapshot.guest.dpc_pipe_busy),
        ("DPC_TMEM", snapshot.guest.dpc_tmem_busy),
    ] {
        if value & !DPC_COUNTER_MASK != 0 {
            return Err(GateError::NonCanonicalDpcCounter { register, value });
        }
    }
    let mut out = Vec::with_capacity(8 * 1024 + snapshot.save_bytes.as_ref().map_or(0, Vec::len));
    out.extend_from_slice(b"fn64.device-evidence.v19\0");
    encode_guest_device_snapshot(&mut out, snapshot.guest);
    push_bytes(&mut out, &snapshot.pi_timing_policy);

    match snapshot.pending_pi {
        Some(pending) => {
            out.push(1);
            push_u64(&mut out, pending.token);
            encode_pi_request(&mut out, pending.request);
        }
        None => out.push(0),
    }
    match snapshot.current_ai {
        Some(pending) => {
            out.push(1);
            push_u64(&mut out, pending.id.get());
            push_u64(&mut out, pending.token);
            encode_ai_request(&mut out, pending.request);
            push_u64(&mut out, pending.started_at.get());
            push_u64(&mut out, pending.deadline.get());
        }
        None => out.push(0),
    }
    match snapshot.queued_ai {
        Some(queued) => {
            out.push(1);
            push_u64(&mut out, queued.id.get());
            encode_ai_request(&mut out, queued.request);
        }
        None => out.push(0),
    }
    match snapshot.pending_dpc {
        Some(pending) => {
            out.push(1);
            encode_dpc_submission(&mut out, pending.submission);
            for value in [
                pending.rollback_start,
                pending.rollback_end,
                pending.rollback_current,
                pending.rollback_status,
            ] {
                push_u32(&mut out, value);
            }
        }
        None => out.push(0),
    }
    match snapshot.pending_si {
        Some(fn64_runtime::PendingSiSnapshot::Dma { token, request }) => {
            out.push(1);
            push_u64(&mut out, token);
            encode_si_request(&mut out, request);
        }
        Some(fn64_runtime::PendingSiSnapshot::PifControl { token, command }) => {
            out.push(2);
            push_u64(&mut out, token);
            out.push(match command {
                fn64_runtime::PifControlCommand::TerminateBoot => 0,
            });
        }
        None => out.push(0),
    }
    out.push(snapshot.si_dma_error as u8);
    push_u64(&mut out, snapshot.si_latency.get());
    push_u64(&mut out, snapshot.pif_control_latency.get());
    out.extend_from_slice(&snapshot.pif_ram);

    out.extend_from_slice(&snapshot.rsp_dmem);
    out.extend_from_slice(&snapshot.rsp_imem);
    for value in [snapshot.sp_rd_len, snapshot.sp_wr_len, snapshot.sp_pc] {
        push_u32(&mut out, value);
    }
    out.push(snapshot.sp_semaphore as u8);
    match snapshot.active_sp_dma {
        Some(pending) => {
            out.push(1);
            push_u64(&mut out, pending.token);
            encode_sp_dma_request(&mut out, pending.request);
        }
        None => out.push(0),
    }
    match snapshot.queued_sp_dma {
        Some(request) => {
            out.push(1);
            encode_sp_dma_request(&mut out, request);
        }
        None => out.push(0),
    }
    push_u64(&mut out, snapshot.sp_dma_setup_cycles.get());

    for value in snapshot.vi_registers {
        push_u32(&mut out, value);
    }
    push_u64(&mut out, snapshot.vi_epoch.get());
    for token in [
        snapshot.pending_vi_token,
        snapshot.pending_sp_token,
        snapshot.pending_dp_token,
    ] {
        match token {
            Some(token) => {
                out.push(1);
                push_u64(&mut out, token);
            }
            None => out.push(0),
        }
    }
    push_u64(&mut out, snapshot.scheduled_events.len() as u64);
    for event in snapshot.scheduled_events {
        push_u64(&mut out, event.at.get());
        push_u64(&mut out, event.sequence);
        push_u64(&mut out, event.token);
        out.push(match event.kind {
            ScheduledDeviceEventKind::Pi => 0,
            ScheduledDeviceEventKind::Ai => 1,
            ScheduledDeviceEventKind::Si => 2,
            ScheduledDeviceEventKind::PifControl => 7,
            ScheduledDeviceEventKind::SpDma => 3,
            ScheduledDeviceEventKind::Vi => 4,
            ScheduledDeviceEventKind::Sp => 5,
            ScheduledDeviceEventKind::Dp => 6,
        });
    }
    push_u64(&mut out, snapshot.next_event_sequence);
    push_u64(&mut out, snapshot.next_ai_dma_id);

    match snapshot.save_bytes {
        Some(bytes) => {
            out.push(1);
            push_bytes(&mut out, &bytes);
        }
        None => out.push(0),
    }
    match snapshot.pending_eeprom_write {
        Some(pending) => {
            out.push(1);
            push_u32(&mut out, pending.offset);
            out.extend_from_slice(&pending.data);
            push_u64(&mut out, pending.ready_at.get());
        }
        None => out.push(0),
    }
    Ok(out)
}

pub(super) fn encode_executor_control_component(
    snapshot: fn64_runtime::ExecutorControlEvidenceSnapshot,
) -> Vec<u8> {
    let mut out = Vec::new();
    encode_executor_control(&mut out, snapshot);
    out
}

fn encode_executor_control_component_v1(
    snapshot: fn64_runtime::ExecutorControlEvidenceSnapshot,
) -> Vec<u8> {
    let mut out = Vec::new();
    encode_executor_control_v1_body(&mut out, snapshot);
    out
}

pub(super) fn encode_abi_host_component(snapshot: fn64_abi::AbiHostEvidenceSnapshot) -> Vec<u8> {
    let mut out = Vec::new();
    encode_abi_host(&mut out, snapshot);
    out
}

fn encode_abi_host_component_v1(snapshot: fn64_abi::AbiHostEvidenceSnapshot) -> Vec<u8> {
    let mut out = Vec::new();
    encode_abi_host_with_interrupt_routes(&mut out, snapshot, false);
    out
}

pub(super) fn operational_component_sha256(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(OPERATIONAL_STATE_COMPONENT_DIGEST_SCHEMA_V1.as_bytes());
    digest.update([0]);
    digest.update(domain);
    digest.update([0]);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

/// Hash canonical release-gate state components independently for an
/// operational A/B comparison. This API cannot import program evidence or
/// construct any release report, digest artifact, or closure authority.
pub fn operational_state_component_digests_v1(
    snapshot: DeviceEvidenceSnapshot,
    executor: fn64_runtime::ExecutorControlEvidenceSnapshot,
    host: fn64_abi::AbiHostEvidenceSnapshot,
) -> Result<OperationalStateComponentDigestsV1, GateError> {
    let device = try_encode_device_component_v16(snapshot)?;
    // V1 predates typed kernel ownership. Preserve its exact wire while the
    // release report uses the current authority-bound executor component.
    let executor = encode_executor_control_component_v1(executor);
    let abi_host = encode_abi_host_component_v1(host);
    Ok(OperationalStateComponentDigestsV1 {
        device_sha256: operational_component_sha256(b"device", &device),
        executor_sha256: operational_component_sha256(b"executor", &executor),
        abi_host_sha256: operational_component_sha256(b"abi-host", &abi_host),
    })
}

#[cfg(test)]
mod rsp_dp_submission_tests {
    use super::*;
    use fn64_audio::rsp::runtime::RspDpSubmission;

    fn encode_legacy_shape(
        start: u32,
        end: u32,
        xbus: bool,
        payload: &[u8],
        words: &[u32],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        push_u32(&mut out, start);
        push_u32(&mut out, end);
        out.push(xbus as u8);
        push_bytes(&mut out, payload);
        push_u64(&mut out, words.len() as u64);
        for word in words {
            push_u32(&mut out, *word);
        }
        out
    }

    #[test]
    fn typed_dpc_sources_preserve_the_release_evidence_wire_shape() {
        let xbus_bytes = vec![
            0x11, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xcc, 0xdd,
        ];
        let xbus = RspDpSubmission::from_xbus_bytes(0x100, 0x108, xbus_bytes.clone());
        let mut encoded = Vec::new();
        encode_rsp_dp_submission(&mut encoded, &xbus);
        assert_eq!(
            encoded,
            encode_legacy_shape(
                0x100,
                0x108,
                true,
                &xbus_bytes,
                &[0x1122_3344, 0xaabb_ccdd],
            )
        );

        let words = vec![0x0123_4567, 0x89ab_cdef];
        let rdram = RspDpSubmission::from_rdram_words(0x200, 0x208, words.clone());
        encoded.clear();
        encode_rsp_dp_submission(&mut encoded, &rdram);
        assert_eq!(
            encoded,
            encode_legacy_shape(0x200, 0x208, false, &[], &words)
        );
    }
}
