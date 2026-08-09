use crate::*;

pub(crate) const DENSE_AOT_ARTIFACTS: &[DenseAotArtifact] = &[
    DenseAotArtifact {
        bank_id: revenge_block_shard_00::BANK_ID,
        code_bank: revenge_block_shard_00::code_bank,
        runner: revenge_block_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_01::BANK_ID,
        code_bank: revenge_block_shard_01::code_bank,
        runner: revenge_block_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_02::BANK_ID,
        code_bank: revenge_block_shard_02::code_bank,
        runner: revenge_block_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_03::BANK_ID,
        code_bank: revenge_block_shard_03::code_bank,
        runner: revenge_block_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_04::BANK_ID,
        code_bank: revenge_block_shard_04::code_bank,
        runner: revenge_block_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_05::BANK_ID,
        code_bank: revenge_block_shard_05::code_bank,
        runner: revenge_block_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_06::BANK_ID,
        code_bank: revenge_block_shard_06::code_bank,
        runner: revenge_block_shard_06::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_07::BANK_ID,
        code_bank: revenge_block_shard_07::code_bank,
        runner: revenge_block_shard_07::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_shard_08::BANK_ID,
        code_bank: revenge_block_shard_08::code_bank,
        runner: revenge_block_shard_08::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_resident_tail_shard_00::BANK_ID,
        code_bank: revenge_block_resident_tail_shard_00::code_bank,
        runner: revenge_block_resident_tail_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_resident_tail_shard_01::BANK_ID,
        code_bank: revenge_block_resident_tail_shard_01::code_bank,
        runner: revenge_block_resident_tail_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_resident_tail_shard_02::BANK_ID,
        code_bank: revenge_block_resident_tail_shard_02::code_bank,
        runner: revenge_block_resident_tail_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_resident_tail_shard_03::BANK_ID,
        code_bank: revenge_block_resident_tail_shard_03::code_bank,
        runner: revenge_block_resident_tail_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_resident_tail_shard_04::BANK_ID,
        code_bank: revenge_block_resident_tail_shard_04::code_bank,
        runner: revenge_block_resident_tail_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_resident_tail_shard_05::BANK_ID,
        code_bank: revenge_block_resident_tail_shard_05::code_bank,
        runner: revenge_block_resident_tail_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_resident_tail_shard_06::BANK_ID,
        code_bank: revenge_block_resident_tail_shard_06::code_bank,
        runner: revenge_block_resident_tail_shard_06::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_0_shard_00::BANK_ID,
        code_bank: revenge_block_overlay_0_shard_00::code_bank,
        runner: revenge_block_overlay_0_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_0_shard_01::BANK_ID,
        code_bank: revenge_block_overlay_0_shard_01::code_bank,
        runner: revenge_block_overlay_0_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_0_shard_02::BANK_ID,
        code_bank: revenge_block_overlay_0_shard_02::code_bank,
        runner: revenge_block_overlay_0_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_0_shard_03::BANK_ID,
        code_bank: revenge_block_overlay_0_shard_03::code_bank,
        runner: revenge_block_overlay_0_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_1_shard_00::BANK_ID,
        code_bank: revenge_block_overlay_1_shard_00::code_bank,
        runner: revenge_block_overlay_1_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_1_shard_01::BANK_ID,
        code_bank: revenge_block_overlay_1_shard_01::code_bank,
        runner: revenge_block_overlay_1_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_1_shard_02::BANK_ID,
        code_bank: revenge_block_overlay_1_shard_02::code_bank,
        runner: revenge_block_overlay_1_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_1_shard_03::BANK_ID,
        code_bank: revenge_block_overlay_1_shard_03::code_bank,
        runner: revenge_block_overlay_1_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_1_shard_04::BANK_ID,
        code_bank: revenge_block_overlay_1_shard_04::code_bank,
        runner: revenge_block_overlay_1_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: revenge_block_overlay_1_shard_05::BANK_ID,
        code_bank: revenge_block_overlay_1_shard_05::code_bank,
        runner: revenge_block_overlay_1_shard_05::run,
    },
];

pub(crate) const DENSE_AOT_IDENTITIES: &[LinkedDenseIdentity] = &[
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_00::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_01::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_02::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_03::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_04::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_05::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_06::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_06::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_07::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_07::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_shard_08::SOURCE_SHA256,
        runner_source_sha256: revenge_block_shard_08::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_resident_tail_shard_00::SOURCE_SHA256,
        runner_source_sha256: revenge_block_resident_tail_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_resident_tail_shard_01::SOURCE_SHA256,
        runner_source_sha256: revenge_block_resident_tail_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_resident_tail_shard_02::SOURCE_SHA256,
        runner_source_sha256: revenge_block_resident_tail_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_resident_tail_shard_03::SOURCE_SHA256,
        runner_source_sha256: revenge_block_resident_tail_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_resident_tail_shard_04::SOURCE_SHA256,
        runner_source_sha256: revenge_block_resident_tail_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_resident_tail_shard_05::SOURCE_SHA256,
        runner_source_sha256: revenge_block_resident_tail_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_resident_tail_shard_06::SOURCE_SHA256,
        runner_source_sha256: revenge_block_resident_tail_shard_06::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_0_shard_00::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_0_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_0_shard_01::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_0_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_0_shard_02::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_0_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_0_shard_03::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_0_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_1_shard_00::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_1_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_1_shard_01::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_1_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_1_shard_02::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_1_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_1_shard_03::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_1_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_1_shard_04::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_1_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: revenge_block_overlay_1_shard_05::SOURCE_SHA256,
        runner_source_sha256: revenge_block_overlay_1_shard_05::RUNNER_SOURCE_SHA256,
    },
];
