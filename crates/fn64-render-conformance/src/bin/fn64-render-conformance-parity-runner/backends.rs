//! Per-backend execution: reference, RT64, wgpu, angrylion.

use super::*;

pub(crate) fn command_words(commands: &[(u32, u32)]) -> Vec<u32> {
    commands
        .iter()
        .flat_map(|&(word0, word1)| [word0, word1])
        .collect()
}

/// The seeded guest memory every backend starts from: STALE everywhere in the
/// target, GUARD immediately either side of it, and the command words at
/// `COMMAND_START`.
pub(crate) fn seeded(commands: &[(u32, u32)]) -> Vec<u8> {
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for index in 0..PIXEL_COUNT {
            view.write_u16(RdramAddr::from_offset(FRAMEBUFFER + index * 2), STALE);
        }
        view.write_u16(RdramAddr::from_offset(FRAMEBUFFER - 2), GUARD);
        view.write_u16(
            RdramAddr::from_offset(FRAMEBUFFER + FRAMEBUFFER_BYTES),
            GUARD,
        );
        // **The texture source, staged for every case.** A fill case never
        // reads it, so seeding it unconditionally costs nothing and keeps
        // `seeded` a single function of the command list. Written through the
        // same `write_u16` the framebuffer uses, so the guest byte-lane
        // mapping is applied once and in one place -- a raw `copy_from_slice`
        // here would stage the texels byte-swapped and every textured case
        // would report a texture defect that was really a runner defect.
        for (index, texel) in TEXTURE_TEXELS.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(TEXTURE_SOURCE + index as u32 * 2),
                *texel,
            );
        }
        // The wide (`line = 2`) source, staged the same way and for the same
        // reason: unconditionally, so `seeded` stays a single function of the
        // command list, and through `write_u16` so the guest byte-lane
        // mapping is applied in exactly one place.
        for (index, texel) in WIDE_TEXELS.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(WIDE_SOURCE + index as u32 * 2),
                *texel,
            );
        }
        // Stage the same synthetic bar on every source row used by the odd
        // base and even-low-T control. Keeping these bytes fixed means the
        // control changes only the command's tile/load origin.
        for source_y in (SKEW_LOW_T_ODD - 1)..(SKEW_LOW_T_ODD + SKEW_HEIGHT) {
            for x in 0..SKEW_WIDTH {
                let source_index = source_y * SKEW_WIDTH + x;
                view.write_u16(
                    RdramAddr::from_offset(SKEW_SOURCE + source_index * 2),
                    skew_texel(x),
                );
            }
        }
        // The CI4 palette, RGBA16 like every other texel image.
        for (index, entry) in PALETTE.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(PALETTE_SOURCE + index as u32 * 2),
                *entry,
            );
        }
        // The CI4 index image: two 4-bit indices per byte, high nibble first,
        // written as logical guest bytes so the `^3` lane map is applied once
        // -- the same reason every other source here goes through a view
        // rather than a raw slice write.
        let packed: Vec<u8> = CI_INDICES
            .chunks_exact(2)
            .map(|pair| (pair[0] << 4) | (pair[1] & 0xf))
            .collect();
        view.write_logical_bytes(RdramAddr::from_offset(CI_SOURCE), &packed);
        view.write_logical_bytes(RdramAddr::from_offset(CI8_SOURCE), &CI8_INDICES);
        // loadblock-deep slice: an independent RGBA16 strip and CI8 index
        // strip, staged the same way as the sources above, so its LoadBlock
        // DxT cases read real bytes rather than zeroed RDRAM.
        for (index, texel) in LOADBLOCK_DEEP_RGBA16_TEXELS.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(LOADBLOCK_DEEP_RGBA16_SOURCE + index as u32 * 2),
                *texel,
            );
        }
        view.write_logical_bytes(
            RdramAddr::from_offset(LOADBLOCK_DEEP_CI8_SOURCE),
            &LOADBLOCK_DEEP_CI8_INDICES,
        );
        for index in 0u16..=255 {
            view.write_u16(
                RdramAddr::from_offset(CI8_PALETTE_SOURCE + u32::from(index) * 2),
                ci8_palette_entry(index as u8),
            );
        }
        // -------------------------------------------------------------
        // Track-B fan-out pass 2 staging.
        // -------------------------------------------------------------

        // lod-mip slice: the half-size mip level (tile 1).
        for (index, texel) in MIP1_TEXELS.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(MIP1_SOURCE + index as u32 * 2),
                *texel,
            );
        }

        // tlut-palette-deep slice: two-bank CI4 palette + index image.
        for (index, entry) in PALETTE_BANK0.iter().chain(PALETTE_BANK1.iter()).enumerate() {
            view.write_u16(
                RdramAddr::from_offset(PALETTE_BANK_TLUT_SOURCE + index as u32 * 2),
                *entry,
            );
        }
        let bank_packed: Vec<u8> = PALETTE_BANK_CI4_INDICES
            .chunks_exact(2)
            .map(|pair| (pair[0] << 4) | (pair[1] & 0xf))
            .collect();
        view.write_logical_bytes(
            RdramAddr::from_offset(PALETTE_BANK_CI4_SOURCE),
            &bank_packed,
        );

        // tlut-palette-deep slice: full 0..255 CI8 ramp + its 256-entry TLUT.
        let full_range_indices: Vec<u8> = (0..CI8_FULL_RANGE_WIDTH)
            .map(ci8_full_range_index)
            .collect();
        view.write_logical_bytes(
            RdramAddr::from_offset(CI8_FULL_RANGE_SOURCE),
            &full_range_indices,
        );
        for index in 0u16..=255 {
            view.write_u16(
                RdramAddr::from_offset(CI8_FULL_RANGE_TLUT_SOURCE + u32::from(index) * 2),
                ci8_full_range_palette_entry(index as u8),
            );
        }

        // tlut-palette-deep slice: tlut_type RGBA16-vs-IA16 twin.
        view.write_logical_bytes(
            RdramAddr::from_offset(TLUT_TYPE_CI8_SOURCE),
            &TLUT_TYPE_INDICES,
        );
        for (slot, &index) in TLUT_TYPE_INDICES.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(TLUT_TYPE_TLUT_SOURCE + u32::from(index) * 2),
                TLUT_TYPE_ENTRIES[slot],
            );
        }

        // tlut-palette-deep slice: nonzero-origin LoadTlut source array.
        for i in 0..(PALETTE_ORIGIN_TEXEL_OFFSET + PALETTE_ORIGIN_ENTRIES) {
            let value = if i < PALETTE_ORIGIN_TEXEL_OFFSET {
                0x0843
            } else {
                PALETTE_ORIGIN_TLUT_ENTRIES[(i - PALETTE_ORIGIN_TEXEL_OFFSET) as usize]
            };
            view.write_u16(RdramAddr::from_offset(PALETTE_ORIGIN_SOURCE + i * 2), value);
        }
        view.write_logical_bytes(
            RdramAddr::from_offset(PALETTE_ORIGIN_CI8_SOURCE),
            &PALETTE_ORIGIN_CI8_INDICES,
        );

        for (address, bytes) in [
            (RGBA32_SOURCE, RGBA32_BYTES.as_slice()),
            (IA8_SOURCE, IA8_BYTES.as_slice()),
            (IA4_SOURCE, IA4_BYTES.as_slice()),
            (IA16_SOURCE, IA16_BYTES.as_slice()),
            (I4_SOURCE, I4_BYTES.as_slice()),
            (I8_SOURCE, I8_BYTES.as_slice()),
            (YUV16_SOURCE, YUV16_BYTES.as_slice()),
        ] {
            view.write_logical_bytes(RdramAddr::from_offset(address), bytes);
        }
    }
    for (index, &(word0, word1)) in commands.iter().enumerate() {
        let offset = COMMAND_START as usize + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    rdram
}

