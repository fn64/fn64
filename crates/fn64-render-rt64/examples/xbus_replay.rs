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
//!     /tmp/wm2000-gfx-dumps/xbus-0007.bin /tmp/replay-out [rdram-image.bin]`
//!
//! The optional third argument is a full RDRAM dump captured by
//! `FN64_XBUS_STREAM_DUMP_RDRAM` at the same stream index -- with it, texel
//! and TLUT content sampled from RDRAM is the REAL data the live task saw,
//! so the replay reproduces actual frame content, not just coverage.

use fn64_render::{RenderBackend, RenderConfig};
use fn64_render_rt64::ReferenceBackend;

fn main() {
    let mut args = std::env::args().skip(1);
    let stream_path = args
        .next()
        .expect("usage: xbus_replay <xbus-stream.bin> [out-dir] [rdram-image.bin]");
    let out_dir = args.next().unwrap_or_else(|| "/tmp/xbus-replay".to_string());
    let rdram_path = args.next();
    let stream = std::fs::read(&stream_path)
        .unwrap_or_else(|error| panic!("reading {stream_path}: {error}"));
    assert!(
        !stream.is_empty() && stream.len().is_multiple_of(8),
        "stream length {:#x} must be nonempty and 8-byte aligned",
        stream.len()
    );

    // Guest RDRAM -- either a real captured image, or a console-sized
    // zero-filled stand-in -- with the stream staged after it in the recomp
    // word-native layout `read_u32` expects, the exact shape
    // `dispatch_raw_rdp_xbus` hands the backend.
    let base = match &rdram_path {
        Some(path) => {
            std::fs::read(path).unwrap_or_else(|error| panic!("reading {path}: {error}"))
        }
        None => vec![0u8; 0x80_0000],
    };
    let staging = (base.len() + 7) & !7;
    let mut image = vec![0u8; staging + stream.len()];
    image[..base.len()].copy_from_slice(&base);
    drop(base);
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

    // With a real RDRAM image, also decode the committed RGBA16 color image
    // straight from RDRAM (`FN64_XBUS_REPLAY_CIMG=<hex-addr>[,<width>x<height>]`)
    // -- the exact bytes the VI would scan out, independent of the backend's
    // internal surface.
    if let Ok(spec) = std::env::var("FN64_XBUS_REPLAY_CIMG") {
        let (addr_raw, dims) = match spec.split_once(',') {
            Some((addr, dims)) => (addr, Some(dims)),
            None => (spec.as_str(), None),
        };
        let addr = usize::from_str_radix(addr_raw.trim_start_matches("0x"), 16)
            .unwrap_or_else(|_| panic!("FN64_XBUS_REPLAY_CIMG addr must be hex, got {spec:?}"));
        let (width, height) = dims.map_or((320usize, 240usize), |dims| {
            let (w, h) = dims
                .split_once('x')
                .unwrap_or_else(|| panic!("dims must be WxH, got {dims:?}"));
            (w.parse().expect("width"), h.parse().expect("height"))
        });
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let offset = addr + (y * width + x) * 2;
                // Native-word layout: logical big-endian byte b lives at b^3.
                let hi = image[(offset) ^ 3];
                let lo = image[(offset + 1) ^ 3];
                let pixel = u16::from_be_bytes([hi, lo]);
                let expand = |value: u16| -> u8 {
                    let value = (value & 0x1f) as u8;
                    (value << 3) | (value >> 2)
                };
                let out = (y * width + x) * 4;
                rgba[out] = expand(pixel >> 11);
                rgba[out + 1] = expand(pixel >> 6);
                rgba[out + 2] = expand(pixel >> 1);
                rgba[out + 3] = 255;
            }
        }
        let path = format!("{out_dir}/cimg-{addr:08x}.png");
        fn64_render_rt64::png_dump::write_png(
            std::path::Path::new(&path),
            width as u32,
            height as u32,
            &rgba,
        )
        .unwrap_or_else(|error| panic!("writing {path}: {error}"));
        println!("committed color image ({width}x{height}) decoded to {path}");
    }
}
