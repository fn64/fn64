//! Deterministic, synthetic replay of the production ordered-task conveyor.
//!
//! Recipes contain only builder inputs. Every invocation mints a fresh
//! backend/session pair and reconstructs every submission, read binding,
//! completion and publication capability through the production constructors.

use super::*;
use fn64_render::ir_effect_content_digest;
use fn64_render_ir::{ContentDigest, ResourceRegion};
use fn64_runtime::{RdramAddr, RdramViewMut};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const DEFAULT_MEMBERS: usize = 13;

#[derive(Clone, Debug)]
struct StructuralPacket {
    words: Vec<u32>,
    read_payloads: Vec<Vec<u8>>,
}

/// Auditable, game-content-free inputs for one ordered renderer task.
#[derive(Clone, Debug)]
pub(super) struct StructuralTaskRecipe {
    width: u32,
    height: u32,
    seed: u32,
    members: usize,
}

impl StructuralTaskRecipe {
    pub(super) fn hot_chain(seed: u32) -> Self {
        Self {
            width: 16,
            height: 8,
            seed,
            members: DEFAULT_MEMBERS,
        }
    }

    #[cfg(test)]
    pub(super) fn with_members(mut self, members: usize) -> Self {
        assert!(
            members >= 3,
            "a replay needs clear, setup/load, and draw members"
        );
        self.members = members;
        self
    }