pub(crate) fn observation_bytes(rdram: &[u8]) -> Vec<u8> {
    rdram[FRAMEBUFFER as usize..(FRAMEBUFFER + FRAMEBUFFER_BYTES) as usize].to_vec()
}

pub(crate) fn command_end(commands: &[(u32, u32)]) -> u32 {
    COMMAND_START + (commands.len() as u32) * 8
}

/// The reference backend's committed guest framebuffer.
pub(crate) fn reference_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let mut rdram = seeded(commands);
    let mut backend = ReferenceBackend::default();
    if let Err(error) = backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT)) {
        return Err(error.to_string());
    }
    match backend.process_rdp_commands(
        &mut rdram,
        COMMAND_START,
        command_end(commands),
        FRAMEBUFFER,
        true,
    ) {
        Ok(fn64_render::FrameStatus::Complete) => Ok(observation_bytes(&rdram)),
        Ok(status) => Err(format!("nonterminal status {status:?}")),
        Err(error) => Err(error.to_string()),
    }
}

/// Preserve the RT64/wgpu differential when the diagnostic-only reference
/// lane loudly rejects a state combination. Reference output never enters a
/// verdict, so converting its trap to a reported refusal changes no authority
/// claim and keeps the remaining corpus rows observable.
pub(crate) fn reference_outcome(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    match std::panic::catch_unwind(|| reference_bytes(commands)) {
        Ok(outcome) => outcome,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            Err(format!("reference trapped: {message}"))
        }
    }
}

