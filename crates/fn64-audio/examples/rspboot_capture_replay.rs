use fn64_audio::hle::{AudioHleCatalog, AudioHleCatalogEntry};
use fn64_audio::hle_outcome::{AudioHleFamily, CanonicalRdramRanges};
use fn64_audio::hle_rspboot::execute_audio_rspboot_to_entry;
use fn64_audio::task_capture::decode_audio_rspboot_capture;
use fn64_audio::whole_task::prepare_no_dpc_submission_whole_audio_task;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let rdram_path = args.next().expect("usage: rspboot_capture_replay RDRAM RSPSTATE OUTPUT");
    let state_path = args.next().expect("missing RSPSTATE path");
    let output_path = args.next().expect("missing OUTPUT path");
    assert!(args.next().is_none(), "unexpected extra argument");

    let rdram = std::fs::read(&rdram_path).expect("read captured RDRAM");
    let state = std::fs::read(&state_path).expect("read captured RSP state");
    let input = decode_audio_rspboot_capture(&state, rdram).expect("decode exact task capture");
    let boot = execute_audio_rspboot_to_entry(input.clone()).expect("execute rspboot");
    let identity = boot.entry().identity();
    let entries = [AudioHleCatalogEntry {
        identity,
        family: AudioHleFamily::StandardAbi,
        implementation_revision: 1,
    }];
    let admission = AudioHleCatalog::new(&entries)
        .expect("construct one-entry replay catalog")
        .admit(identity)
        .expect("admit captured identity");
    let prepared = prepare_no_dpc_submission_whole_audio_task(
        input,
        admission,
        CanonicalRdramRanges::default(),
    )
    .expect("execute captured whole audio task");
    std::fs::write(
        output_path,
        prepared.reference().lle_result().rdram_storage(),
    )
    .expect("write replayed final RDRAM");
    eprintln!(
        "replayed audio task: rspboot_steps={} ucode_steps={} writes={}",
        prepared.reference().steps().rspboot(),
        prepared.reference().steps().ucode(),
        prepared.reference().final_rdram_patches().as_slice().len(),
    );
    for patch in prepared.reference().final_rdram_patches().as_slice() {
        eprintln!(
            "write={:#x}+{:#x}",
            patch.range().start(),
            patch.range().byte_len()
        );
    }
}
