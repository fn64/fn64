    use super::*;
    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }
    const NOP: u32 = 0;

    #[test]
    fn probe_static_index() {
        // index arrives from a static (load-image) global, then sltiu-bounded.
        //   lui   $v0,0x8000
        //   lw    $v0, 0x00f0($v0)   ; static global initialized to 3 in image
        //   addiu $v1,$v0,0x00
        //   sltiu $v0,$v1,0x1d       ; bound 29
        //   beq   $v0,$zero,default
        //   sll   $v0,$v1,2          (delay)
        //   lui   $at,0x8000
        //   addu  $at,$at,$v0
        //   lw    $v0, 0x40($at)
        //   jr    $v0 ; nop
        // default: jr $ra ; nop
        let lui_v0 = 0x3c02_8000u32;
        let lw_glob = 0x8c42_00f0u32; // lw $v0,0xf0($v0)
        let addiu_v1 = 0x2443_0000u32; // addiu $v1,$v0,0
        let sltiu = 0x2c62_001du32;
        let beq_default = 0x1040_0006u32;
        let sll = 0x0003_1080u32;
        let lui_at = 0x3c01_8000u32;
        let addu_at = 0x0022_0821u32;
        let lw_v0 = 0x8c22_0040u32;
        let jr_v0 = 0x0040_0008u32;
        let jr_ra = 0x03e0_0008u32;
        let mut bytes = asm(&[
            lui_v0,
            lw_glob,
            addiu_v1,
            sltiu,
            beq_default,
            sll,
            lui_at,
            addu_at,
            lw_v0,
            jr_v0,
            NOP,
            jr_ra,
            NOP,
        ]);
        bytes.resize(0x200, 0);
        // static global at 0xf0 = 3
        bytes[0xf0..0xf4].copy_from_slice(&3u32.to_be_bytes());
        // 29-entry table at 0x40, all -> valid aligned in-bank targets 0x100..
        for i in 0..29usize {
            let off = 0x40 + i * 4;
            let tgt = 0x8000_0100u32 + (i as u32) * 4;
            bytes[off..off + 4].copy_from_slice(&tgt.to_be_bytes());
        }
        // put jr_ra at each target
        for i in 0..29usize {
            let t = 0x100 + i * 4;
            bytes[t..t + 4].copy_from_slice(&jr_ra.to_be_bytes());
        }
        let closure = build_cfg_value_set_closed("p", &bytes, 0x8000_0000, &[0x8000_0000]);
        for r in &closure.indirect {
            eprintln!(
                "site 0x{:08x} state={:?} kind={:?} ntargets={} nmem={}",
                r.site_pc,
                r.state,
                r.kind,
                r.targets.len(),
                r.memory_sources.len()
            );
        }
    }