/// The concrete graphics API RT64 can actually create a device with on this
/// host.
///
/// `RenderGraphicsApi::Automatic` is deliberately NOT used: this runner's
/// verdicts are only meaningful against a device whose backend is known, and
/// `Rt64Backend` refuses a live device whose API disagrees with an explicit
/// request (`graphics_api_matches_request`), which is the check that names
/// the backend in the report. Naming the API per platform keeps that check
/// live instead of accepting whatever RT64 happened to resolve.
///
/// Requesting Metal on Linux is not merely unsupported, it is a silent
/// gate hole: RT64's `Application::setup` reaches its `InvalidGraphicsAPI`
/// return only AFTER creating the application window, so every case would
/// refuse with "Metal is not supported on this platform" and the checker
/// would see 33 refusals rather than a measurement.
pub(crate) const fn rt64_graphics_api() -> RenderGraphicsApi {
    if cfg!(target_os = "macos") {
        RenderGraphicsApi::Metal
    } else if cfg!(target_os = "windows") {
        RenderGraphicsApi::D3d12
    } else {
        RenderGraphicsApi::Vulkan
    }
}

/// RT64's committed guest framebuffer -- the oracle's answer.
///
/// RT64 is created once per case rather than once per sweep: the deferred
/// history runner's state machine shows RT64 carries per-frame state across
/// submissions, and a shared backend would let case N's history leak into
/// case N+1's answer.
pub(crate) fn rt64_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let mut rdram = seeded(commands);
    let runtime = RenderRuntimeSettings {
        graphics_api: rt64_graphics_api(),
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)
            .map_err(|error| error.to_string())?,
        idle_work_active: false,
        developer_mode: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime);
    if let Err(error) = backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT)) {
        return Err(error.to_string());
    }
    match backend.process_rdp_commands(
        &mut rdram,
        COMMAND_START,
        command_end(commands),
        FRAMEBUFFER,
        true,
    ) {
        Ok(_) => Ok(observation_bytes(&rdram)),
        Err(error) => Err(error.to_string()),
    }
}

/// fn64's shipping wgpu backend, copied back exactly the way production
/// copies it.
///
/// `device_bytes` are flat big-endian device bytes; guest RDRAM stores native
/// words under the `^3` byte-lane mapping. Going through
/// `write_logical_bytes` is the same call `fn64-abi`'s
/// `copy_committed_guest_writes` makes. A raw `copy_from_slice` here reports
/// every pixel as byte-swapped -- a runner defect that reads exactly like a
/// renderer defect.
pub(crate) fn wgpu_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let mut rdram = seeded(commands);
    let mut session =
        ConformanceSession::try_new(WIDTH, HEIGHT).map_err(|refusal| refusal.to_string())?;
    let replay = ConformanceReplay {
        layout_bytes: RDRAM_LEN as u32,
        command_start: COMMAND_START,
        words: command_words(commands),
        transaction_sequence: 1,
        guest_read_sources: Vec::new(),
        // Serve declared reads from this fixture's own RDRAM image, exactly
        // as `fn64-abi` slices the live allocation. A partial `FillRectangle`
        // declares a colour-image seed read that no fixture author wrote into
        // `guest_read_sources`; without this the replay supplies 0 sources
        // for 1 declared read and every partial fill is refused, which would
        // read as a wgpu defect when it is a runner gap.
        guest_rdram: Some(rdram.to_vec()),
        target_width: WIDTH,
        target_height: HEIGHT,
    };
    let outcome = session
        .replay(&replay, FRAMEBUFFER)
        .map_err(|refusal| refusal.to_string())?;
    let published = outcome.target_bytes;
    if published.len() < FRAMEBUFFER_BYTES as usize {
        return Err(format!(
            "published {} target bytes, fewer than the declared {FRAMEBUFFER_BYTES}",
            published.len()
        ));
    }
    RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
        RdramAddr::from_offset(FRAMEBUFFER),
        &published[..FRAMEBUFFER_BYTES as usize],
    );
    Ok(observation_bytes(&rdram))
}

/// Keep one loud backend trap from erasing every other case's differential.
/// The gate still treats the structured refusal as non-parity; this only
/// preserves the remaining rows and the trapped message as evidence.
pub(crate) fn wgpu_outcome(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    match std::panic::catch_unwind(|| wgpu_bytes(commands)) {
        Ok(outcome) => outcome,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            Err(format!("wgpu trapped: {message}"))
        }
    }
}

/// The default path to the external angrylion bit-accurate RDP oracle.
///
/// angrylion-rdp-plus is MAME-licensed. It lives OUTSIDE fn64 and is invoked
/// only as an external process; nothing from it is linked or vendored, and
/// this runner never depends on it at build time. If the binary is absent the
/// leg is skipped (fail-open), never a build or test failure.
pub(crate) const ANGRYLION_ORACLE_DEFAULT: &str = "/Users/jer/Code/angrylion-oracle/oracle";
pub(crate) const ANGRYLION_ORACLE_ENV: &str = "FN64_ANGRYLION_ORACLE";