    fn packets(&self) -> Vec<StructuralPacket> {
        let base_color = 0x2040_6081u32 ^ self.seed.rotate_left(7);
        let texture = vec![
            0x0843 ^ self.seed as u16,
            0x4211 ^ self.seed.rotate_left(3) as u16,
            0x7bef ^ self.seed.rotate_left(9) as u16,
            0xf801 ^ self.seed.rotate_left(13) as u16,
        ];
        let frame = Rdp::new(self.width, self.height)
            .cycle(CycleType::One)
            .combine_prim_passthrough()
            .prim_color(base_color)
            .texture(0, 4, 1, texture);
        let mut packets = vec![
            StructuralPacket {
                words: frame.clear_words(),
                read_payloads: Vec::new(),
            },
            StructuralPacket {
                words: frame.draw_words(),
                read_payloads: frame
                    .textures
                    .iter()
                    .map(StagedTexture::source_bytes)
                    .collect(),
            },
        ];
        for index in 2..self.members {
            let pixel = ((base_color as u16).wrapping_add((index as u16).wrapping_mul(0x0711))) | 1;
            let color = u32::from(pixel) << 16 | u32::from(pixel);
            let x1 = self.width - 1;
            let y1 = self.height - 1;
            let mut words = Vec::new();
            words.extend(set_other_mode(CycleType::Fill as u32, 0));
            words.extend(frame.set_color_image());
            words.extend([word(SET_FILL_COLOR, 0), color]);
            words.extend([word(FILL_RECTANGLE, ((x1 << 2) << 12) | (y1 << 2)), 0]);
            packets.push(StructuralPacket {
                words,
                read_payloads: Vec::new(),
            });
        }
        packets
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GenerationReceipt {
    pub(super) member: usize,
    pub(super) write_ranges: Vec<(u32, u32)>,
    pub(super) write_contents: Vec<ContentDigest>,
    pub(super) payload_sha256: [u8; 32],
    pub(super) tmem_generation: u64,
    pub(super) tmem_sha256: [u8; 32],
    pub(super) color_generation: Option<u64>,
    pub(super) color_sha256: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TaskReplayReceipt {
    pub(super) generations: Vec<GenerationReceipt>,
    pub(super) zero_read_members: usize,
    pub(super) postimage_sha256: [u8; 32],
    pub(super) normalized_sha256: [u8; 32],
}

pub(super) fn replay_fresh(recipe: &StructuralTaskRecipe) -> TaskReplayReceipt {
    let (mut backend, mut session) = WgpuBackend::try_new().expect("fresh replay backend");
    configure_extent(&mut backend, recipe.width, recipe.height);
    let packets = recipe.packets();
    let planned = backend
        .plan_raw_dpc_task_batch(
            packets
                .iter()
                .enumerate()
                .map(|(index, packet)| {
                    session.plan_request(capture_sequence(
                        packet.words.clone(),
                        u64::try_from(index + 1).expect("bounded member index"),
                    ))
                })
                .collect(),
        )
        .expect("plan synthetic task");

    let mut zero_read_members = 0usize;
    let bounds = planned
        .into_iter()
        .zip(&packets)
        .map(|(planned, packet)| {
            let reads = planned.guest_read_plan().reads();
            zero_read_members += usize::from(reads.is_empty());
            assert_eq!(
                reads.len(),
                packet.read_payloads.len(),
                "synthetic packet must bind every declared read exactly once"
            );
            let captured = DeferredGuestReadCapture::new(
                reads
                    .iter()
                    .zip(&packet.read_payloads)
                    .map(|(read, payload)| {
                        let mut bytes = payload.clone();
                        bytes.resize(read.range().len() as usize, 0);
                        CapturedGuestRead::try_new(*read, bytes)
                            .expect("payload sized to its declared read")
                    })
                    .collect(),
            );
            session
                .finalize_and_submit(planned, captured)
                .expect("finalize synthetic task member")
        })
        .collect();

    let prepared = backend
        .execute_raw_dpc_task_batch(bounds)
        .expect("execute synthetic task");

    let mut guest = vec![0xa5; LAYOUT_BYTES as usize];
    let mut generations = Vec::with_capacity(prepared.len());
    for (member, prepared) in prepared.into_iter().enumerate() {
        let submission = prepared.submission();
        let writes = backend.staged_guest_render_target_writes(submission);
        let payloads = backend.committed_guest_render_target_bytes(submission);
        assert_eq!(
            writes.len(),
            payloads.len(),
            "one payload per completed write"
        );
        for (write, payload) in writes.iter().zip(&payloads) {
            assert_eq!(payload.len() as u32, write.byte_count());
            assert_eq!(ir_effect_content_digest(payload), write.content());
        }
        let committed = if writes.is_empty() {
            session
                .commit_zero_guest_writes(prepared)
                .expect("commit zero-write member")
        } else {
            session
                .commit_guest_render_target_writes(prepared, writes.clone())
                .expect("commit guest-write member")
        };
        assert_eq!(committed.submission(), submission);
        for (write, payload) in writes.iter().zip(&payloads) {
            let ResourceRegion::Rdram { range, .. } = write.access().region() else {
                panic!("renderer guest write must name RDRAM")
            };
            RdramViewMut::from_storage(&mut guest)
                .write_logical_bytes(RdramAddr::from_offset(range.start().get()), payload);
        }
        let mut fabric = admitted_fabric();
        let token = fabric
            .pending_dpc_submission()
            .expect("pending fabric")
            .token;
        let ready = fabric.prepare_dpc_commit(token).expect("prepare fabric");
        let capsule = session
            .seal_publication(committed, ready)
            .expect("seal synthetic publication");
        assert_eq!(capsule.submission(), submission);
        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);

        let tmem = crate::project_committed_tmem(backend.physical_tmem());
        let resident = backend.color_targets().and_then(|registry| {
            registry
                .residents()
                .iter()
                .find(|resident| resident.key().address().get() == TARGET_ADDRESS)
        });
        generations.push(GenerationReceipt {
            member,
            write_ranges: writes
                .iter()
                .map(|write| match write.access().region() {
                    ResourceRegion::Rdram { range, .. } => {
                        (range.start().get(), write.byte_count())
                    }
                    _ => unreachable!("validated guest write names RDRAM"),
                })
                .collect(),
            write_contents: writes.iter().map(|write| write.content()).collect(),
            payload_sha256: sha256(payloads.iter().flat_map(|payload| payload.iter().copied())),
            tmem_generation: backend.physical_tmem().generation(),
            tmem_sha256: sha256(
                tmem.bytes.iter().copied().chain(
                    tmem.validity_words
                        .iter()
                        .flat_map(|word| word.to_le_bytes()),
                ),
            ),
            color_generation: resident.map(|resident| resident.generation().get()),
            color_sha256: resident
                .map(|resident| sha256(resident.device_bytes().device_bytes().iter().copied())),
        });
    }
    let postimage_sha256 = sha256(guest.iter().copied());
    let normalized_sha256 = normalized_digest(&generations, zero_read_members, postimage_sha256);
    TaskReplayReceipt {
        generations,
        zero_read_members,
        postimage_sha256,
        normalized_sha256,
    }
}

fn capture_sequence(words: Vec<u32>, sequence: u64) -> fn64_render::OwnedRawDpcCapture {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
    let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
    let submission = OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words).unwrap();
    fn64_render::OwnedRawDpcCapture::new(
        submission,
        layout,
        sequence,
        TemporalBoundary::new(sequence, DpInterruptState::Clear),
    )
}

fn sha256(bytes: impl IntoIterator<Item = u8>) -> [u8; 32] {
    let mut hash = Sha256::new();
    for byte in bytes {
        hash.update([byte]);
    }
    hash.finalize().into()
}

fn normalized_digest(
    generations: &[GenerationReceipt],
    zero_read_members: usize,
    postimage: [u8; 32],
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"fn64.synthetic-task-replay.v1\0");
    hash.update((zero_read_members as u64).to_le_bytes());
    for generation in generations {
        hash.update((generation.member as u64).to_le_bytes());
        hash.update((generation.write_ranges.len() as u64).to_le_bytes());
        for ((start, len), content) in generation
            .write_ranges
            .iter()
            .zip(&generation.write_contents)
        {
            hash.update(start.to_le_bytes());
            hash.update(len.to_le_bytes());
            hash.update(content.as_ref());
        }
        hash.update(generation.payload_sha256);
        hash.update(generation.tmem_generation.to_le_bytes());
        hash.update(generation.tmem_sha256);
        hash.update(generation.color_generation.unwrap_or(0).to_le_bytes());
        hash.update(generation.color_sha256.unwrap_or([0; 32]));
    }
    hash.update(postimage);
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_authorities_produce_the_same_normalized_generation_receipt() {
        let recipe = StructuralTaskRecipe::hot_chain(0x51a7_c3d9);
        let first = replay_fresh(&recipe);
        let second = replay_fresh(&recipe);
        assert_eq!(first, second);
        assert_eq!(first.generations.len(), DEFAULT_MEMBERS);
        assert_eq!(first.zero_read_members, DEFAULT_MEMBERS - 1);
    }

