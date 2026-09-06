//! Census and probe unit tests: typestate/Send bounds, the ordered CPU
//! accumulators, `word_source_bytes`, the captured-read index and the
//! graphics-task LLE deferral.

use super::*;

#[test]
fn production_backend_and_raw_dpc_typestates_are_sendable_to_one_worker() {
    fn assert_send<T: Send>() {}
    assert_send::<super::WgpuBackend>();
    assert_send::<fn64_render::BoundSubmittedRawDpc>();
    assert_send::<fn64_render::BackendPreparedRawDpc>();
}

/// `CommandIndex` round-trips through `new`/`get` and indexes
/// `Vec<ScheduledRawTriangle>` the same way a plain `usize` would --
/// pinned directly because no production call site threads a typed
/// `CommandIndex` yet (every `raw_triangle_commands` reader still takes
/// a bare `usize` "schedule position", the same parameter several of
/// them also use to index `texrect_commands`, a different Vec this type
/// does not cover); the `Index<CommandIndex>` impl exists as the
/// documented contract for a future caller that narrows to the
/// `raw_triangle_commands`-only path.
#[test]
fn command_index_round_trips_and_indexes_scheduled_raw_triangles() {
    let commands = vec![
        super::ScheduledRawTriangle {
            span: fn64_render::TriangleAccessSpan {
                first_access_index: 0,
                access_count: 0,
            },
            triangle_index: super::TriangleIndex::new(0),
            command_index: 0,
            decoded: Err(super::ScheduledRawTriangleDecodeError::MissingOpcode),
        },
        super::ScheduledRawTriangle {
            span: fn64_render::TriangleAccessSpan {
                first_access_index: 0,
                access_count: 0,
            },
            triangle_index: super::TriangleIndex::new(1),
            command_index: 7,
            decoded: Err(super::ScheduledRawTriangleDecodeError::MissingOpcode),
        },
    ];
    let index = super::CommandIndex::new(1);
    assert_eq!(index.get(), 1);
    assert_eq!(commands[index].command_index, 7);
    assert_eq!(commands[index].triangle_index, super::TriangleIndex::new(1));
}

#[test]
fn ordered_cpu_batch_moves_accumulators_and_flushes_both_compute_boundaries() {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x20_0000).unwrap();
    let key = crate::targets::ColorTargetKey::try_new(
        layout.address(0x1000).unwrap(),
        crate::targets::ColorTargetExtent::try_new(8, 2).unwrap(),
        crate::targets::ColorTargetFormat::Rgba16,
    )
    .unwrap();
    let mut registry = crate::targets::ColorTargetRegistry::try_new(layout, 2).unwrap();
    let mut batch = super::OrderedCpuColorBatch::new();

    let (first, first_seed) = batch.begin_member(&mut registry, key).unwrap();
    let (mut first_seed, mut first_coverage) =
        first_seed.expect("first generation has an explicit zero base");
    first_seed.fill(0x11);
    for pixel in 0..key.extent().pixels() as usize {
        first_coverage.set_exact(pixel, crate::Coverage::FULL);
    }
    let expected_coverage = first_coverage.clone();
    let first_pointer = first_seed.as_ptr();
    let first = completed_cpu_accumulator(first, first_seed, first_coverage);
    let reservation = batch.active.take().unwrap();
    batch.continuity =
        Some(crate::targets::OrderedCpuColorContinuity::start(reservation, &first).unwrap());
    batch.tail = Some(first);

    let (second, second_seed) = batch.begin_member(&mut registry, key).unwrap();
    let (mut second_seed, second_coverage) =
        second_seed.expect("successor consumes the prior CPU accumulator");
    assert_eq!(second_seed.as_ptr(), first_pointer);
    assert_eq!(second_coverage, expected_coverage);
    assert_eq!(second.generation().get(), 2);
    second_seed[8..16].fill(0x33);
    let second_pointer = second_seed.as_ptr();
    let second = completed_cpu_accumulator(second, second_seed, second_coverage);
    let reservation = batch.active.take().unwrap();
    batch.continuity = Some(
        batch
            .continuity
            .take()
            .unwrap()
            .append(reservation, &second)
            .unwrap(),
    );
    batch.tail = Some(second);

    // CPU -> compute: the hard boundary moves the tail into the private
    // registry, preserving both allocation identity and generation.
    batch.flush(&mut registry).unwrap();
    let resident = &registry.residents()[0];
    assert_eq!(resident.generation().get(), 2);
    assert_eq!(
        resident.device_bytes().device_bytes().as_ptr(),
        second_pointer
    );
    assert_eq!(resident.coverage(), &expected_coverage);

    // Compute -> CPU: beginning a new CPU segment observes the flushed
    // private resident and reserves its exact successor.
    let (third, third_seed) = batch.begin_member(&mut registry, key).unwrap();
    assert_eq!(third.generation().get(), 3);
    assert_eq!(
        third_seed, None,
        "a durable resident is seeded by the ordinary path"
    );
}

