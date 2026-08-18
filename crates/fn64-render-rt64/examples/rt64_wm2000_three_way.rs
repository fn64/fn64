//! **Is the wgpu port's WM2000 frame 0 validated against an independent
//! AUTHORITY, or only against a same-lineage implementation?**
//!
//! `crates/fn64-abi/src/task_dispatch/tests/raw_dpc_session_integration.rs`'s
//! `wm2000_frame_zero_agrees_exactly_when_alpha_dither_is_disabled` reports 0
//! differing pixels of 115,200 between `WgpuBackend` and
//! `fn64-render-reference`. Both of those derive from public SGI documentation
//! and this project's reading of it, so their agreement proves internal
//! consistency, not fidelity to silicon.
//!
//! RT64 is a separate lineage. This example drives the *same captured packet*
//! over the *same* staged bytes and the same `[start, end)` range through all
//! three backends, and reports all three pairwise differing-pixel counts.
//!
//! The entry point is not uniform, and that is a finding rather than a
//! convenience: `fn64-render-reference` and `fn64-render-rt64` both implement
//! `RenderBackend::process_rdp_commands`, but **`WgpuBackend` does not** -- it
//! inherits the trait default, which refuses every range by name. The port's
//! raw-RDP path is the `RawDpcAbiSession` seam, so it is driven through
//! `osDpSetNextBuffer_recomp`, the production `libultra` shim. See
//! `docs/RT64-WM2000-THREE-WAY.md` §5.
//!
//! # The controlled variable
//!
//! `alpha_dither` is rewritten to `Disabled` (encoding 3) in the shared word
//! stream handed to **all three** backends, exactly as the existing two-way
//! comparison does. That is a control, not a tuning: the oracle's `Noise` term
//! is a SplitMix64 stream its own source declines to call the silicon
//! sequence, so no correct implementation can be required to match it. The
//! rewrite touches only other-mode-high bits 4:5 and is asserted to touch
//! nothing else.
//!
//! # What this does not do
//!
//! It does not modify any of the three implementations, and it does not
//! weaken any refusal. A backend that refuses the packet has its refusal
//! reported as the measurement.

use std::error::Error;
use std::io;

use fn64_render::{RenderBackend, RenderConfig};
use fn64_render_reference::ReferenceBackend;
use fn64_render_rt64::Rt64Backend;

/// The packet dump, same variable the existing two-way comparison reads.
const WM2000_PACKET_ENV: &str = "FN64_WM2000_PACKET_TSV";
const WM2000_PACKET_ENTRY_ENV: &str = "FN64_WM2000_PACKET_ENTRY";

/// Where the replay stages the captured words in guest RDRAM. Same address
/// the reference path in the two-way harness uses, so the byte stream every
/// backend decodes is identical.
const REPLAY_START: u32 = 0x1000;

/// One decode entry's command words, reconstructed from a packet dump.
struct CapturedPacket {
    entry: u64,
    words: Vec<u32>,
    source_pc: usize,
}

