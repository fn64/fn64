    use super::*;

    const START: u32 = 0x8000_1000;
    const WRAPPER: u32 = START + 0x20;
    const INNER_DMA: u32 = 0x8000_1800;

    fn i(op: u32, rs: u8, rt: u8, immediate: i16) -> u32 {
        (op << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | immediate as u16 as u32
    }

    fn r(rs: u8, rt: u8, rd: u8, funct: u32) -> u32 {
        ((rs as u32) << 21) | ((rt as u32) << 16) | ((rd as u32) << 11) | funct
    }

    fn jal(target: u32) -> u32 {
        (0x03 << 26) | ((target >> 2) & 0x03ff_ffff)
    }

    fn wrapper_image() -> Vec<u32> {
        vec![
            jal(WRAPPER),
            0,
            jal(WRAPPER),
            0,
            0x03e0_0008,
            0,
            0,
            0,
            i(0x09, 29, 29, -0x20),
            i(0x2b, 29, 4, 0x20),
            i(0x2b, 29, 5, 0x24),
            i(0x2b, 29, 6, 0x28),
            i(0x23, 29, 8, 0x28),
            i(0x23, 29, 9, 0x24),
            r(8, 9, 10, 0x23),
            i(0x09, 0, 11, -16),
            r(10, 11, 10, 0x24),
            i(0x2b, 29, 10, 0x1c),
            i(0x23, 29, 7, 0x24),
            i(0x23, 29, 11, 0x20),
            i(0x2b, 29, 11, 0x10),
            i(0x23, 29, 12, 0x1c),
            i(0x09, 0, 6, 0),
            jal(INNER_DMA),
            i(0x2b, 29, 12, 0x14),
            i(0x23, 29, 13, 0x20),
            i(0x23, 29, 14, 0x1c),
            r(13, 14, 13, 0x21),
            i(0x2b, 29, 13, 0x20),
            i(0x23, 29, 15, 0x24),
            r(15, 14, 15, 0x21),
            i(0x2b, 29, 15, 0x24),
            i(0x23, 29, 24, 0x1c),
            r(24, 14, 24, 0x23),
            i(0x2b, 29, 24, 0x1c),
            i(0x05, 24, 0, -18),
            0,
            0x03e0_0008,
            i(0x09, 29, 29, 0x20),
        ]
    }

    #[test]
    fn infers_end_address_chunk_wrapper_from_semantics() {
        let image = wrapper_image();
        let report = infer_physical_end_dma_wrappers(&image, START);
        assert!(!report.candidate_limit_hit);
        assert_eq!(report.candidates_examined, 1);
        assert_eq!(
            report.admitted,
            [PhysicalEndDmaWrapper {
                entry_va: WRAPPER,
                callers: vec![START, START + 8],
                nested_dma_call_pc: START + 23 * 4,
            }]
        );

        let mut db = crate::facts::FactDb::new();
        let diagnostics = crate::record_physical_end_dma_wrapper_candidates(&image, START, &mut db);
        assert_eq!(diagnostics.semantic_proof_unavailable, 1);
        assert!(db.proven_rom_mappings().is_empty());
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            crate::facts::Fact::Evidence { note, .. }
                if note.contains("CFG/path and inner-callee authority remain open")
        )));
    }

    #[test]
    fn rejects_count_semantics_and_non_looping_copy_shapes() {
        let mut count_semantics = wrapper_image();
        count_semantics[14] = r(8, 0, 10, 0x21);
        assert!(infer_physical_end_dma_wrappers(&count_semantics, START)
            .admitted
            .is_empty());

        let mut no_loop = wrapper_image();
        no_loop[35] = 0;
        assert!(infer_physical_end_dma_wrappers(&no_loop, START)
            .admitted
            .is_empty());
    }

    #[test]
    fn rejection_census_names_the_fact_the_candidate_failed() {
        // The facts are not independent: the loop, cursor, and remaining-length
        // facts are only evaluated once the inner DMA call is recognized, which
        // itself needs the length dataflow. So breaking the length computation
        // cascades, and the census reports every fact left unestablished rather
        // than a single root cause. What must hold is that the broken fact is
        // always named, and that a fact broken in isolation is named alone.
        let mut count_semantics = wrapper_image();
        count_semantics[14] = r(8, 0, 10, 0x21);
        let report = infer_physical_end_dma_wrappers(&count_semantics, START);
        assert_eq!(report.candidates_examined, 1);
        assert_eq!(report.rejections.no_end_minus_start, 1);

        // Breaking only the backward branch leaves every earlier fact intact,
        // so exactly one counter moves.
        let mut no_loop = wrapper_image();
        no_loop[35] = 0;
        let report = infer_physical_end_dma_wrappers(&no_loop, START);
        assert_eq!(
            report.rejections,
            WrapperRejectionCensus {
                no_backward_loop: 1,
                ..WrapperRejectionCensus::default()
            }
        );

        // An admitted candidate contributes nothing to the census.
        let report = infer_physical_end_dma_wrappers(&wrapper_image(), START);
        assert_eq!(report.admitted.len(), 1);
        assert_eq!(report.rejections, WrapperRejectionCensus::default());
    }

    #[test]
    fn rejects_a_single_caller_even_when_the_body_matches() {
        let mut image = wrapper_image();
        image[2] = 0;
        let report = infer_physical_end_dma_wrappers(&image, START);
        assert_eq!(report.candidates_examined, 0);
        assert!(report.admitted.is_empty());
    }