#[test]
fn task_guest_read_pool_shares_only_identical_range_and_bytes() {
    let range = fn64_render_ir::PhysicalRange::try_new(0x1000, 0x1004).unwrap();
    let other_range = fn64_render_ir::PhysicalRange::try_new(0x2000, 0x2004).unwrap();
    let bytes = [1, 2, 3, 4];
    let content = fn64_render_ir::FastContentDigest::hash(b"task-read-test", &[&bytes]);
    let mut pool = super::TaskGuestReadCapturePool::default();

    let first = pool.intern_parts(range, content, &bytes);
    let duplicate = pool.intern_parts(range, content, &bytes);
    assert!(first.shares_allocation_with(&duplicate));

    // The fast digest is a bucket selector, not reuse authority. This
    // deliberately supplies the same digest for different bytes to pin
    // the collision check that guards correctness.
    let collision = pool.intern_parts(range, content, &[4, 3, 2, 1]);
    assert!(!first.shares_allocation_with(&collision));

    let other_address = pool.intern_parts(other_range, content, &bytes);
    assert!(!first.shares_allocation_with(&other_address));
}

#[test]
fn owned_color_command_input_transfers_the_same_allocation() {
    let mut accumulated = Some(vec![1, 2, 3, 4]);
    let pointer = accumulated.as_ref().unwrap().as_ptr();
    let input = super::color_command_input(
        &mut accumulated,
        true,
        crate::targets::ColorTargetKey::try_new(
            fn64_render_ir::PhysicalAddress::try_new(0x1000).unwrap(),
            crate::targets::ColorTargetExtent::try_new(1, 2).unwrap(),
            crate::targets::ColorTargetFormat::Rgba16,
        )
        .unwrap(),
    )
    .unwrap();

    let std::borrow::Cow::Owned(bytes) = input else {
        panic!("the ownership lane must not downgrade to a borrowed slice");
    };
    assert!(
        accumulated.is_none(),
        "owned input consumes the accumulator"
    );
    assert_eq!(bytes, [1, 2, 3, 4]);
    assert_eq!(
        bytes.as_ptr(),
        pointer,
        "ownership transfer must retain the exact allocation"
    );
}

#[test]
fn borrowed_color_command_control_leaves_the_accumulator_owned_by_the_schedule() {
    let mut accumulated = Some(vec![1, 2, 3, 4]);
    let pointer = accumulated.as_ref().unwrap().as_ptr();
    let input = super::color_command_input(
        &mut accumulated,
        false,
        crate::targets::ColorTargetKey::try_new(
            fn64_render_ir::PhysicalAddress::try_new(0x1000).unwrap(),
            crate::targets::ColorTargetExtent::try_new(1, 2).unwrap(),
            crate::targets::ColorTargetFormat::Rgba16,
        )
        .unwrap(),
    )
    .unwrap();

    let std::borrow::Cow::Borrowed(bytes) = input else {
        panic!("the control lane must retain the former borrowed input");
    };
    assert_eq!(bytes.as_ptr(), pointer);
    drop(input);
    assert_eq!(accumulated.unwrap(), [1, 2, 3, 4]);
}

