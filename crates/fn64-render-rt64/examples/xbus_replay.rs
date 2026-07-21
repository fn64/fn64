//! Offline replay of a captured raw-RDP XBUS command stream through the
//! reference backend.
//!
//! Input: one `FN64_XBUS_STREAM_DUMP_DIR` dump (`xbus-NNNN.bin` -- logical
//! big-endian command bytes, exactly what `dispatch_raw_rdp_xbus` staged at
//! submission time). The stream is re-staged after a zeroed stand-in RDRAM
//! image the same way the live dispatch path stages it, so decode/raster
//! behavior (coverage, combiner, blender, scissor, state machine) replays
//! deterministically without booting the game. Texture CONTENT sampled from
//! RDRAM is zeros here -- this tool answers "where/whether pixels landed",
//! not "which texels they carried".
//!
//! Usage: `cargo run -p fn64-render-rt64 --example xbus_replay -- \
//!     /tmp/wm2000-gfx-dumps/xbus-0007.bin /tmp/replay-out`

use fn64_render::{RenderBackend, RenderConfig};
use fn64_render_rt64::ReferenceBackend;

fn main() {
    let mut args = std::env::args().skip(1);
    let stream_path = args
        .next()
        .expect("usage: xbus_replay <xbus-stream.bin> [out-dir]");
    let out_dir = args.next().unwrap_or_else(|| "/tmp/xbus-replay".to_string());
    let stream = std::fs::read(&stream_path)
        .unwrap_or_else(|error| panic!("reading {stream_path}: {error}"));
    assert!(
        !stream.is_empty() && stream.len().is_multiple_of(8),
        "stream length {:#x} must be nonempty and 8-byte aligned",
        stream.len()
    );

    // Stand-in guest RDRAM (console-sized, zero-filled) with the stream
    // staged after it in the recomp word-native layout `read_u32` expects --
    // the exact shape `dispatch_raw_rdp_xbus` hands the backend.
    const RDRAM_LEN: usize = 0x80_0000;
    let staging = RDRAM_LEN;
    let mut image = vec![0u8; staging + stream.len()];
    for (index, word) in stream.chunks_exact(4).enumerate() {
        let value = u32::from_be_bytes(word.try_into().expect("four stream bytes"));
        let offset = staging + index * 4;
        image[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }

    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_auto_dump(&out_dir, "xbus-replay", 16);
    backend
        .create(&RenderConfig::new(480, 240))
        .expect("reference backend create");
    backend
        .process_rdp_commands(
            &mut image,
            staging as u32,
            (staging + stream.len()) as u32,
            0,
        )
        .expect("raw RDP stream replay");
    println!(
        "replayed {} bytes of raw RDP commands; any non-clear frame was dumped to {out_dir}",
        stream.len()
    );
}
