//! Wire-format verification for `tools/mupen-trace/mupen_devtrace.c`
//! (design spec `docs/superpowers/specs/2026-07-23-timing-oracle-design.md`,
//! increment 0a component 1).
//!
//! This is NOT a test of `timing_trace.rs` itself (that module already has
//! its own unit tests). It exists to catch wire-format DRIFT between the C
//! producer and the Rust schema: the fixture below is BYTE-IDENTICAL to what
//! `mupen_devtrace.c`'s `emit_header`/`emit_event`/`emit_end` functions print
//! (same `fprintf` format strings, copied verbatim from the C source at the
//! time this test was written) -- so if a future edit to either side changes
//! a field name, tag value, or record shape without updating the other,
//! `ingest_jsonl` fails here instead of silently accepting a malformed trace
//! at oracle-run time.
//!
//! The producer core (`~/Code/mupen64plus-core`, `DEBUGGER=1`) could not be
//! rebuilt in this environment (its `projects/unix/Makefile` has a pre-
//! existing, unrelated macOS/arm64 breakage: `strings --version` is a GNU-
//! only flag BSD `strings` rejects, `all` is not the first-defined target so
//! bare `make` prints help instead of building, and linking `-lopcodes
//! -lbfd` from Homebrew's binutils pulls in a stray `_main` undefined symbol
//! under `-flto`) -- so this fixture stands in for a live-captured sample
//! until that core is fixed and a real capture can be diffed against it.

use fn64_discover::timing_trace::{
    ingest_jsonl, DeviceTraceCompletion, TimingDevice, TimingEventKind,
};
use std::io::Cursor;

/// Verbatim output of `mupen_devtrace.c`'s emit_header/emit_event/emit_end
/// for a representative PI + SI + MI + VI sequence (the four device classes
/// this producer increment emits; SP/DP DMA timing is out of scope per the
/// design spec). Field order and JSON shape match the C `fprintf` calls
/// exactly, not just the Rust struct's `Serialize` output -- this is the
/// point of the test.
const SAMPLE_JSONL: &str = concat!(
    "{\"record\":\"header\",\"ordinal\":0,\"schema_version\":1,",
    "\"producer\":\"mupen-devtrace v1 (mupen64plus-core DEBUGGER=1 pure-interpreter + rsp plugin, single-step device-register polling via public m64p_debugger API)\",",
    "\"trace_id\":\"mini-smoke-1\"}\n",
    "{\"record\":\"device_event\",\"ordinal\":1,\"event_kind\":\"dma_start\",\"device\":\"pi\",\"cycle\":100,\"addr_or_source\":32,\"value_or_len\":64}\n",
    "{\"record\":\"device_event\",\"ordinal\":2,\"event_kind\":\"dma_complete\",\"device\":\"pi\",\"cycle\":112,\"addr_or_source\":32,\"value_or_len\":64}\n",
    "{\"record\":\"device_event\",\"ordinal\":3,\"event_kind\":\"mi_raise\",\"device\":\"mi\",\"cycle\":112,\"addr_or_source\":16,\"value_or_len\":0}\n",
    "{\"record\":\"device_event\",\"ordinal\":4,\"event_kind\":\"dma_start\",\"device\":\"si\",\"cycle\":200,\"addr_or_source\":1024,\"value_or_len\":0}\n",
    "{\"record\":\"device_event\",\"ordinal\":5,\"event_kind\":\"dma_complete\",\"device\":\"si\",\"cycle\":264,\"addr_or_source\":1024,\"value_or_len\":0}\n",
    "{\"record\":\"device_event\",\"ordinal\":6,\"event_kind\":\"mi_raise\",\"device\":\"mi\",\"cycle\":264,\"addr_or_source\":2,\"value_or_len\":0}\n",
    "{\"record\":\"device_event\",\"ordinal\":7,\"event_kind\":\"mi_ack\",\"device\":\"mi\",\"cycle\":300,\"addr_or_source\":2,\"value_or_len\":0}\n",
    "{\"record\":\"device_event\",\"ordinal\":8,\"event_kind\":\"vi_retrace\",\"device\":\"vi\",\"cycle\":350,\"addr_or_source\":0,\"value_or_len\":0}\n",
    "{\"record\":\"end\",\"ordinal\":9,\"completion\":\"completed\"}\n",
);

#[test]
fn mupen_devtrace_c_wire_format_ingests_unmodified() {
    let ingest = ingest_jsonl(Cursor::new(SAMPLE_JSONL)).expect(
        "mupen_devtrace.c's exact emitted JSONL shape must ingest without any \
         schema adaptation -- a failure here means the C producer's fprintf \
         format strings have drifted from timing_trace.rs's DeviceTraceRecord",
    );

    assert_eq!(ingest.header.schema_version, 1);
    assert_eq!(
        ingest.header.producer,
        "mupen-devtrace v1 (mupen64plus-core DEBUGGER=1 pure-interpreter + rsp plugin, single-step device-register polling via public m64p_debugger API)"
    );
    assert_eq!(ingest.header.trace_id, "mini-smoke-1");
    assert_eq!(ingest.completion, DeviceTraceCompletion::Completed);
    assert_eq!(ingest.final_ordinal, 9);
    assert_eq!(ingest.events.len(), 8);

    let ev = &ingest.events;
    // PI DMA start/complete, cycle-stamped, cart-address + length payload.
    assert_eq!(ev[0].event_kind, TimingEventKind::DmaStart);
    assert_eq!(ev[0].device, TimingDevice::Pi);
    assert_eq!(ev[0].cycle, 100);
    assert_eq!(ev[0].addr_or_source, 0x20);
    assert_eq!(ev[0].value_or_len, 64);

    assert_eq!(ev[1].event_kind, TimingEventKind::DmaComplete);
    assert_eq!(ev[1].device, TimingDevice::Pi);
    assert_eq!(ev[1].cycle, 112);
    assert_eq!(ev[1].addr_or_source, 0x20);
    assert_eq!(ev[1].value_or_len, 64);

    // MI raise for the PI source bit (0x10), same cycle as PI completion.
    assert_eq!(ev[2].event_kind, TimingEventKind::MiRaise);
    assert_eq!(ev[2].device, TimingDevice::Mi);
    assert_eq!(ev[2].cycle, 112);
    assert_eq!(ev[2].addr_or_source, 0x10);

    // SI DMA start/complete: fixed 64-byte PIF window, value_or_len == 0.
    assert_eq!(ev[3].event_kind, TimingEventKind::DmaStart);
    assert_eq!(ev[3].device, TimingDevice::Si);
    assert_eq!(ev[3].value_or_len, 0);
    assert_eq!(ev[4].event_kind, TimingEventKind::DmaComplete);
    assert_eq!(ev[4].device, TimingDevice::Si);

    // MI raise/ack for the SI source bit (0x02), at distinct cycles.
    assert_eq!(ev[5].event_kind, TimingEventKind::MiRaise);
    assert_eq!(ev[5].addr_or_source, 0x02);
    assert_eq!(ev[5].cycle, 264);
    assert_eq!(ev[6].event_kind, TimingEventKind::MiAck);
    assert_eq!(ev[6].addr_or_source, 0x02);
    assert_eq!(ev[6].cycle, 300);

    // VI retrace: no payload, its own cycle.
    assert_eq!(ev[7].event_kind, TimingEventKind::ViRetrace);
    assert_eq!(ev[7].device, TimingDevice::Vi);
    assert_eq!(ev[7].cycle, 350);
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