/// **The seed's byte-lane inversion, pinned against a hand-built pair.**
///
/// Mutation-driven: replacing `captured[word + lane]` with
/// `captured[index]` -- dropping the inversion entirely -- survived the
/// whole unit suite AND the differential sweep. The sweep misses it
/// because its fixture seeds every framebuffer halfword to the same
/// value (`STALE = 0xffff`), which is a palindrome under any byte
/// permutation, so a swapped seed reads back identical.
///
/// This fixture uses four DISTINCT bytes per word, so every one of the
/// 24 possible permutations gives a different answer.
///
/// Expectation derived from `fn64-runtime`'s own mapping, not from the
/// function: `RdramViewMut::write_u8` indexes `range(addr, 1, 3)`, i.e.
/// `offset ^ 3` within the word, and this is that inverse. So storage
/// `[0, 1, 2, 3]` is logical `[3, 2, 1, 0]`.
#[test]
fn a_captured_rdram_seed_is_unswizzled_into_logical_order() {
    assert_eq!(
        super::logical_bytes_from_captured_rdram(&[0, 1, 2, 3]),
        vec![3, 2, 1, 0],
        "one word: logical[i] must be storage[i ^ 3]"
    );
    // Two words, so a mutant that inverts across the WHOLE buffer
    // rather than within each word is also caught: whole-buffer
    // inversion would give [7, 6, 5, 4, 3, 2, 1, 0].
    assert_eq!(
        super::logical_bytes_from_captured_rdram(&[0, 1, 2, 3, 4, 5, 6, 7]),
        vec![3, 2, 1, 0, 7, 6, 5, 4],
        "the swap is per aligned word, never across the buffer"
    );
    // Round trip: applying it twice is the identity, which a mutant
    // using a different XOR constant (^1 or ^2) would still satisfy --
    // so this is an additional check, not the load-bearing one above.
    let storage: Vec<u8> = (0..16).collect();
    assert_eq!(
        super::logical_bytes_from_captured_rdram(&super::logical_bytes_from_captured_rdram(
            &storage
        )),
        storage,
        "the lane swap is its own inverse"
    );
}

#[test]
fn compute_replacement_threshold_is_inclusive_and_keeps_small_work_on_cpu() {
    assert!(!compute_raster_replacement_admitted(16_383, 16_384));
    assert!(compute_raster_replacement_admitted(16_384, 16_384));
    assert!(compute_raster_replacement_admitted(16_385, 16_384));
}

/// A transfer word's `source_access_byte_offset` is relative to **its
/// own** source access, never to a flattened concatenation of the
/// load's whole source run (`tmem::physical`'s word projection
/// subtracts the preceding accesses' byte total before storing it).
/// A partial-width `LoadTile` declares one source read per row, so
/// resolving every word against row 0 would silently feed the wrong
/// row's pixels to every word after the first -- a wrong picture, not
/// a crash.
///
/// Each row here is filled with its own row index, so a
/// misresolved row is directly observable in the returned bytes.
#[test]
fn word_source_bytes_slices_the_row_the_word_names_not_the_first_row() {
    const ROWS: u32 = 4;
    const ROW_BYTES: usize = 8;
    const FIRST_ACCESS: u32 = 1;

    let rows: Vec<Vec<u8>> = (0..ROWS).map(|row| vec![row as u8; ROW_BYTES]).collect();
    let (reads, source_accesses) = indexed_source_rows(FIRST_ACCESS, &rows);

    for row in 0..ROWS {
        let word = TmemTransferWord::new(
            row as u16,
            row * ROW_BYTES as u32,
            FIRST_ACCESS + row,
            0,
            0xff,
            0xff,
            row as u16,
            0,
            false,
            crate::TmemTransferPhysicalWord::Linear(
                fn64_render_ir::TmemRange::try_new(row * 8, row * 8 + 8).unwrap(),
            ),
        );
        let bytes = word_source_bytes(&reads, &source_accesses, FIRST_ACCESS, word)
            .expect("every word binds to a row in the run");
        assert_eq!(
            bytes,
            &[row as u8; ROW_BYTES],
            "word naming access {} must read row {row}, not row 0",
            FIRST_ACCESS + row
        );
    }
}