/// Parse a packet-dump TSV and reconstruct one decode entry's word stream.
///
/// Consecutive rows must be exactly 8 RDRAM bytes apart. That contiguity
/// check is what makes the concatenated word pairs the wire stream rather
/// than a lossy sample of it.
fn parse_packet_dump(text: &str, entry: u64) -> CapturedPacket {
    let mut rows: Vec<(usize, u32, u32)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("entry\t") {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            fields.len(),
            5,
            "{WM2000_PACKET_ENV} line {} has {} tab-separated fields, expected 5",
            index + 1,
            fields.len()
        );
        let parse_hex = |field: &str, name: &str| -> u64 {
            let stripped = field.strip_prefix("0x").unwrap_or_else(|| {
                panic!(
                    "{WM2000_PACKET_ENV} line {} field {name} is {field:?}, expected 0x-prefixed hex",
                    index + 1
                )
            });
            u64::from_str_radix(stripped, 16).unwrap_or_else(|e| {
                panic!(
                    "{WM2000_PACKET_ENV} line {} field {name} is {field:?}: {e}",
                    index + 1
                )
            })
        };
        let row_entry: u64 = fields[0].parse().unwrap_or_else(|e| {
            panic!(
                "{WM2000_PACKET_ENV} line {} entry field is {:?}: {e}",
                index + 1,
                fields[0]
            )
        });
        if row_entry != entry {
            continue;
        }
        assert_eq!(
            fields[1],
            "RDP",
            "{WM2000_PACKET_ENV} line {} is on the {} lane; this replay drives the raw-RDP lane",
            index + 1,
            fields[1]
        );
        rows.push((
            parse_hex(fields[2], "pc") as usize,
            parse_hex(fields[3], "w0") as u32,
            parse_hex(fields[4], "w1") as u32,
        ));
    }
    assert!(
        !rows.is_empty(),
        "{WM2000_PACKET_ENV} carries no rows for decode entry {entry}"
    );
    for pair in rows.windows(2) {
        assert_eq!(
            pair[1].0,
            pair[0].0 + 8,
            "packet-dump rows for entry {entry} are not contiguous: {:#010x} then {:#010x}",
            pair[0].0,
            pair[1].0
        );
    }
    let source_pc = rows[0].0;
    let words = rows
        .iter()
        .flat_map(|&(_, w0, w1)| [w0, w1])
        .collect::<Vec<u32>>();
    CapturedPacket {
        entry,
        words,
        source_pc,
    }
}

fn load_captured_packet() -> Option<CapturedPacket> {
    let path = std::env::var_os(WM2000_PACKET_ENV)?;
    let entry: u64 = match std::env::var(WM2000_PACKET_ENTRY_ENV) {
        Ok(raw) if !raw.trim().is_empty() => raw
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("{WM2000_PACKET_ENTRY_ENV} is {raw:?}: {e}")),
        _ => 0,
    };
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{WM2000_PACKET_ENV} names {path:?}, unreadable: {e}"));
    Some(parse_packet_dump(&text, entry))
}

/// Walk a raw-RDP word stream into `(byte_offset, cmd6, w0, w1)`.
///
/// Deliberately minimal and independent of any decoder under test: it knows
/// only that a raw-RDP command is 8 bytes except `G_TEXRECT` (`0x24`) and
/// `G_TEXRECTFLIP` (`0x25`), which are 16.
fn walk_raw_rdp_commands(words: &[u32]) -> Vec<(usize, u8, u32, u32)> {
    let mut out = Vec::new();
    let mut index = 0usize;
    while index + 1 < words.len() {
        let w0 = words[index];
        let w1 = words[index + 1];
        let cmd6 = ((w0 >> 24) & 0x3f) as u8;
        out.push((index * 4, cmd6, w0, w1));
        index += if matches!(cmd6, 0x24 | 0x25) { 4 } else { 2 };
    }
    out
}

/// Color-target extent read from the packet's own `SetColorImage` width and
/// `SetScissor` lower-right Y, rather than hardcoded.
fn replay_target_extent(commands: &[(usize, u8, u32, u32)]) -> (u32, u32) {
    let width = commands
        .iter()
        .find(|&&(_, cmd6, _, _)| cmd6 == 0x3f)
        .map(|&(_, _, w0, _)| (w0 & 0x0fff) + 1)
        .expect("the packet must set a color image");
    let height = commands
        .iter()
        .find(|&&(_, cmd6, _, _)| cmd6 == 0x2d)
        .map(|&(_, _, _, w1)| (w1 & 0x0fff) >> 2)
        .expect("the packet must set a scissor");
    (width, height)
}

/// The packet's own `SetColorImage` destination address.
fn wm2000_color_image_addr(commands: &[(usize, u8, u32, u32)]) -> u32 {
    commands
        .iter()
        .find(|&&(_, cmd6, _, _)| cmd6 == 0x3f)
        .map(|&(_, _, _, w1)| w1)
        .expect("the packet must set a color image")
}

