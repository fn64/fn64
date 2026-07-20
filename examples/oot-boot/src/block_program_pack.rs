//! Host-side admission for one out-of-tree generated OoT `BlockProgram`.
//!
//! The selected source is game-derived build output and is never copied into
//! fn64. Its SHA-256 is passed back into the generated builder so every bank
//! runner and the live dispatch seam carry the identity of the exact source
//! Cargo compiled.

use fn64_recomp_rs::{BlockProgram, ExecutionKey, InstructionBudget, ProgramArtifactIdentity};

mod generated {
    include!(env!("FN64_RECOMP_RS_BLOCK_PROGRAM"));
}

pub struct LoadedBlockProgram {
    pub program: BlockProgram,
    pub entry: ExecutionKey,
    pub budget: InstructionBudget,
    pub artifact_identity: ProgramArtifactIdentity,
}

pub fn load() -> LoadedBlockProgram {
    let artifact_sha256 = super::block_program_config::parse_lowercase_sha256(env!(
        "FN64_RECOMP_RS_BLOCK_PROGRAM_SHA256"
    ))
    .expect("oot-boot build script emitted an invalid block-program source SHA-256");
    let artifact_identity = ProgramArtifactIdentity::new(artifact_sha256);
    let program = generated::build_block_program(artifact_identity).unwrap_or_else(|error| {
        panic!("oot-boot: generated BlockProgram construction failed: {error}")
    });
    let entry = generated::entry();
    program.code().resolve(entry).unwrap_or_else(|fault| {
        panic!(
            "oot-boot: generated BlockProgram does not admit its declared entry {entry}: {fault}"
        )
    });
    let snapshot = program.evidence_snapshot();
    assert!(
        !snapshot.banks.is_empty(),
        "oot-boot: generated BlockProgram contains no executable banks"
    );
    for bank in &snapshot.banks {
        assert_eq!(
            bank.runner_artifact_identity, artifact_identity,
            "oot-boot: generated BlockProgram runner {} is not bound to selected pack source sha256={} ",
            bank.id,
            env!("FN64_RECOMP_RS_BLOCK_PROGRAM_SHA256")
        );
    }
    LoadedBlockProgram {
        program,
        entry,
        budget: generated::instruction_budget(),
        artifact_identity,
    }
}

pub use generated::{entry_lookup, transfer_lookup};