/// The offset is applied *within* the named row, and a word that would
/// read past that row's end is refused rather than silently spilling
/// into the next row's captured bytes.
#[test]
fn word_source_bytes_refuses_a_word_that_overruns_its_own_row() {
    const ROW_BYTES: usize = 8;
    let rows = [vec![0xaa_u8; ROW_BYTES], vec![0xbb_u8; ROW_BYTES]];
    let (reads, source_accesses) = indexed_source_rows(1, &rows);

    // Offset 4 with 8 defined bytes runs 4 bytes past row 0's end. If
    // the rows were flattened this would happily return 4 bytes of
    // row 0 followed by 4 bytes of row 1.
    let overrun = TmemTransferWord::new(
        0,
        4,
        1,
        4,
        0xff,
        0xff,
        0,
        0,
        false,
        crate::TmemTransferPhysicalWord::Linear(
            fn64_render_ir::TmemRange::try_new(0, 8).unwrap(),
        ),
    );
    assert!(
        word_source_bytes(&reads, &source_accesses, 1, overrun).is_none(),
        "a word may not read past the end of the row it names"
    );

    // A word naming an access outside the run is refused too, in both
    // directions.
    let before_run = TmemTransferWord::new(
        0,
        0,
        0,
        0,
        0xff,
        0xff,
        0,
        0,
        false,
        crate::TmemTransferPhysicalWord::Linear(
            fn64_render_ir::TmemRange::try_new(0, 8).unwrap(),
        ),
    );
    assert!(word_source_bytes(&reads, &source_accesses, 1, before_run).is_none());
    let past_run = TmemTransferWord::new(
        0,
        0,
        9,
        0,
        0xff,
        0xff,
        0,
        0,
        false,
        crate::TmemTransferPhysicalWord::Linear(
            fn64_render_ir::TmemRange::try_new(0, 8).unwrap(),
        ),
    );
    assert!(word_source_bytes(&reads, &source_accesses, 1, past_run).is_none());
}

#[test]
fn word_source_bytes_refuses_a_different_exact_source_access() {
    let rows = [vec![0xaa_u8; 8]];
    let (reads, mut source_accesses) = indexed_source_rows(1, &rows);
    let original = source_accesses[0];
    source_accesses[0] = ResourceAccess::try_new(
        original.operation(),
        AccessMode::Read,
        AccessPurpose::UploadSource,
        original.region(),
    )
    .unwrap();
    let word = TmemTransferWord::new(
        0,
        0,
        1,
        0,
        0xff,
        0xff,
        0,
        0,
        false,
        crate::TmemTransferPhysicalWord::Linear(
            fn64_render_ir::TmemRange::try_new(0, 8).unwrap(),
        ),
    );
    assert!(word_source_bytes(&reads, &source_accesses, 1, word).is_none());
}

#[test]
fn captured_read_index_refuses_missing_binding() {
    let (_, accesses, _) = captured_binding_fixture();
    let mut authority = CapturedGuestReadAuthority::default();
    assert!(matches!(
        authority.bind_accesses(&accesses),
        Err(WgpuRawDpcExecutionError::MissingCapturedSourceAccess { .. })
    ));
}

#[test]
fn captured_read_index_refuses_duplicate_binding() {
    let (read, accesses, bytes) = captured_binding_fixture();
    let mut authority = CapturedGuestReadAuthority::default();
    authority.push(read, CapturedGuestReadBytes::copied(&bytes));
    authority.push(read, CapturedGuestReadBytes::copied(&bytes));
    assert!(matches!(
        authority.bind_accesses(&accesses),
        Err(WgpuRawDpcExecutionError::DuplicateCapturedSource { .. })
    ));
}

#[test]
fn captured_read_index_refuses_out_of_range_binding() {
    let (read, _, bytes) = captured_binding_fixture();
    let mut authority = CapturedGuestReadAuthority::default();
    authority.push(read, CapturedGuestReadBytes::copied(&bytes));
    assert!(matches!(
        authority.bind_accesses(&[]),
        Err(WgpuRawDpcExecutionError::CapturedSourceAccessOutOfRange { .. })
    ));
}

#[test]
fn captured_read_index_refuses_wrong_access_binding() {
    let (read, mut accesses, bytes) = captured_binding_fixture();
    let wrong = ResourceAccess::try_new(
        read.operation(),
        AccessMode::Read,
        AccessPurpose::UploadSource,
        ResourceRegion::Rdram {
            resource: read.resource(),
            range: read.range(),
        },
    )
    .unwrap();
    accesses[read.access_index() as usize] = wrong;
    let mut authority = CapturedGuestReadAuthority::default();
    authority.push(read, CapturedGuestReadBytes::copied(&bytes));
    assert!(matches!(
        authority.bind_accesses(&accesses),
        Err(WgpuRawDpcExecutionError::CapturedSourceAccessMismatch { .. })
    ));
}

