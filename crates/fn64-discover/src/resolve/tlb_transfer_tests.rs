    use super::*;
    use crate::cfg::build_cfg;

    const START: u32 = 0x8000_1000;
    const NOP: u32 = 0;

    fn mtc0(rt: u8, cop0d: u8) -> u32 {
        (0x10 << 26) | (4 << 21) | ((rt as u32) << 16) | ((cop0d as u32) << 11)
    }

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn setup_words() -> Vec<u32> {
        vec![
            0x2408_0001, // addiu t0,zero,1: Index
            0x2409_001f, // addiu t1,zero,0x1f: EntryLo0
            0x240a_0001, // addiu t2,zero,1: EntryLo1
            0x3c0b_007f, // lui t3,0x007f
            0x356b_e000, // ori t3,t3,0xe000: PageMask
            0x3c0c_7000, // lui t4,0x7000: EntryHi
            mtc0(8, 0),
            mtc0(9, 2),
            mtc0(10, 3),
            mtc0(11, 5),
            mtc0(12, 10),
        ]
    }

    #[test]
    fn constant_tlbwi_is_correlated_with_post_delay_transfer() {
        let mut words = setup_words();
        words.extend([
            0x4200_0002, // tlbwi
            0x3c0d_7000, // lui t5,0x7000
            0x35ad_0510, // ori t5,t5,0x0510
            0x01a0_0008, // jr t5
            NOP,
        ]);
        let image = bytes(&words);
        let cfg = build_cfg("tlb-constant", &image, START, &[START]);
        let analysis = analyze_constant_tlb_transfers(&cfg, &image, START);
        let [transfer] = analysis.transfers.as_slice() else {
            panic!("expected one computed transfer")
        };
        assert_eq!(transfer.transfer_pc, START + 14 * 4);
        assert_eq!(transfer.target, Some(0x7000_0510));
        assert_eq!(transfer.entry_hi_at_transfer, Some(0x7000_0000));
        assert!(transfer.blockers.is_empty());
        assert_eq!(
            transfer.active_writes,
            vec![TlbWriteProofV1 {
                tlbwi_pc: START + 11 * 4,
                index_raw: 1,
                page_mask_raw: 0x007f_e000,
                entry_hi_raw: 0x7000_0000,
                entry_lo0_raw: 0x1f,
                entry_lo1_raw: 0x1,
            }]
        );
    }

    #[test]
    fn bypass_path_cannot_inherit_tlb_write() {
        let mut words = setup_words();
        words.extend([
            0x1080_0004, // beq a0,zero,join
            NOP,
            0x4200_0002, // tlbwi
            0x0800_0410, // j START+0x40 (join)
            NOP,
            0x3c0d_7000, // join: lui t5,0x7000
            0x35ad_0510,
            0x01a0_0008,
            NOP,
        ]);
        let image = bytes(&words);
        let cfg = build_cfg("tlb-bypass", &image, START, &[START]);
        let analysis = analyze_constant_tlb_transfers(&cfg, &image, START);
        let transfer = analysis
            .transfers
            .iter()
            .find(|transfer| transfer.target == Some(0x7000_0510))
            .expect("joined transfer");
        assert!(transfer.active_writes.is_empty());
        assert!(transfer
            .blockers
            .contains(&TlbTransferBlockerV1::TlbPathDisagreement));
        assert!(transfer
            .blockers
            .contains(&TlbTransferBlockerV1::NoProvenTlbWrite));
    }

    #[test]
    fn delay_slot_tlb_mutation_blocks_transfer() {
        let mut words = setup_words();
        words.extend([
            0x4200_0002, // tlbwi
            0x3c0d_7000,
            0x35ad_0510,
            0x01a0_0008, // jr t5
            0x4200_0006, // tlbwr in the architectural delay slot
        ]);
        let image = bytes(&words);
        let cfg = build_cfg("tlb-delay", &image, START, &[START]);
        let analysis = analyze_constant_tlb_transfers(&cfg, &image, START);
        let [transfer] = analysis.transfers.as_slice() else {
            panic!("expected one computed transfer")
        };
        assert_eq!(transfer.target, Some(0x7000_0510));
        assert!(transfer.active_writes.is_empty());
        assert!(transfer
            .blockers
            .contains(&TlbTransferBlockerV1::RandomIndexedWrite));
        assert!(transfer
            .blockers
            .contains(&TlbTransferBlockerV1::NoProvenTlbWrite));
    }

    #[test]
    fn transfer_entry_hi_is_sampled_after_the_delay_slot() {
        let mut words = setup_words();
        words.extend([
            0x4200_0002, // tlbwi
            0x3c0d_7000,
            0x35ad_0510,
            0x3c0e_7000,
            0x35ce_0001,  // t6 = transfer-time EntryHi with ASID 1
            0x01a0_0008,  // jr t5
            mtc0(14, 10), // architectural delay slot
        ]);
        let image = bytes(&words);
        let cfg = build_cfg("tlb-entry-hi-delay", &image, START, &[START]);
        let analysis = analyze_constant_tlb_transfers(&cfg, &image, START);
        let [transfer] = analysis.transfers.as_slice() else {
            panic!("expected one computed transfer")
        };
        assert_eq!(transfer.target, Some(0x7000_0510));
        assert_eq!(transfer.entry_hi_at_transfer, Some(0x7000_0001));
        assert!(transfer.blockers.is_empty());
    }

    #[test]
    fn open_target_is_retained_without_indexing_an_empty_set() {
        let image = bytes(&[0x0080_0008, NOP]); // jr a0; nop
        let cfg = build_cfg("tlb-open-target", &image, START, &[START]);
        let analysis = analyze_constant_tlb_transfers(&cfg, &image, START);
        let [transfer] = analysis.transfers.as_slice() else {
            panic!("expected one computed transfer")
        };
        assert_eq!(transfer.target, None);
        assert!(transfer
            .blockers
            .contains(&TlbTransferBlockerV1::TargetOpen));
        assert!(transfer
            .blockers
            .contains(&TlbTransferBlockerV1::NoProvenTlbWrite));
    }