/// Rewrite `alpha_dither` (other-mode high, bits 4:5) in every `SetOtherMode`
/// (`0x2f`), asserting that no other bit moves.
fn wm2000_packet_with_alpha_dither(packet: &CapturedPacket, mode: u32) -> CapturedPacket {
    assert!(
        mode < 4,
        "alpha_dither is a two-bit field; {mode} does not fit"
    );
    let commands = walk_raw_rdp_commands(&packet.words);
    let mut words = packet.words.clone();
    let mut rewritten = 0usize;
    for &(offset, cmd6, _, _) in &commands {
        if cmd6 != 0x2f {
            continue;
        }
        let index = offset / 4;
        words[index] = (words[index] & !(0x3 << 4)) | (mode << 4);
        rewritten += 1;
    }
    assert!(
        rewritten > 0,
        "the packet must latch other-mode at least once"
    );
    for (before, after) in packet.words.iter().zip(words.iter()) {
        assert_eq!(
            before & !(0x3 << 4),
            after & !(0x3 << 4),
            "the alpha_dither rewrite altered a bit outside other-mode high 4:5"
        );
    }
    CapturedPacket {
        entry: packet.entry,
        words,
        source_pc: packet.source_pc,
    }
}

fn words_to_rdram_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_ne_bytes()).collect()
}

/// Stage the captured words into a fresh 8 MiB RDRAM and return the
/// `(rdram, start, end)` triple every backend is driven over.
///
/// The packet ends with its own `G_ENDDL` (`0xdf`), and `process_rdp_commands`
/// appends a terminator of its own at `end`. Handing a backend the packet's
/// terminator as a command would make this harness the frontier, so the last
/// command is excluded and the backend's own terminator takes its place at
/// exactly the same address. The byte stream each backend decodes is the
/// packet's, unmodified.
fn stage(packet: &CapturedPacket, commands: &[(usize, u8, u32, u32)]) -> (Vec<u8>, u32, u32) {
    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    let bytes = words_to_rdram_bytes(&packet.words);
    let start = REPLAY_START;
    let loaded_end = start + bytes.len() as u32;
    rdram[start as usize..loaded_end as usize].copy_from_slice(&bytes);
    let end = commands
        .last()
        .filter(|&&(_, cmd6, _, _)| cmd6 == 0x1f)
        .map_or(loaded_end, |&(offset, _, _, _)| start + offset as u32);
    (rdram, start, end)
}

/// Read the published color image back through `fn64-runtime`'s single
/// authority on the `^3` storage lane mapping, the same readback the existing
/// two-way comparison uses.
fn read_published(rdram: &[u8], color_image_addr: u32, target_bytes: usize) -> Vec<u8> {
    let mut published = vec![0u8; target_bytes];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(color_image_addr),
        &mut published,
    );
    published
}