/// Sentinel a skipped angrylion leg returns instead of an error, so a missing
/// oracle never turns a wgpu-vs-RT64 verdict into a failure.
pub(crate) const ANGRYLION_SKIPPED: &str = "angrylion-oracle-skipped";

/// angrylion's committed guest framebuffer -- BIT-ACCURATE hardware ground
/// truth, produced by shelling out to the external oracle binary.
///
/// **Byte domain.** angrylion's RDP core applies exactly fn64's storage lane
/// XORs: `BYTE_ADDR_XOR = 3` (byte reads), `WORD_ADDR_XOR = 1` (halfword
/// reads, i.e. fn64's `^2` on a u16), and no XOR on 32-bit word reads/command
/// fetch. So the runner's `seeded()` storage image is already in angrylion's
/// native RDRAM domain, on BOTH the texture-read side and the framebuffer-
/// write side, and no re-swizzle is applied in either direction: the oracle's
/// raw framebuffer bytes are compared directly against `observation_bytes`.
/// Verified: a `0xf801` fill returns all-`0xf801`, and a two-colour probe
/// (green drawn at x=1 over red) places green at raw slot 0 (`0 ^ 1 = 1`) --
/// the same slot fn64 stores logical pixel 1 in.
///
/// **The whole seeded image is handed to the oracle**, not just the commands.
/// A textured draw reads texture source memory the command stream never wrote;
/// seeding only the commands (the oracle's original mode) makes angrylion read
/// texel 0 everywhere and every textured case reports a spurious disagreement.
/// So the runner writes `seeded(commands)` to a temp file and invokes the
/// oracle's `--rdram` mode with the `[COMMAND_START, command_end)` byte range,
/// so angrylion renders from byte-identical guest memory to wgpu and RT64.
///
/// Missing binary -> `Err(ANGRYLION_SKIPPED)`, a skip sentinel the caller
/// treats as "no reading", not as a defect.
pub(crate) fn angrylion_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let oracle = std::env::var(ANGRYLION_ORACLE_ENV)
        .unwrap_or_else(|_| ANGRYLION_ORACLE_DEFAULT.to_string());
    if !std::path::Path::new(&oracle).exists() {
        return Err(ANGRYLION_SKIPPED.to_string());
    }

    // The full guest memory image every backend renders from: STALE
    // background, GUARDs, staged texture sources, and the command words at
    // COMMAND_START -- byte-identical to what wgpu and RT64 receive.
    let rdram = seeded(commands);

    let dir = std::env::temp_dir();
    let unique = format!(
        "{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let cmds_path = dir.join(format!("fn64-angrylion-rdram-{unique}.bin"));
    let out_path = dir.join(format!("fn64-angrylion-out-{unique}.bin"));

    std::fs::write(&cmds_path, &rdram)
        .map_err(|error| format!("angrylion: writing RDRAM image: {error}"))?;

    let cleanup = |cmds: &std::path::Path, out: &std::path::Path| {
        let _ = std::fs::remove_file(cmds);
        let _ = std::fs::remove_file(out);
    };

    let status = std::process::Command::new(&oracle)
        .arg("--rdram")
        .arg(&cmds_path)
        .arg(format!("{COMMAND_START:x}"))
        .arg(format!("{:x}", command_end(commands)))
        .arg(format!("{FRAMEBUFFER:x}"))
        .arg(WIDTH.to_string())
        .arg(HEIGHT.to_string())
        .arg("2")
        .arg(&out_path)
        .output();

    let output = match status {
        Ok(output) => output,
        Err(error) => {
            cleanup(&cmds_path, &out_path);
            return Err(format!("angrylion: spawning oracle: {error}"));
        }
    };
    if !output.status.success() {
        cleanup(&cmds_path, &out_path);
        return Err(format!(
            "angrylion: oracle exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let bytes = match std::fs::read(&out_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            cleanup(&cmds_path, &out_path);
            return Err(format!("angrylion: reading framebuffer: {error}"));
        }
    };
    cleanup(&cmds_path, &out_path);

    if bytes.len() < FRAMEBUFFER_BYTES as usize {
        return Err(format!(
            "angrylion: oracle wrote {} bytes, fewer than the declared {FRAMEBUFFER_BYTES}",
            bytes.len()
        ));
    }
    Ok(bytes[..FRAMEBUFFER_BYTES as usize].to_vec())
}

/// Whether an angrylion result is the skip sentinel rather than a real answer.
pub(crate) fn angrylion_is_skipped(outcome: &Result<Vec<u8>, String>) -> bool {
    matches!(outcome, Err(message) if message == ANGRYLION_SKIPPED)
}
