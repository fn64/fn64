#!/usr/bin/env python3
"""Add a WM2000_WATCH guest-memory probe to a COPY of the wm2000-boot harness.

Why this exists, and why it is a committed script rather than a .patch file:
the harness source lives in the sibling `~/Code/recomps/wm2000` repository,
which fn64 lanes are told not to edit, and `run-rs-lane.sh` copies it into a
scratch sibling tree on every run. A probe therefore has to be re-applied each
time. The harness-traps doc records exactly what happens when such a knob is
instead hand-edited into a scratch copy and committed only as a `.patch` with
absolute /tmp paths: it was never re-applied, the next run silently used the
old behaviour, and the result was misread. An idempotent, committed,
source-anchored patcher does not rot the same way.

WHAT THE PROBE ANSWERS. "Does input reach gameplay?" has been attacked with
frame hashes, and the frame hash has been shown to be the wrong instrument on
this ROM (a screen can compose at full rate with a frozen scanned-out image).
Guest memory is the direct instrument: the ROM's own input path writes the pad
word into per-port records, so reading those addresses back out of RDRAM says
whether the button the harness injected arrived where the game reads it --
independently of anything the renderer does.

Addresses default to the ones read out of the NWXE disassembly:

  0x80095184  port-0 HELD      (func_80004628, sh $v1, 0x4($a1) @ 0x8000496C)
  0x80095186  port-0 PRESSED   (                sh $v0, 0x6($a1) @ 0x8000497C)
  0x80095190  port-1 HELD      (base D_80095180 + stride 0xC)
  0x80095192  port-1 PRESSED

WM2000_WATCH=<hex>[:<width>][,...]  -- width is 1, 2 (default) or 4.
The probe samples once per VI swap and prints only on CHANGE, so an idle
watch costs one comparison per swap and produces no output.

Usage:  wm2000-watch-patch.py <path-to-main.rs>
"""
import re
import sys

DECL = '''    // WM2000_WATCH=<hex>[:<width>][,...] -- sample guest RDRAM once per VI
    // swap and print only on change. See docs/tools/wm2000-watch-patch.py.
    let watch_specs: Vec<(u32, u32)> = std::env::var("WM2000_WATCH")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|spec| {
                    let mut it = spec.trim().split(':');
                    let addr_raw = it.next().unwrap_or_default();
                    let addr = u32::from_str_radix(addr_raw.trim_start_matches("0x"), 16)
                        .unwrap_or_else(|_| panic!("WM2000_WATCH address must be hex, got {addr_raw:?}"));
                    let width = it.next().map_or(2u32, |w| {
                        w.parse::<u32>()
                            .unwrap_or_else(|_| panic!("WM2000_WATCH width must be 1, 2 or 4, got {w:?}"))
                    });
                    assert!(matches!(width, 1 | 2 | 4), "WM2000_WATCH width must be 1, 2 or 4, got {width}");
                    // KSEG0 vaddr -> RDRAM offset, the same conversion
                    // WM2000_CHAN_ARRAY_PTR above already encodes.
                    assert!(addr >= 0x8000_0000, "WM2000_WATCH address must be a KSEG0 vaddr, got {addr:#010x}");
                    (addr - 0x8000_0000, width)
                })
                .collect()
        })
        .unwrap_or_default();
    for (off, width) in &watch_specs {
        println!("[wm2000-watch] watching {:#010x} (rdram offset {off:#x}) width {width}", off + 0x8000_0000);
    }
    let mut watch_last: Vec<Option<u32>> = vec![None; watch_specs.len()];
'''

PROBE = '''            if !watch_specs.is_empty() {
                let view = fn64_runtime::RdramView::from_storage(&rdram);
                for (i, (off, width)) in watch_specs.iter().enumerate() {
                    let addr = fn64_runtime::RdramAddr::from_offset(*off);
                    let value = match width {
                        1 => u32::from(view.read_u8(addr)),
                        2 => u32::from(view.read_u16(addr)),
                        _ => view.read_u32(addr),
                    };
                    if watch_last[i] != Some(value) {
                        println!(
                            "[wm2000-watch] swap #{swap_count}: {:#010x} = {value:#x} (was {})",
                            off + 0x8000_0000,
                            watch_last[i].map_or("-".to_string(), |v| format!("{v:#x}"))
                        );
                        let _ = std::io::stdout().flush();
                        watch_last[i] = Some(value);
                    }
                }
            }
'''

DECL_ANCHOR = "    let mut last_applied_input: (u16, i8, i8) = (0, 0, 0);\n"
PROBE_ANCHOR = "        if swap_count > last_swap_count {\n"


def main():
    path = sys.argv[1]
    src = open(path).read()
    if "WM2000_WATCH" in src:
        print(f"[watch-patch] {path}: already patched, nothing to do")
        return
    for anchor, name in ((DECL_ANCHOR, "declaration"), (PROBE_ANCHOR, "probe")):
        n = src.count(anchor)
        if n != 1:
            sys.exit(f"[watch-patch] FATAL: {name} anchor found {n} times, expected 1. "
                     "The harness moved; fix this script rather than loosening the anchor.")
    src = src.replace(DECL_ANCHOR, DECL + DECL_ANCHOR)
    src = src.replace(PROBE_ANCHOR, PROBE_ANCHOR + PROBE)
    open(path, "w").write(src)
    print(f"[watch-patch] {path}: WM2000_WATCH probe applied")


if __name__ == "__main__":
    main()