/// **Drive `WgpuBackend` over the staged packet through the production ABI
/// seam.**
///
/// `WgpuBackend` does NOT implement `RenderBackend::process_rdp_commands` --
/// it inherits the trait default, which refuses every range by name
/// (`fn64-render/src/lib.rs:1643`, "raw RDP command execution ... is
/// unsupported"). The port's raw-RDP path is the `RawDpcAbiSession` seam
/// instead, so driving it through the trait method would measure the trait
/// default's refusal rather than the port. That difference is a finding, not
/// a workaround, and it is why this function exists alongside `render_with`.
///
/// The entry used here is `osDpSetNextBuffer_recomp`, the real `libultra`
/// shim a recompiled game calls -- not a test-internal helper -- so the port
/// is driven over exactly the bytes and the boundary production uses.
fn render_port(
    packet: &CapturedPacket,
    commands: &[(usize, u8, u32, u32)],
) -> Result<Vec<u8>, String> {
    let (target_width, target_height) = replay_target_extent(commands);
    let color_image_addr = wm2000_color_image_addr(commands);
    let target_bytes = (target_width * target_height * 2) as usize;
    let (mut rdram, start, end) = stage(packet, commands);

    fn64_abi::load_rom(Vec::new());
    {
        let (mut backend, session) = fn64_render_wgpu::WgpuBackend::try_new()
            .map_err(|e| format!("WgpuBackend::try_new refused: {e:?}"))?;
        let _ = backend.create(&RenderConfig {
            width: target_width,
            height: target_height,
            tv_type: fn64_runtime::TvType::default(),
        });
        fn64_abi::set_render_backend(Box::new(backend), rdram.len());
        fn64_abi::set_raw_dpc_session(session);
    }

    // `osDpSetNextBuffer(bufPtr, size)`: o32 puts the aligned 64-bit second
    // argument in `$a2:$a3`. The range is the same `[start, end)` every other
    // backend receives.
    let mut ctx: fn64_abi::RecompContext = unsafe { std::mem::zeroed() };
    ctx.r4 = u64::from(start);
    let size = u64::from(end - start);
    ctx.r6 = size >> 32;
    ctx.r7 = size & 0xffff_ffff;

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        fn64_abi::osDpSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut ctx);
    }));
    let published = match outcome {
        Ok(()) => {
            if ctx.r2 == u64::MAX {
                Err("osDpSetNextBuffer_recomp returned -1 (DP busy or bad range)".to_owned())
            } else {
                Ok(read_published(&rdram, color_image_addr, target_bytes))
            }
        }
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic payload>")
                .to_owned();
            Err(format!("panicked: {message}"))
        }
    };
    fn64_abi::clear_raw_dpc_session();
    published
}

/// Drive one backend over the staged packet and return its published image,
/// or the backend's own refusal text. A refusal is a measurement, not a skip.
fn render_with<B: RenderBackend>(
    mut backend: B,
    packet: &CapturedPacket,
    commands: &[(usize, u8, u32, u32)],
) -> Result<Vec<u8>, String> {
    let (target_width, target_height) = replay_target_extent(commands);
    let color_image_addr = wm2000_color_image_addr(commands);
    let target_bytes = (target_width * target_height * 2) as usize;
    let (mut rdram, start, end) = stage(packet, commands);

    backend
        .create(&RenderConfig {
            width: target_width,
            height: target_height,
            tv_type: fn64_runtime::TvType::default(),
        })
        .map_err(|e| format!("create() refused: {e:?}"))?;

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        backend.process_rdp_commands(&mut rdram, start, end, 0, true)
    }));
    match outcome {
        Ok(Ok(_status)) => {}
        Ok(Err(e)) => return Err(format!("returned an error: {e:?}")),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic payload>")
                .to_owned();
            return Err(format!("panicked: {message}"));
        }
    }
    Ok(read_published(&rdram, color_image_addr, target_bytes))
}

