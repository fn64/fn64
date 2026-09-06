//! Wire-format verification for `tools/mupen-trace/mupen_devtrace.c`
//! (design spec `docs/superpowers/specs/2026-07-23-timing-oracle-design.md`,
//! increment 0a component 1).
//!
//! This is NOT a test of `timing_trace.rs` itself (that module already has
//! its own unit tests). It compiles and runs the checked-in C producer's shared
//! classification/formatting helper, then feeds those emitted bytes directly
//! through Rust ingestion. No ROM or GPL core is needed.

use fn64_discover::timing_trace::{
    ingest_jsonl, DeviceTraceCompletion, TimingDevice, TimingDmaDirection, TimingEventKind,
    TimingPiDevice,
};
use std::path::Path;
use std::process::Command;

fn producer_fixture_jsonl() -> Vec<u8> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = workspace.join("tools/mupen-trace/test-mupen-devtrace-wire.c");
    let binary =
        std::env::temp_dir().join(format!("fn64-mupen-devtrace-wire-{}", std::process::id()));
    let compile = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("a C compiler is required for the checked-in producer contract test");
    assert!(
        compile.status.success(),
        "producer fixture must compile warning-clean:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&binary)
        .output()
        .expect("compiled producer fixture must run");
    let _ = std::fs::remove_file(&binary);
    assert!(
        run.status.success(),
        "producer fixture failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    run.stdout
}

#[test]
fn checked_in_mupen_devtrace_v3_wire_ingests_without_adaptation() {
    let jsonl = producer_fixture_jsonl();
    let text = std::str::from_utf8(&jsonl).expect("C producer emits UTF-8 JSONL");
    assert!(text.contains("\"dma_direction\":null,\"pi_device\":null,\"pi_offset\":null"));
    let ingest = ingest_jsonl(jsonl.as_slice())
        .expect("the checked-in C producer's v3 JSONL must ingest without adaptation");

    assert_eq!(ingest.header.schema_version, 3);
    assert_eq!(ingest.header.clock.hz, fn64_runtime::CPU_CLOCK_HZ);
    assert_eq!(ingest.header.clock.quantum, 2);
    assert_eq!(
        ingest.header.observed_devices,
        vec![
            TimingDevice::Pi,
            TimingDevice::Ai,
            TimingDevice::Si,
            TimingDevice::Vi,
            TimingDevice::Mi,
        ]
    );
    assert_eq!(ingest.header.producer, "mupen-devtrace v3 source fixture");
    assert_eq!(ingest.header.trace_id, "source-fixture-1");
    assert_eq!(ingest.completion, DeviceTraceCompletion::Completed);
    assert_eq!(ingest.final_ordinal, 9);
    assert_eq!(ingest.events.len(), 8);

    let ev = &ingest.events;
    // PI DMA start/complete, cycle-stamped, DRAM-address + length payload.
    assert_eq!(ev[0].event_kind, TimingEventKind::DmaStart);
    assert_eq!(ev[0].device, TimingDevice::Pi);
    assert_eq!(ev[0].cycle, 0);
    assert_eq!(ev[0].addr_or_source, 0x20);
    assert_eq!(ev[0].value_or_len, 64);
    assert_eq!(ev[0].dma_direction, Some(TimingDmaDirection::ToRdram));
    assert_eq!(ev[0].pi_device, Some(TimingPiDevice::Rom));
    assert_eq!(ev[0].pi_offset, Some(0x10));

    assert_eq!(ev[1].event_kind, TimingEventKind::DmaComplete);
    assert_eq!(ev[1].device, TimingDevice::Pi);
    assert_eq!(ev[1].cycle, 12);
    assert_eq!(ev[1].addr_or_source, 0x20);
    assert_eq!(ev[1].value_or_len, 64);

    // The same-poll PI interrupt proof is emitted after completion, preserving
    // the canonical PI-complete-then-MI-raise ordering.
    assert_eq!(ev[2].event_kind, TimingEventKind::MiRaise);
    assert_eq!(ev[2].device, TimingDevice::Mi);
    assert_eq!(ev[2].addr_or_source, 0x10);
    assert_eq!(ev[2].cycle, ev[1].cycle);

    assert_eq!(ev[3].event_kind, TimingEventKind::DmaStart);
    assert_eq!(ev[3].device, TimingDevice::Pi);
    assert_eq!(ev[3].dma_direction, Some(TimingDmaDirection::FromRdram));
    assert_eq!(ev[3].pi_device, Some(TimingPiDevice::Sram));
    assert_eq!(ev[3].pi_offset, Some(0x10));
    assert_eq!(ev[4].event_kind, TimingEventKind::DmaComplete);
    assert_eq!(ev[4].dma_direction, ev[3].dma_direction);
    assert_eq!(ev[4].pi_device, ev[3].pi_device);
    assert_eq!(ev[4].pi_offset, ev[3].pi_offset);

    // SI DMA: fixed 64-byte PIF window, value_or_len == 0.
    assert_eq!(ev[5].event_kind, TimingEventKind::DmaStart);
    assert_eq!(ev[5].device, TimingDevice::Si);
    assert_eq!(ev[5].value_or_len, 0);
    assert_eq!(ev[5].dma_direction, None);
    assert_eq!(ev[5].pi_device, None);
    assert_eq!(ev[5].pi_offset, None);

    // MI raise for the SI source bit (0x02).
    assert_eq!(ev[6].event_kind, TimingEventKind::MiRaise);
    assert_eq!(ev[6].addr_or_source, 0x02);
    assert_eq!(ev[6].cycle, 140);

    // VI retrace: no payload, its own cycle.
    assert_eq!(ev[7].event_kind, TimingEventKind::ViRetrace);
    assert_eq!(ev[7].device, TimingDevice::Vi);
    assert_eq!(ev[7].cycle, 150);
    assert_eq!(ev[7].addr_or_source, 0);
    assert_eq!(ev[7].value_or_len, 0);

    // Ordinals are dense 1..=8 for the events (0 is the header, 9 is end).
    let ordinals: Vec<u64> = ev.iter().map(|e| e.ordinal).collect();
    assert_eq!(ordinals, (1..=8).collect::<Vec<u64>>());
}

/// The MI raw-mask convention: `addr_or_source` for mi_raise/mi_ack carries
/// the SAME bit value as fn64's `InterruptSource::bit()`
/// (`crates/fn64-runtime/src/device.rs`), not a bit index -- both producers
/// must agree on this encoding without either side needing to know the
/// other's internal enum. This pins that convention against regression.
#[test]
fn mi_source_bits_match_fn64_interrupt_source_convention() {
    use fn64_runtime::InterruptSource;

    assert_eq!(InterruptSource::Sp.bit(), 0x01);
    assert_eq!(InterruptSource::Si.bit(), 0x02);
    assert_eq!(InterruptSource::Ai.bit(), 0x04);
    assert_eq!(InterruptSource::Vi.bit(), 0x08);
    assert_eq!(InterruptSource::Pi.bit(), 0x10);
    assert_eq!(InterruptSource::Dp.bit(), 0x20);
}
