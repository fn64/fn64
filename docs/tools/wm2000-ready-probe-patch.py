import re, sys
p = "/private/tmp/wm2000-rc-probe/scratch/sib/recomps/wm2000/packages/wm2000-boot/src/main.rs"
src = open(p).read()

anchor = "            last_swap_count = swap_count;\n"
assert src.count(anchor) == 1, src.count(anchor)

probe = r'''
            // ---- WM2000_READY_PROBE (scratch, read-only) ----
            // fn64 backs RDRAM word-swapped: Rdram::store_h writes at
            // backing_offset(vaddr) ^ 2. So read halfwords at (off ^ 2) and
            // decode little-endian; read bytes at (off ^ 3) likewise.
            if let Some((lo, hi)) = ready_probe_window {
                if swap_count >= lo && swap_count <= hi {
                    let rd = |off: u32| -> u16 {
                        let o = ((off & 0x00ff_ffff) ^ 2) as usize;
                        u16::from_le_bytes([rdram[o], rdram[o + 1]])
                    };
                    let rb = |off: u32| -> u8 {
                        rdram[((off & 0x00ff_ffff) ^ 3) as usize]
                    };
                    let rw = |off: u32| -> u32 {
                        let o = (off & 0x00ff_ffff) as usize;
                        u32::from_be_bytes([rdram[o], rdram[o+1], rdram[o+2], rdram[o+3]])
                    };
                    // D_801702A4 is a POINTER; entries live at *ptr + 0x512, stride 0x88.
                    let base_ptr = rw(0x001702A4);
                    let mut ports = [0u16; 4];
                    let mut e00 = [0u16; 4];
                    if base_ptr >= 0x8000_0000 {
                        let entries = (base_ptr - 0x8000_0000) + 0x512;
                        for i in 0..4u32 {
                            let e = entries + i * 0x88;
                            ports[i as usize] = rd(e + 0x16) & 0xF;
                            e00[i as usize] = rd(e);
                        }
                    }
                    // D_80095180 array, stride 12 per port: +4 held, +6 pressed, +8 released
                    let mut held = [0u16; 4];
                    let mut pressed = [0u16; 4];
                    for i in 0..4u32 {
                        held[i as usize] = rd(0x00095180 + i * 12 + 4);
                        pressed[i as usize] = rd(0x00095180 + i * 12 + 6);
                    }
                    // OSContPad[] at D_80057210, stride 6: +0 button, +4 errno byte (D_80057214)
                    let mut pad_btn = [0u16; 4];
                    let mut pad_errno = [0u8; 4];
                    for i in 0..4u32 {
                        pad_btn[i as usize] = rd(0x00057210 + i * 6);
                        pad_errno[i as usize] = rb(0x00057214 + i * 6);
                    }
                    // D_8011BF50 slot array (4 words), the "joined" flags read by func_801456C8
                    let mut slots = [0u32; 4];
                    for i in 0..4u32 {
                        slots[i as usize] = rw(0x0011BF50 + i * 4);
                    }
                    println!(
                        "[wm2000-ready] swap={} screen={} bitpat_F8={:#04x} bitpat_F9={:#04x} \
                         D_800573FA={:#04x} D_8011C37E={:#06x} entbase={:#010x} \
                         ports={:?} ent0x00={:x?} slots={:x?} held={:04x?} pressed={:04x?} \
                         pad_btn={:04x?} pad_errno={:?} D_80161FF8={:#010x} D_800FEF2C={:#x}",
                        swap_count,
                        rd(0x0003DD04),
                        rb(0x000573F8), rb(0x000573F9), rb(0x000573FA),
                        rd(0x0011C37E),
                        base_ptr,
                        ports, e00, slots, held, pressed, pad_btn, pad_errno,
                        rw(0x00161FF8),
                        rw(0x000FEF2C),
                    );
                    let _ = std::io::stdout().flush();
                }
            }
'''

src = src.replace(anchor, probe + anchor, 1)

# declare ready_probe_window before the main loop
decl_anchor = "    let mut steps = 0u64;\n    let mut drain = fn64_boot_harness::GuestDrain::default();\n"
assert src.count(decl_anchor) == 1
decl = '''    // WM2000_READY_PROBE=<lo>-<hi>: per-swap read-only dump of the
    // four-player ready-check inputs (scratch probe, not upstream).
    let ready_probe_window: Option<(u64, u64)> = std::env::var("WM2000_READY_PROBE")
        .ok()
        .map(|raw| {
            let (a, b) = raw.split_once('-').expect("WM2000_READY_PROBE=<lo>-<hi>");
            (a.parse::<u64>().unwrap(), b.parse::<u64>().unwrap())
        });
'''
src = src.replace(decl_anchor, decl + decl_anchor, 1)
open(p, "w").write(src)
print("patched")
