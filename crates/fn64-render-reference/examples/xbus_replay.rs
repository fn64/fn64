//! Offline replay of a captured raw-RDP XBUS command stream through the
//! reference backend.
//!
//! Input: one `FN64_XBUS_STREAM_DUMP_DIR` dump (`xbus-NNNN.bin`), or a
//! `FN64_RAW_DPC_STREAM_DUMP_DIR` containing ordered `raw-dpc-*-xbus.bin`
//! packets. Both carry logical big-endian command bytes, exactly what the live
//! dispatch staged at submission time. Each stream is re-staged after RDRAM
//! image the same way the live dispatch path stages it, so decode/raster
//! behavior (coverage, combiner, blender, scissor, state machine) replays
//! deterministically without booting the game. Texture CONTENT sampled from
//! RDRAM is zeros here -- this tool answers "where/whether pixels landed",
//! not "which texels they carried".
//!
//! Usage: `cargo run -p fn64-render-reference --example xbus_replay -- \
//!     <stream.bin|stream-dir> /tmp/replay-out [rdram-image.bin]`
//!
//! The optional third argument is a full RDRAM dump captured by
//! `FN64_XBUS_STREAM_DUMP_RDRAM` at the same stream index -- with it, texel
//! and TLUT content sampled from RDRAM is the REAL data the live task saw,
//! so the replay reproduces actual frame content, not just coverage. Directory
//! replay requires that image and retains RDP/TMEM/RDRAM state across packets.
//! `FN64_XBUS_REPLAY_TERMINAL_PACKET=N` truncates a raw-DPC directory after
//! canonical packet N. A capture that begins after the prior task's target
//! declaration may name that observed durable state as
//! `FN64_XBUS_REPLAY_INITIAL_CIMG=<hex-address>,<width>`.

use fn64_render::{RenderBackend, RenderConfig};
use fn64_render_reference::ReferenceBackend;

