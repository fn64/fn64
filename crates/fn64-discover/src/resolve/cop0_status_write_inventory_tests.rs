    use super::*;
    use crate::cfg::build_cfg;

    const START: u32 = 0x8000_1000;

    fn cop0_move(rs: u8, rt: u8, cop0d: u8) -> u32 {
        (0x10 << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | ((cop0d as u32) << 11)
    }

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    #[test]
    fn inventories_typed_status_writes_without_promoting_unknown_words() {
        let image = bytes(&[
            0x3c08_0040,          // lui t0,0x0040
            cop0_move(4, 8, 12),  // mtc0 t0,Status
            0x03e0_0008,          // jr ra
            0,                    // delay
            cop0_move(5, 9, 12),  // dmtc0 t1,Status (data-shaped)
            cop0_move(4, 10, 12), // unclassified raw word
            cop0_move(4, 11, 13), // mtc0 t3,Cause (not Status)
        ]);
        let mut cfg = build_cfg("status", &image, START, &[START]);
        cfg.word_class.insert(START + 16, WordClass::ProvenData);

        let report = inventory_cop0_status_writes(&cfg, &image, START).unwrap();
        assert_eq!(report.proven_code_writes.len(), 1);
        assert_eq!(report.proven_code_writes[0].site_pc, START + 4);
        assert_eq!(report.proven_code_writes[0].source_register, 8);
        assert_eq!(report.proven_code_writes[0].kind, Cop0StatusWriteKind::Mtc0);
        assert_eq!(report.proven_data_words.len(), 1);
        assert_eq!(report.proven_data_words[0].site_pc, START + 16);
        assert_eq!(report.proven_data_words[0].kind, Cop0StatusWriteKind::Dmtc0);
        assert_eq!(report.unclassified_writes.len(), 1);
        assert_eq!(report.unclassified_writes[0].site_pc, START + 20);
        assert!(report.open_indirect_sites.is_empty());
    }

    #[test]
    fn status_value_analysis_reuses_cfg_state_and_retains_sound_blockers() {
        let constant = bytes(&[
            0x3c08_1234,         // lui t0,0x1234
            cop0_move(4, 8, 12), // mtc0 t0,Status
            0x03e0_0008,
            0,
        ]);
        let cfg = build_cfg("constant-status", &constant, START, &[START]);
        let analysis = analyze_cop0_status_writes(&cfg, &constant, START).unwrap();
        assert_eq!(analysis.inventory.proven_code_writes.len(), 1);
        assert_eq!(
            analysis.proven_code_value_proofs,
            vec![Cop0StatusValueProof {
                site_pc: START + 4,
                values: vec![0x1234_0000],
                known_zero: !0x1234_0000,
                known_one: 0x1234_0000,
                blockers: Vec::new(),
            }]
        );

        let mut mutable_load = bytes(&[
            0x3c08_8000,         // lui t0,0x8000
            0x8d09_1020,         // lw t1,0x1020(t0)
            cop0_move(4, 9, 12), // mtc0 t1,Status
            0x03e0_0008,
            0,
        ]);
        mutable_load.resize(0x24, 0);
        mutable_load[0x20..0x24].copy_from_slice(&0x3400_0000u32.to_be_bytes());
        let cfg = build_cfg("mutable-status", &mutable_load, START, &[START]);
        let analysis = analyze_cop0_status_writes(&cfg, &mutable_load, START).unwrap();
        assert_eq!(
            analysis.proven_code_value_proofs[0].values,
            vec![0x3400_0000]
        );
        assert!(analysis.proven_code_value_proofs[0]
            .blockers
            .iter()
            .any(|blocker| matches!(
                blocker,
                Cop0StatusValueBlocker::MutableStaticMemorySource { addresses }
                    if addresses == &[START + 0x20]
            )));
        assert_eq!(analysis.proven_code_value_proofs[0].known_zero, 0);
        assert_eq!(analysis.proven_code_value_proofs[0].known_one, 0);

        let unsupported = bytes(&[cop0_move(5, 4, 12), 0x03e0_0008, 0]);
        let cfg = build_cfg("dmtc0-status", &unsupported, START, &[START]);
        let analysis = analyze_cop0_status_writes(&cfg, &unsupported, START).unwrap();
        assert!(analysis.proven_code_value_proofs[0]
            .blockers
            .contains(&Cop0StatusValueBlocker::Dmtc0Unsupported));
        assert!(analysis.proven_code_value_proofs[0]
            .blockers
            .contains(&Cop0StatusValueBlocker::ValueOpen));
    }

    #[test]
    fn status_read_modify_write_retains_bev_known_zero() {
        let image = bytes(&[
            cop0_move(0, 8, 12), // mfc0 t0,Status
            0x2401_fffe,         // addiu at,zero,-2
            0x0101_4824,         // and t1,t0,at
            cop0_move(4, 9, 12), // mtc0 t1,Status
            0x03e0_0008,
            0,
        ]);
        let cfg = build_cfg("status-rmw", &image, START, &[START]);
        let analysis = analyze_cop0_status_writes(&cfg, &image, START).unwrap();
        let [proof] = analysis.proven_code_value_proofs.as_slice() else {
            panic!("expected one Status proof")
        };
        assert!(proof.values.is_empty());
        assert_eq!(proof.known_zero & COP0_STATUS_BEV, COP0_STATUS_BEV);
        assert_eq!(proof.known_zero & 1, 1);
        assert_eq!(proof.known_one, 0);
        assert_eq!(proof.blockers, vec![Cop0StatusValueBlocker::ValueOpen]);
    }

    #[test]
    fn retains_open_indirect_frontier_even_without_status_words() {
        let image = bytes(&[0x0100_0008, 0]); // jr t0; nop
        let cfg = build_cfg("indirect", &image, START, &[START]);
        let first = inventory_cop0_status_writes(&cfg, &image, START).unwrap();
        let second = inventory_cop0_status_writes(&cfg, &image, START).unwrap();
        assert_eq!(first, second);
        assert!(first.proven_code_writes.is_empty());
        assert!(first.proven_data_words.is_empty());
        assert!(first.unclassified_writes.is_empty());
        assert_eq!(first.open_indirect_sites, vec![START]);
    }

    /// Pin the four WM2000 resident `MTC0 Status` shapes that remain open, and
    /// the one preserve-mask shape that closes, using the exact instruction
    /// words read from the NWXE ROM on 2026-08-03. These are the concrete
    /// reason the `0xbfc0...` bootstrap frontier stays open: three of the four
    /// derive BEV from runtime-mutable state and the fourth ORs in an
    /// unconstrained argument. Widening any of them to "clear" would lower the
    /// admission bar rather than prove the invariant.
    #[test]
    fn wm2000_resident_status_write_shapes_keep_bev_open() {
        // `mfc0 k1,Status; addiu at,zero,-4; and k1,k1,at; mtc0 k1,Status`
        // is the WM shape at 0x800367ac: mask 0xfffffffc preserves bit 22, so
        // the `mfc0` BEV known-zero seed survives and the site proves clear.
        let preserving = bytes(&[
            cop0_move(0, 27, 12), // mfc0 k1,Status
            0x2401_fffc,          // addiu at,zero,-4
            0x0361_d824,          // and k1,k1,at
            cop0_move(4, 27, 12), // mtc0 k1,Status
            0x03e0_0008,          // jr ra
            0,
        ]);
        let cfg = build_cfg("preserving", &preserving, START, &[START]);
        let analysis = analyze_cop0_status_writes(&cfg, &preserving, START).unwrap();
        let proof = analysis
            .proven_code_value_proofs
            .iter()
            .find(|proof| proof.site_pc == START + 12)
            .expect("the masked write is proven code");
        assert_eq!(proof.known_zero & COP0_STATUS_BEV, COP0_STATUS_BEV);

        // `mfc0 t0,Status; or t0,t0,a0; mtc0 t0,Status` is the WM shape at
        // 0x8002a2c8. The caller's `$a0` is unconstrained, so BEV known-zero
        // must be dropped by the `or` rule.
        let or_with_argument = bytes(&[
            cop0_move(0, 8, 12), // mfc0 t0,Status
            0x0104_4025,         // or t0,t0,a0
            cop0_move(4, 8, 12), // mtc0 t0,Status
            0x03e0_0008,         // jr ra
            0,
            0,
        ]);
        let cfg = build_cfg("or_argument", &or_with_argument, START, &[START]);
        let analysis = analyze_cop0_status_writes(&cfg, &or_with_argument, START).unwrap();
        let proof = analysis
            .proven_code_value_proofs
            .iter()
            .find(|proof| proof.site_pc == START + 8)
            .expect("the or-with-argument write is proven code");
        assert_eq!(
            proof.known_zero & COP0_STATUS_BEV,
            0,
            "OR with an unconstrained argument must not retain BEV known-zero"
        );

        // A bare `mtc0 a0,Status` is the WM shape at 0x800376d0: a whole-Status
        // write from an unconstrained argument retains no BEV fact at all.
        let bare = bytes(&[
            cop0_move(4, 4, 12), // mtc0 a0,Status
            0x03e0_0008,         // jr ra
            0,
            0,
        ]);
        let cfg = build_cfg("bare", &bare, START, &[START]);
        let analysis = analyze_cop0_status_writes(&cfg, &bare, START).unwrap();
        let proof = analysis
            .proven_code_value_proofs
            .iter()
            .find(|proof| proof.site_pc == START)
            .expect("the bare write is proven code");
        assert_eq!(proof.known_zero & COP0_STATUS_BEV, 0);
    }

    #[test]
    fn rejects_unaligned_or_wrapping_image_geometry() {
        let cfg = build_cfg("empty", &[], START, &[]);
        assert_eq!(
            inventory_cop0_status_writes(&cfg, &[0; 3], START),
            Err(Cop0StatusWriteInventoryError::UnalignedImage)
        );
        assert_eq!(
            inventory_cop0_status_writes(&cfg, &[0; 4], u32::MAX - 3),
            Err(Cop0StatusWriteInventoryError::AddressOverflow)
        );
    }