    #[test]
    fn synthetic_input_mutation_changes_the_frozen_receipt_domain() {
        let baseline = replay_fresh(&StructuralTaskRecipe::hot_chain(0x51a7_c3d9));
        let mutated = replay_fresh(&StructuralTaskRecipe::hot_chain(0x51a7_c3d8));
        assert_ne!(baseline.normalized_sha256, mutated.normalized_sha256);
        assert_ne!(baseline.postimage_sha256, mutated.postimage_sha256);
    }

    #[test]
    fn default_chain_has_one_load_member_and_a_zero_read_hot_majority() {
        let receipt = replay_fresh(&StructuralTaskRecipe::hot_chain(0x51a7_c3d9));
        assert_eq!(receipt.zero_read_members, DEFAULT_MEMBERS - 1);
        assert_eq!(receipt.generations[0].tmem_generation, 0);
        assert_eq!(receipt.generations[1].tmem_generation, 1);
        assert!(receipt.generations[2..]
            .iter()
            .all(|generation| generation.tmem_generation == 1));
        assert_eq!(
            receipt
                .generations
                .last()
                .and_then(|generation| generation.color_generation),
            Some((DEFAULT_MEMBERS - 1) as u64)
        );
    }

    #[test]
    fn normalized_receipt_is_frozen_for_the_default_synthetic_chain() {
        let receipt = replay_fresh(&StructuralTaskRecipe::hot_chain(0x51a7_c3d9));
        assert_eq!(
            hex(receipt.normalized_sha256),
            "a74aba557d654da22f2c585b41e1bae80cd8f69160a754c3b71003c680f9468f"
        );
    }

    #[test]
    #[ignore = "release-only 80k-plan inner-loop timing; not a correctness gate"]
    fn eighty_thousand_plan_structural_task_replay() {
        const PLANS: usize = 80_000;
        let recipe = StructuralTaskRecipe::hot_chain(0x51a7_c3d9).with_members(100);
        let iterations = PLANS / recipe.members;
        let started = Instant::now();
        let mut plan_total = Duration::ZERO;
        let packets = recipe.packets();
        let mut planned_members = 0usize;
        for _ in 0..iterations {
            let (mut backend, session) = WgpuBackend::try_new().expect("fresh replay backend");
            RenderBackend::resize(&mut backend, recipe.width, recipe.height);
            let plan_started = Instant::now();
            let planned = backend
                .plan_raw_dpc_task_batch(
                    packets
                        .iter()
                        .enumerate()
                        .map(|(index, packet)| {
                            session.plan_request(capture_sequence(
                                packet.words.clone(),
                                u64::try_from(index + 1).expect("bounded member index"),
                            ))
                        })
                        .collect(),
                )
                .expect("plan synthetic task");
            plan_total += plan_started.elapsed();
            planned_members += std::hint::black_box(planned.len());
        }
        assert_eq!(planned_members, PLANS);
        eprintln!(
            "[structural-task-replay] plans={} wall_ms={:.3} plan_ms={:.3}",
            planned_members,
            started.elapsed().as_secs_f64() * 1e3,
            plan_total.as_secs_f64() * 1e3,
        );
    }

    fn hex(bytes: [u8; 32]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