/// A graphics task is a disposition, not an error: this backend has no
/// HLE display-list front end, so it reports `NeedsLle` and `fn64-abi`
/// runs the microcode on the RSP. Measured on WM2000 (NWXE), whose gfx
/// tasks carry a real F3DEX2 display list under an uncatalogued IMEM
/// digest -- `ReferenceBackend` returns `NeedsLle` for those same tasks.
#[test]
fn a_graphics_task_defers_to_lle_with_the_live_imem_digest() {
    let mut backend = WgpuBackend::try_new().unwrap().0;
    let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
    let mut rsp_memory = rsp_memory_with_imem(b"wm2000-uncatalogued-geometry-microcode");
    let expected =
        fn64_render::UcodeDigest::from_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
            .as_bytes();

    let task = fn64_render::OsTask {
        task_type: fn64_render::M_GFXTASK,
        data_ptr: COMMAND_START,
        data_size: 64,
        ..fn64_render::OsTask::default()
    };
    let status = backend
        .process_task(&mut rdram, &mut rsp_memory, &task, 0)
        .expect("a graphics task is a disposition, not a backend error");

    assert_eq!(
        status,
        fn64_render::FrameStatus::NeedsLle {
            ucode_sha256: expected
        },
        "the reported microcode identity must be the live IMEM digest"
    );
}

/// The digest is read from live IMEM, not from the task header or a
/// constant, so a different microcode reports a different identity. A
/// mutant returning a fixed digest, or hashing the task's `ucode` image
/// instead, fails here.
#[test]
fn the_deferred_ucode_digest_tracks_live_imem_not_a_constant() {
    let mut backend = WgpuBackend::try_new().unwrap().0;
    let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
    let task = fn64_render::OsTask {
        task_type: fn64_render::M_GFXTASK,
        ..fn64_render::OsTask::default()
    };

    let mut first = rsp_memory_with_imem(b"microcode-a");
    let mut second = rsp_memory_with_imem(b"microcode-b");
    let a = backend
        .process_task(&mut rdram, &mut first, &task, 0)
        .unwrap();
    let b = backend
        .process_task(&mut rdram, &mut second, &task, 0)
        .unwrap();

    assert_ne!(
        a, b,
        "two different live microcodes must not report one identity"
    );
    for (memory, status) in [(&mut first, a), (&mut second, b)] {
        let fn64_render::FrameStatus::NeedsLle { ucode_sha256 } = status else {
            panic!("a graphics task must defer to LLE, got {status:?}");
        };
        assert_eq!(
            ucode_sha256,
            fn64_render::UcodeDigest::from_text(memory.bank(fn64_runtime::RspMemoryBank::Imem))
                .as_bytes(),
            "the digest must be the live IMEM bank's"
        );
    }
}

/// Deferring a graphics task must not become a shrug that swallows a
/// routing bug. A non-graphics task is still a loud named error, and the
/// message names the type it received rather than saying "out of scope".
#[test]
fn a_non_graphics_task_is_still_refused_by_name() {
    let mut backend = WgpuBackend::try_new().unwrap().0;
    let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
    let mut rsp_memory = fn64_runtime::RspMemory::new();
    let audio = fn64_render::OsTask {
        task_type: fn64_render::M_GFXTASK + 1,
        ..fn64_render::OsTask::default()
    };

    let error = backend
        .process_task(&mut rdram, &mut rsp_memory, &audio, 0)
        .expect_err("a non-graphics task at this seam is a routing bug");
    let reason = error.to_string();
    assert!(
        reason.contains(&(fn64_render::M_GFXTASK + 1).to_string()),
        "the refusal must name the task type it received: {reason}"
    );
}

/// A graphics task must not mutate guest memory on the way to its
/// deferral: `fn64-abi` runs the very same task through LLE afterwards,
/// and a half-applied prefix would be executed twice.
#[test]
fn deferring_a_graphics_task_leaves_guest_memory_untouched() {
    let mut backend = WgpuBackend::try_new().unwrap().0;
    let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
    for (index, byte) in rdram.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    let before = rdram.clone();
    let mut rsp_memory = rsp_memory_with_imem(b"uncatalogued");
    let rsp_before = rsp_memory.clone();

    let task = fn64_render::OsTask {
        task_type: fn64_render::M_GFXTASK,
        data_ptr: COMMAND_START,
        data_size: 64,
        ..fn64_render::OsTask::default()
    };
    backend
        .process_task(&mut rdram, &mut rsp_memory, &task, 0)
        .expect("a graphics task defers rather than failing");

    assert_eq!(rdram, before, "deferral must not write guest RDRAM");
    assert_eq!(
        rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem),
        rsp_before.bank(fn64_runtime::RspMemoryBank::Imem),
        "deferral must not write RSP IMEM"
    );
}
