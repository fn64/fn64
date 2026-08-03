    use super::*;

    #[test]
    fn word_and_byte_entry_points_match_across_return_boundaries() {
        let words: [u32; 7] = [
            0x3c08_a460,
            0,
            0x03e0_0008,
            0x3c09_a460,
            0x3c0a_a460,
            0x03e0_0008,
            0,
        ];
        let bytes = words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();

        let from_words = recover_pi_primitives_words(&words, 0x8000_1000);
        assert_eq!(from_words, recover_pi_primitives(&bytes, 0x8000_1000));
        assert_eq!(
            from_words
                .iter()
                .map(|primitive| (primitive.entry_va, primitive.register_site_pcs.clone()))
                .collect::<Vec<_>>(),
            vec![
                (0x8000_1010, vec![0x8000_100c, 0x8000_1010]),
                (0x8000_1000, vec![0x8000_1000]),
            ]
        );
    }