/// Histogram of the RGBA16 halfwords, descending by count.
fn rgba16_histogram(image: &[u8]) -> Vec<(u16, usize)> {
    let mut counts: std::collections::BTreeMap<u16, usize> = std::collections::BTreeMap::new();
    for pixel in image.chunks_exact(2) {
        *counts
            .entry(u16::from_be_bytes([pixel[0], pixel[1]]))
            .or_default() += 1;
    }
    let mut out: Vec<(u16, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(2)
        .zip(b.chunks_exact(2))
        .filter(|(x, y)| x != y)
        .count()
}

/// Perturb an image's red channel by `+delta` five-bit steps, saturating.
/// This is the mutation control: a known delta whose differing-pixel count is
/// predicted before it is measured.
fn perturb_red(image: &[u8], delta: u16) -> Vec<u8> {
    let mut out = image.to_vec();
    for pixel in out.chunks_exact_mut(2) {
        let value = u16::from_be_bytes([pixel[0], pixel[1]]);
        let red = (value >> 11) & 0x1f;
        let moved = (red + delta).min(0x1f);
        let rebuilt = (value & 0x07ff) | (moved << 11);
        pixel.copy_from_slice(&rebuilt.to_be_bytes());
    }
    out
}

fn report(label: &str, image: &Result<Vec<u8>, String>) {
    match image {
        Ok(bytes) => println!(
            "[three-way] {label}: {} pixels, histogram {:04x?}",
            bytes.len() / 2,
            rgba16_histogram(bytes)
        ),
        Err(reason) => println!("[three-way] {label}: REFUSED -- {reason}"),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let Some(packet) = load_captured_packet() else {
        return Err(io::Error::other(format!(
            "{WM2000_PACKET_ENV} unset; this comparison requires the captured WM2000 packet"
        ))
        .into());
    };

    let as_captured = walk_raw_rdp_commands(&packet.words);
    println!(
        "[three-way] entry {} source_pc {:#010x}: {} commands",
        packet.entry,
        packet.source_pc,
        as_captured.len()
    );
    // The census-measured identity of entry 0, checked so a synthetic
    // stand-in cannot masquerade as the capture.
    assert_eq!(as_captured.len(), 366, "WM2000 entry 0 is 366 commands");
    let fillrects = as_captured
        .iter()
        .filter(|&&(_, c, _, _)| c == 0x36)
        .count();
    let texrects = as_captured
        .iter()
        .filter(|&&(_, c, _, _)| c == 0x24)
        .count();
    let triangles = as_captured
        .iter()
        .filter(|&&(_, c, _, _)| (0x08..=0x0f).contains(&c))
        .count();
    println!("[three-way] fillrects {fillrects} texrects {texrects} triangles {triangles}");
    assert_eq!(fillrects, 60, "census: 60 G_FILLRECT");
    assert_eq!(texrects, 60, "census: 60 G_TEXRECT");
    assert_eq!(triangles, 0, "census: zero triangles");

    // The control: alpha_dither -> Disabled (encoding 3) in the ONE shared
    // word stream all three backends decode.
    let controlled = wm2000_packet_with_alpha_dither(&packet, 3);
    let commands = walk_raw_rdp_commands(&controlled.words);
    // Decoded off the wire by hand, not through any backend's accessor.
    for &(_, cmd6, w0, _) in &commands {
        if cmd6 == 0x2f {
            assert_eq!(
                (w0 >> 4) & 0x3,
                3,
                "every rewritten SetOtherMode must latch alpha_dither = Disabled"
            );
        }
    }
    let (width, height) = replay_target_extent(&commands);
    println!(
        "[three-way] target {width}x{height}, color image {:#010x}",
        wm2000_color_image_addr(&commands)
    );

    let port = render_port(&controlled, &commands);
    let reference = render_with(ReferenceBackend::new(), &controlled, &commands);
    let rt64 = render_with(Rt64Backend::default(), &controlled, &commands);

    report("port  (fn64-render-wgpu)", &port);
    report("ref   (fn64-render-reference)", &reference);
    report("rt64  (fn64-render-rt64)", &rt64);

    let pair = |a: &Result<Vec<u8>, String>, b: &Result<Vec<u8>, String>| -> String {
        match (a, b) {
            (Ok(x), Ok(y)) => format!("{} of {}", differing(x, y), x.len() / 2),
            _ => "n/a (a side refused)".to_owned(),
        }
    };
    println!("[three-way] port vs ref  : {}", pair(&port, &reference));
    println!("[three-way] port vs rt64 : {}", pair(&port, &rt64));
    println!("[three-way] ref  vs rt64 : {}", pair(&reference, &rt64));

    // Mutation control. The hand-derived prediction, stated before the
    // measurement: the port publishes 0xdef7 at 114,481 pixels and 0x0001 at
    // 719. 0xdef7's red field is 0xdef7 >> 11 = 27, so +1 moves it to 28 and
    // those pixels differ. 0x0001's red field is 0, so +1 moves it to 1 and
    // those differ too. Every pixel should therefore move: 115,200 of
    // 115,200.
    if let Ok(port_image) = &port {
        let mutated = perturb_red(port_image, 1);
        let moved = differing(port_image, &mutated);
        println!(
            "[three-way] mutation control (+1 red step): {moved} of {} differ",
            port_image.len() / 2
        );
        assert_eq!(
            moved,
            port_image.len() / 2,
            "the +1-red mutant must move every pixel; if it does not, the comparison does not \
             discriminate"
        );
        if let Ok(reference_image) = &reference {
            let against_reference = differing(&mutated, reference_image);
            println!(
                "[three-way] mutant vs ref: {against_reference} of {} differ",
                mutated.len() / 2
            );
            assert_eq!(
                against_reference,
                mutated.len() / 2,
                "the mutant must disagree with the reference everywhere"
            );
        }
    }

    // **The positive control on the fixture itself.** Agreement is only
    // evidence if the three backends are actually rasterizing this packet
    // rather than all leaving the target untouched. Perturb the packet's own
    // prim colour and require every backend that produced an image to move.
    //
    // Hand-derived prediction, stated before measurement: the texrects'
    // combiner is flat `Primitive`, so the 114,481 texrect pixels are the
    // prim colour and must change, while the 719 fill pixels come from
    // `SetFillColor` and must not. The expected count is therefore 114,481 --
    // not "some pixels", and not the whole target.
    let perturbed = wm2000_packet_with_prim_red(&controlled, 0x20);
    let perturbed_commands = walk_raw_rdp_commands(&perturbed.words);
    let port_p = render_port(&perturbed, &perturbed_commands);
    let reference_p = render_with(ReferenceBackend::new(), &perturbed, &perturbed_commands);
    let rt64_p = render_with(Rt64Backend::default(), &perturbed, &perturbed_commands);
    for (label, base, moved) in [
        ("port", &port, &port_p),
        ("ref", &reference, &reference_p),
        ("rt64", &rt64, &rt64_p),
    ] {
        let (Ok(base), Ok(moved)) = (base, moved) else {
            println!("[three-way] prim control {label}: n/a (a side refused)");
            continue;
        };
        let count = differing(base, moved);
        println!(
            "[three-way] prim control {label}: {count} of {} moved",
            base.len() / 2
        );
        assert_eq!(
            count, 114_481,
            "{label} must move exactly its texrect pixels when the packet's prim colour changes; \
             a backend that does not is not rasterizing this packet"
        );
    }

    Ok(())
}

/// Rewrite the red channel of every `SetPrimColor` (`0x3a`) in the packet.
///
/// `SetPrimColor`'s word 1 is RGBA8888, so red is bits 31:24. Only that byte
/// moves; the assertion below is what makes this a control rather than a
/// second packet.
fn wm2000_packet_with_prim_red(packet: &CapturedPacket, red: u8) -> CapturedPacket {
    let commands = walk_raw_rdp_commands(&packet.words);
    let mut words = packet.words.clone();
    let mut rewritten = 0usize;
    for &(offset, cmd6, _, _) in &commands {
        if cmd6 != 0x3a {
            continue;
        }
        let index = offset / 4 + 1;
        words[index] = (words[index] & 0x00ff_ffff) | (u32::from(red) << 24);
        rewritten += 1;
    }
    assert!(
        rewritten > 0,
        "the packet must set a prim colour for this control to mean anything"
    );
    for (before, after) in packet.words.iter().zip(words.iter()) {
        assert_eq!(
            before & 0x00ff_ffff,
            after & 0x00ff_ffff,
            "the prim-red rewrite altered a bit outside RGBA8888 bits 31:24"
        );
    }
    CapturedPacket {
        entry: packet.entry,
        words,
        source_pc: packet.source_pc,
    }
}
