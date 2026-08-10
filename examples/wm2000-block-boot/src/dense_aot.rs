use crate::*;

pub(crate) const DENSE_AOT_ARTIFACTS: &[DenseAotArtifact] = &[
    DenseAotArtifact {
        bank_id: wm2000_block_shard_00::BANK_ID,
        code_bank: wm2000_block_shard_00::code_bank,
        runner: wm2000_block_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_01::BANK_ID,
        code_bank: wm2000_block_shard_01::code_bank,
        runner: wm2000_block_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_02::BANK_ID,
        code_bank: wm2000_block_shard_02::code_bank,
        runner: wm2000_block_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_03::BANK_ID,
        code_bank: wm2000_block_shard_03::code_bank,
        runner: wm2000_block_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_04::BANK_ID,
        code_bank: wm2000_block_shard_04::code_bank,
        runner: wm2000_block_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_05::BANK_ID,
        code_bank: wm2000_block_shard_05::code_bank,
        runner: wm2000_block_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_06::BANK_ID,
        code_bank: wm2000_block_shard_06::code_bank,
        runner: wm2000_block_shard_06::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_07::BANK_ID,
        code_bank: wm2000_block_shard_07::code_bank,
        runner: wm2000_block_shard_07::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_08::BANK_ID,
        code_bank: wm2000_block_shard_08::code_bank,
        runner: wm2000_block_shard_08::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_09::BANK_ID,
        code_bank: wm2000_block_shard_09::code_bank,
        runner: wm2000_block_shard_09::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_10::BANK_ID,
        code_bank: wm2000_block_shard_10::code_bank,
        runner: wm2000_block_shard_10::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_11::BANK_ID,
        code_bank: wm2000_block_shard_11::code_bank,
        runner: wm2000_block_shard_11::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_12::BANK_ID,
        code_bank: wm2000_block_shard_12::code_bank,
        runner: wm2000_block_shard_12::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_13::BANK_ID,
        code_bank: wm2000_block_shard_13::code_bank,
        runner: wm2000_block_shard_13::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_14::BANK_ID,
        code_bank: wm2000_block_shard_14::code_bank,
        runner: wm2000_block_shard_14::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_resident_tail_shard_00::BANK_ID,
        code_bank: wm2000_block_resident_tail_shard_00::code_bank,
        runner: wm2000_block_resident_tail_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_resident_tail_shard_01::BANK_ID,
        code_bank: wm2000_block_resident_tail_shard_01::code_bank,
        runner: wm2000_block_resident_tail_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_0_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_0_shard_00::code_bank,
        runner: wm2000_block_overlay_0_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_0_shard_01::BANK_ID,
        code_bank: wm2000_block_overlay_0_shard_01::code_bank,
        runner: wm2000_block_overlay_0_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_0_shard_02::BANK_ID,
        code_bank: wm2000_block_overlay_0_shard_02::code_bank,
        runner: wm2000_block_overlay_0_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_1_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_1_shard_00::code_bank,
        runner: wm2000_block_overlay_1_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_00::code_bank,
        runner: wm2000_block_overlay_2_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_01::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_01::code_bank,
        runner: wm2000_block_overlay_2_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_02::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_02::code_bank,
        runner: wm2000_block_overlay_2_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_03::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_03::code_bank,
        runner: wm2000_block_overlay_2_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_04::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_04::code_bank,
        runner: wm2000_block_overlay_2_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_05::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_05::code_bank,
        runner: wm2000_block_overlay_2_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_00::code_bank,
        runner: wm2000_block_overlay_3_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_01::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_01::code_bank,
        runner: wm2000_block_overlay_3_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_02::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_02::code_bank,
        runner: wm2000_block_overlay_3_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_03::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_03::code_bank,
        runner: wm2000_block_overlay_3_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_04::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_04::code_bank,
        runner: wm2000_block_overlay_3_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_05::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_05::code_bank,
        runner: wm2000_block_overlay_3_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_06::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_06::code_bank,
        runner: wm2000_block_overlay_3_shard_06::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_07::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_07::code_bank,
        runner: wm2000_block_overlay_3_shard_07::run,
    },
];

pub(crate) const DENSE_AOT_IDENTITIES: &[LinkedDenseIdentity] = &[
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_03::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_04::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_05::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_06::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_06::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_07::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_07::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_08::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_08::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_09::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_09::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_10::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_10::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_11::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_11::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_12::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_12::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_13::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_13::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_14::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_14::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_resident_tail_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_resident_tail_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_resident_tail_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_resident_tail_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_0_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_0_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_0_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_0_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_0_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_0_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_1_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_1_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_03::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_04::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_05::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_03::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_04::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_05::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_06::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_06::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_07::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_07::RUNNER_SOURCE_SHA256,
    },
];