fn main() {
    let mut args = std::env::args().skip(1);
    let stream_path = args
        .next()
        .expect("usage: xbus_replay <stream.bin|stream-dir> [out-dir] [rdram-image.bin]");
    let out_dir = args
        .next()
        .unwrap_or_else(|| "/tmp/xbus-replay".to_string());
    let rdram_path = args.next();
    let stream_path = std::path::Path::new(&stream_path);
    let streams = load_streams(stream_path);
    assert!(
        !streams.is_empty(),
        "replay input contains no command streams"
    );
    let directory_replay = stream_path.is_dir();
    assert!(
        !directory_replay || rdram_path.is_some(),
        "directory replay requires its initial RDRAM image"
    );

    // Guest RDRAM -- either a real captured image, or a console-sized
    // zero-filled stand-in -- with the stream staged after it in the recomp
    // word-native layout `read_u32` expects, the exact shape
    // `dispatch_raw_rdp_xbus` hands the backend.
    let base = match &rdram_path {
        Some(path) => std::fs::read(path).unwrap_or_else(|error| panic!("reading {path}: {error}")),
        None => vec![0u8; 0x80_0000],
    };
    let staging = (base.len() + 7) & !7;
    let max_stream_len = streams
        .iter()
        .map(Vec::len)
        .max()
        .expect("streams checked above");
    let mut image = vec![0u8; staging + max_stream_len];
    image[..base.len()].copy_from_slice(&base);
    drop(base);

    // `FN64_XBUS_REPLAY_REPEAT=N` re-executes the same stream against a fresh
    // backend N times and reports the per-iteration render cost plus an FNV-1a
    // digest of the resulting framebuffer. This is the renderer's A/B harness:
    // the stream and RDRAM are real capture data, so the loop measures the
    // actual rasterizer hot path with no guest execution in the sample, and
    // the digest makes an optimization's byte-exactness checkable directly
    // (same digest before and after == identical pixels).
    let repeat = std::env::var("FN64_XBUS_REPLAY_REPEAT")
        .ok()
        .map(|raw| {
            raw.parse::<u32>()
                .unwrap_or_else(|_| panic!("FN64_XBUS_REPLAY_REPEAT must be a u32, got {raw:?}"))
        })
        .unwrap_or(0);
    if repeat > 0 {
        assert_eq!(
            streams.len(),
            1,
            "FN64_XBUS_REPLAY_REPEAT supports one stream, not a directory sequence"
        );
        let stream = &streams[0];
        stage_stream(&mut image, staging, stream);
        // The staged command words are read-only, but rasterizing writes the
        // color image back into `image`. Replay from a pristine copy each
        // iteration so run N sees exactly the inputs run 1 saw.
        let pristine = image.clone();
        let mut digest = None::<u64>;
        let mut best = f64::INFINITY;
        for _ in 0..repeat {
            image.copy_from_slice(&pristine);
            let mut backend = ReferenceBackend::new().with_f3dex2();
            backend
                .create(&RenderConfig::ntsc(480, 240))
                .expect("reference backend create");
            let started = std::time::Instant::now();
            backend
                .process_rdp_commands(
                    &mut image,
                    staging as u32,
                    (staging + stream.len()) as u32,
                    0,
                    true,
                )
                .expect("raw RDP stream replay");
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            best = best.min(elapsed);
            let framebuffer = backend.framebuffer().expect("framebuffer after create()");
            let mut hash: u64 = 0xcbf29ce484222325;
            for &byte in &framebuffer.pixels {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
            // Every iteration must be bit-identical; a digest that drifts
            // between runs would mean the renderer is not deterministic and
            // no A/B comparison below it could be trusted.
            match digest {
                None => digest = Some(hash),
                Some(first) => assert_eq!(
                    first, hash,
                    "reference renderer produced a different framebuffer across identical replays"
                ),
            }
        }
        println!(
            "repeat={repeat} best_render_ms={best:.3} fb_fnv1a={:016x}",
            digest.expect("at least one iteration")
        );
        return;
    }

    let mut backend = ReferenceBackend::new().with_f3dex2();
    if !directory_replay {
        backend = backend.with_auto_dump(&out_dir, "xbus-replay", 16);
    }
    backend
        .create(&RenderConfig::ntsc(480, 240))
        .expect("reference backend create");
    if let Ok(spec) = std::env::var("FN64_XBUS_REPLAY_INITIAL_CIMG") {
        let (address, width) = parse_initial_cimg(&spec);
        let command = [0xff10_0000 | u32::from(width - 1), address & 0x03ff_ffff];
        let stream: Vec<u8> = command.into_iter().flat_map(u32::to_be_bytes).collect();
        stage_stream(&mut image, staging, &stream);
        backend
            .process_rdp_commands(
                &mut image,
                staging as u32,
                (staging + stream.len()) as u32,
                0,
                true,
            )
            .expect("seed observed durable color-image state");
        println!("seeded observed RGBA16 color image at {address:#010x}, width {width}");
    }
    for stream in &streams {
        stage_stream(&mut image, staging, stream);
    backend
        .process_rdp_commands(
            &mut image,
            staging as u32,
            (staging + stream.len()) as u32,
            0,
            true,
        )
        .expect("raw RDP stream replay");
    }
    println!(
        "replayed {} packet(s), {} bytes of raw RDP commands; any configured output was dumped to {out_dir}",
        streams.len(),
        streams.iter().map(Vec::len).sum::<usize>()
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
        fn64_render_reference::png_dump::write_png(
            std::path::Path::new(&path),
            width as u32,
            height as u32,
            &rgba,
        )
        .unwrap_or_else(|error| panic!("writing {path}: {error}"));
        println!("committed color image ({width}x{height}) decoded to {path}");
    }
}

fn load_streams(path: &std::path::Path) -> Vec<Vec<u8>> {
    let paths = if path.is_dir() {
        let terminal = std::env::var("FN64_XBUS_REPLAY_TERMINAL_PACKET")
            .ok()
            .map(|raw| {
                raw.parse::<u64>().unwrap_or_else(|_| {
                    panic!("FN64_XBUS_REPLAY_TERMINAL_PACKET must be a u64, got {raw:?}")
                })
            });
        let mut paths: Vec<_> = std::fs::read_dir(path)
            .unwrap_or_else(|error| panic!("reading stream directory {path:?}: {error}"))
            .map(|entry| entry.expect("reading stream-directory entry").path())
            .filter(|entry| {
                let Some(name) = entry.file_name().and_then(|name| name.to_str()) else {
                    return false;
                };
                if !name.starts_with("raw-dpc-") || !name.ends_with("-xbus.bin") {
                    return false;
                }
                terminal.is_none_or(|terminal| raw_dpc_packet_index(name) <= terminal)
            })
            .collect();
        paths.sort();
        paths
    } else {
        vec![path.to_path_buf()]
    };
    paths
        .into_iter()
        .map(|path| {
            let stream = std::fs::read(&path)
                .unwrap_or_else(|error| panic!("reading command stream {path:?}: {error}"));
            assert!(
                !stream.is_empty() && stream.len().is_multiple_of(8),
                "stream {path:?} length {:#x} must be nonempty and 8-byte aligned",
                stream.len()
            );
            stream
        })
        .collect()
}

fn raw_dpc_packet_index(name: &str) -> u64 {
    name.strip_prefix("raw-dpc-")
        .and_then(|suffix| suffix.strip_suffix("-xbus.bin"))
        .and_then(|index| index.parse().ok())
        .unwrap_or_else(|| panic!("noncanonical raw-DPC stream name {name:?}"))
}

fn stage_stream(image: &mut [u8], staging: usize, stream: &[u8]) {
    for (index, word) in stream.chunks_exact(4).enumerate() {
        let value = u32::from_be_bytes(word.try_into().expect("four stream bytes"));
        let offset = staging + index * 4;
        image[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
}

fn parse_initial_cimg(spec: &str) -> (u32, u16) {
    let (address, width) = spec.split_once(',').unwrap_or_else(|| {
        panic!("FN64_XBUS_REPLAY_INITIAL_CIMG must be <hex-address>,<width>, got {spec:?}")
    });
    let address = u32::from_str_radix(address.trim_start_matches("0x"), 16)
        .unwrap_or_else(|_| panic!("initial color-image address must be hex, got {address:?}"));
    let width = width
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("initial color-image width must be u16, got {width:?}"));
    assert!(
        (1..=4096).contains(&width),
        "initial color-image width must be 1..=4096"
    );
    (address, width)
}
