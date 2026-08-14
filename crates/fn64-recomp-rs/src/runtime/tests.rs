    use crate::boot::{
        BootCicIdentity, BootContext, BootCop0Context, BootRegion, BootTvStandard, Sha256Digest,
        BOOT_CONTEXT_SCHEMA_V1,
    };

    use super::{
        resolve_host_function, set_host_lookup, set_unsupported_observer, trap_unsupported,
        DataAccessError, DataAccessKind, GuestReadEvent, GuestWriteEvent,
        HostFunctionCatalogErrorV1, HostFunctionCatalogV1, InstructionTranslationDiagnosticErrorV1,
        Rdram, RecompContext, RecompFunc, TlbEntryRaw, TlbFault, TlbFaultKind,
        TranslatedDataAddress, TranslatedInstructionAddress, WriterChannel, RDRAM_LEN,
    };

    type RdramOperation = for<'a> fn(&mut Rdram<'a>);

    thread_local! {
        static OBSERVED_WRITES: std::cell::RefCell<Vec<GuestWriteEvent>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static OBSERVED_READS: std::cell::RefCell<Vec<GuestReadEvent>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static MMIO_CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static MMIO_WRITES: std::cell::RefCell<Vec<(u64, u32)>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static UNSUPPORTED_CONTEXTS: std::cell::RefCell<Vec<String>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    fn observe_write(event: GuestWriteEvent) {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().push(event));
    }

    fn observe_read(event: GuestReadEvent) {
        OBSERVED_READS.with(|reads| reads.borrow_mut().push(event));
    }

    fn consume_mmio(vaddr: u64, value: u32) -> bool {
        MMIO_CALLS.with(|calls| calls.set(calls.get() + 1));
        MMIO_WRITES.with(|writes| writes.borrow_mut().push((vaddr, value)));
        true
    }

    fn read_mmio(_vaddr: u64) -> Option<u32> {
        MMIO_CALLS.with(|calls| calls.set(calls.get() + 1));
        Some(0)
    }

    fn read_pattern_mmio(_vaddr: u64) -> Option<u32> {
        MMIO_CALLS.with(|calls| calls.set(calls.get() + 1));
        Some(0x81a2_c3e4)
    }

    fn observe_unsupported(context: &str) {
        UNSUPPORTED_CONTEXTS.with(|contexts| contexts.borrow_mut().push(context.to_owned()));
    }

    fn first_host(_ctx: &mut RecompContext, _mem: &mut Rdram<'_>) {}

    fn second_host(_ctx: &mut RecompContext, _mem: &mut Rdram<'_>) {}

    fn legacy_host_lookup(target: u32) -> Option<RecompFunc> {
        (target == 0x8000_3000).then_some(second_host)
    }

    fn context_from_evidence_for_test(
        snapshot: &super::RecompContextEvidenceSnapshotV1,
    ) -> RecompContext {
        RecompContext {
            r: snapshot.gprs,
            hi: snapshot.hi,
            lo: snapshot.lo,
            fpr: super::FprFile {
                fgr: snapshot.physical_fgrs,
            },
            fpu_cond: snapshot.fpu_cond,
            fcsr: snapshot.fcsr,
            ll_reservation: snapshot.ll_reservation,
            cop0_count: snapshot.cop0_count,
            // Boundary-owned clock phase is synchronized by the executor and
            // deliberately absent from RecompContext-owned evidence.
            cop0_count_phase: 0,
            cop0_compare: snapshot.cop0_compare,
            cop0_count_write: snapshot.cop0_count_write,
            cop0_compare_write: snapshot.cop0_compare_write,
            cop0_cond: snapshot.cop0_cond,
            cop0_status: snapshot.cop0_status,
            cop0_cause: snapshot.cop0_cause,
            cop0_epc: snapshot.cop0_epc,
            cop0_error_epc: snapshot.cop0_error_epc,
            cop0_badvaddr: snapshot.cop0_badvaddr,
            cop0_context: snapshot.cop0_context,
            cop0_xcontext: snapshot.cop0_xcontext,
            cop0_index: snapshot.cop0_index,
            tlb_entries: snapshot.tlb_entries,
            cop0_entry_lo0: snapshot.cop0_entry_lo0,
            cop0_entry_lo1: snapshot.cop0_entry_lo1,
            cop0_page_mask: snapshot.cop0_page_mask,
            cop0_wired: snapshot.cop0_wired,
            cop0_entry_hi: snapshot.cop0_entry_hi,
            cop0_random_phase: snapshot.cop0_random_phase,
            cop0_watch_lo: snapshot.cop0_watch_lo,
            cop0_watch_hi: snapshot.cop0_watch_hi,
            os_interrupt_mask: snapshot.os_interrupt_mask,
            thread_return_pc: snapshot.thread_return_pc,
            indirect_transfers: std::collections::VecDeque::new(),
        }
    }

    #[test]
    fn recomp_context_evidence_v1_round_trips_and_detects_each_owned_field() {
        let mut context = RecompContext::new();
        context.r = std::array::from_fn(|index| index as u64 * 0x101 + 7);
        context.r[0] = 0;
        context.hi = 0x0102_0304_0506_0708;
        context.lo = 0x1112_1314_1516_1718;
        context.fpr.fgr =
            std::array::from_fn(|index| 0x8000_0000_0000_0000u64 | (index as u64 * 0x0101_0101));
        context.fpu_cond = true;
        context.fcsr = 0x0102_0304;
        context.ll_reservation = Some((0xffff_ffff_8123_4560, 8));
        context.cop0_count = 0x1111_1111;
        context.cop0_compare = 0x2222_2222;
        context.cop0_count_write = Some(0x3333_3333);
        context.cop0_compare_write = Some(0x4444_4444);
        context.cop0_cond = true;
        context.cop0_status = 0x5555_5555;
        context.cop0_cause = 0x6666_6666;
        context.cop0_epc = 0x7777_7777;
        context.cop0_error_epc = 0x8888_8888;
        context.cop0_badvaddr = 0x9999_9999_aaaa_aaaa;
        context.cop0_context = 0xbbbb_bbbb;
        context.cop0_xcontext = 0xcccc_cccc_dddd_dddd;
        context.cop0_index = 17;
        context.tlb_entries = std::array::from_fn(|index| TlbEntryRaw {
            page_mask: index as u32 * 0x2000,
            entry_hi: 0x1000_0000_0000_0000 | index as u64,
            entry_lo0: 0x2000_0000 | index as u32,
            entry_lo1: 0x3000_0000 | index as u32,
        });
        context.cop0_entry_lo0 = 0xdddd_dddd;
        context.cop0_entry_lo1 = 0xeeee_eeee;
        context.cop0_page_mask = 0x01ff_e000;
        context.cop0_wired = 11;
        context.cop0_entry_hi = 0xffff_ffff_0123_4567;
        context.cop0_random_phase = 9;
        context.cop0_watch_lo = 0x1234_5678;
        context.cop0_watch_hi = 0x9abc_def0;
        context.os_interrupt_mask = 0x1357_9bdf;
        context.thread_return_pc = Some(0xffff_fffc);

        let baseline = context.evidence_snapshot_v1();
        let restored = context_from_evidence_for_test(&baseline);
        assert_eq!(restored.evidence_snapshot_v1(), baseline);

        macro_rules! changed {
            ($change:expr) => {{
                let mut candidate = baseline.clone();
                $change(&mut candidate);
                assert_ne!(candidate, baseline);
            }};
        }
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.gprs[1] ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.lo ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.physical_fgrs[31] ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.fpu_cond = !s.fpu_cond);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.fcsr ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.ll_reservation = None);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_count ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_compare ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_count_write = None);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_compare_write = None);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_cond = !s.cop0_cond);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_status ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_cause ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_epc ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_error_epc ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_badvaddr ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_context ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_xcontext ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_index ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].page_mask ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].entry_hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].entry_lo0 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].entry_lo1 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_entry_lo0 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_entry_lo1 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_page_mask ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_wired ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_entry_hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_random_phase ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_watch_lo ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_watch_hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.os_interrupt_mask ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.thread_return_pc = None);

        context.record_indirect_transfer(1, 2, 3, 4, Some(5));
        assert_eq!(context.evidence_snapshot_v1(), baseline);
    }

    #[test]
    fn host_function_catalog_canonicalizes_and_resolves_exact_targets() {
        let catalog =
            HostFunctionCatalogV1::new(vec![(0x8000_2000, second_host), (0x8000_1000, first_host)])
                .unwrap();

        assert_eq!(catalog.target_pcs(), &[0x8000_1000, 0x8000_2000]);
        assert_eq!(catalog.len(), 2);
        assert!(!catalog.is_empty());
        assert!(std::ptr::fn_addr_eq(
            catalog.resolve(0x8000_1000).unwrap(),
            first_host as RecompFunc
        ));
        assert!(std::ptr::fn_addr_eq(
            catalog.resolve(0x8000_2000).unwrap(),
            second_host as RecompFunc
        ));
        assert!(catalog.resolve(0x8000_1004).is_none());
    }

    #[test]
    fn host_function_catalog_rejects_misaligned_and_duplicate_targets() {
        assert!(matches!(
            HostFunctionCatalogV1::new(vec![(0x8000_1002, first_host)]),
            Err(HostFunctionCatalogErrorV1::MisalignedTarget {
                target: 0x8000_1002
            })
        ));
        assert!(matches!(
            HostFunctionCatalogV1::new(
                vec![(0x8000_1000, first_host), (0x8000_1000, second_host),]
            ),
            Err(HostFunctionCatalogErrorV1::DuplicateTarget {
                target: 0x8000_1000
            })
        ));
    }

    #[test]
    fn empty_host_function_catalog_is_an_exact_empty_inventory() {
        let catalog = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        assert!(catalog.is_empty());
        assert_eq!(catalog.len(), 0);
        assert!(catalog.target_pcs().is_empty());
        assert!(catalog.resolve(0x8000_1000).is_none());
    }

    #[test]
    fn host_function_catalog_does_not_install_or_replace_legacy_lookup() {
        let previous = set_host_lookup(Some(legacy_host_lookup));
        let catalog = HostFunctionCatalogV1::new(vec![(0x8000_1000, first_host)]).unwrap();

        assert!(catalog.resolve(0x8000_3000).is_none());
        assert!(std::ptr::fn_addr_eq(
            resolve_host_function(0x8000_3000).unwrap(),
            second_host as RecompFunc
        ));
        set_host_lookup(previous);
    }

    #[test]
    fn boot_context_restores_gpr_hilo_and_modeled_cp0_state() {
        let mut gprs = [0u64; 32];
        gprs[20] = 0xffff_ffff_cafe_babe;
        gprs[29] = 0xffff_ffff_a400_1ff0;
        let mut cp0 = [0u64; 32];
        cp0[0] = 7;
        cp0[1] = 19;
        cp0[4] = 0x1234_5678;
        cp0[6] = 4;
        cp0[8] = 0xaaaa_bbbb_cccc_dddd;
        cp0[9] = 0x0102_0304;
        cp0[10] = 0xeeee_ffff_0102_0304;
        cp0[11] = 0x0506_0708;
        cp0[12] = 0x3404_0000;
        cp0[13] = 0x0000_0300;
        cp0[20] = 0x1111_2222_3333_4444;
        let boot = BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "synthetic debugger".to_string(),
            normalized_rom_sha256: Sha256Digest::from_bytes([0x11; 32]),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::from_bytes([0x22; 32]),
            },
            region: BootRegion {
                destination_code: b'E',
                tv_standard: BootTvStandard::Ntsc,
            },
            entry_pc: 0x8000_0400,
            gprs,
            hi: 0x1234,
            lo: 0x5678,
            cp0: BootCop0Context { registers: cp0 },
        };

        let mut ctx = RecompContext::new();
        ctx.restore_boot_context(&boot).unwrap();

        assert_eq!(ctx.gprs(), gprs);
        assert_eq!(ctx.hi, 0x1234);
        assert_eq!(ctx.lo, 0x5678);
        assert_eq!(ctx.cop0_random(), 19);
        assert_eq!(ctx.cop0_index, 7);
        assert_eq!(ctx.cop0_context, 0x1234_5678);
        assert_eq!(ctx.cop0_badvaddr, 0xaaaa_bbbb_cccc_dddd);
        assert_eq!(ctx.cop0_count, 0x0102_0304);
        assert_eq!(ctx.cop0_entry_hi, 0xeeee_ffff_0102_0304);
        assert_eq!(ctx.cop0_compare, 0x0506_0708);
        assert_eq!(ctx.cop0_status, 0x3404_0000);
        assert_eq!(ctx.cop0_cause, 0x0000_0300);
        assert_eq!(ctx.cop0_xcontext, 0x1111_2222_3333_4444);
        assert!(ctx.cop0_cond);
        assert!(ctx.boot_context_state_mismatches(&boot).unwrap().is_empty());

        ctx.set_r(20, 0);
        assert_eq!(
            ctx.boot_context_state_mismatches(&boot).unwrap(),
            vec![crate::boot::BootContextStateMismatch {
                field: crate::boot::BootContextStateField::Gpr(20),
                expected: 0xffff_ffff_cafe_babe,
                actual: 0,
            }]
        );
    }

    #[test]
    fn unsupported_observer_runs_before_the_named_panic() {
        UNSUPPORTED_CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
        let previous = set_unsupported_observer(Some(observe_unsupported));
        let panic = std::panic::catch_unwind(|| trap_unsupported("unsupported COP0 register 7"));
        set_unsupported_observer(previous);

        assert!(panic.is_err());
        UNSUPPORTED_CONTEXTS.with(|contexts| {
            assert_eq!(
                contexts.borrow().as_slice(),
                ["unsupported COP0 register 7"]
            );
        });
    }

    #[test]
    fn exception_return_prefers_error_epc_and_preserves_exl_under_erl() {
        let mut ctx = RecompContext::new();
        ctx.cop0_status = (1 << 1) | (1 << 2);
        ctx.cop0_epc = 0x8000_1000;
        ctx.cop0_error_epc = 0xBFC0_0200;
        ctx.set_ll_reservation(0x8000_0040, 4);

        assert_eq!(ctx.exception_return_pc(), 0xBFC0_0200);
        assert_eq!(ctx.cop0_status & (1 << 2), 0);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert!(!ctx.take_ll_reservation(0x8000_0040, 4));
    }

    #[test]
    fn cop0_status_and_software_interrupt_writes_preserve_hardware_pending() {
        let mut ctx = RecompContext::new();
        ctx.write_cop0(12, 0x3400_FF01);
        assert_eq!(ctx.read_cop0(12), 0x3400_FF01);

        ctx.cop0_cause = (1 << 10) | (9 << 2) | (1 << 31);
        ctx.write_cop0(13, 0b10 << 8);
        assert_eq!(ctx.cop0_cause & (0b11 << 8), 0b10 << 8);
        assert_ne!(ctx.cop0_cause & (1 << 10), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 9);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
    }

    #[test]
    fn cop0_timing_writes_retain_same_value_compare_acknowledgements() {
        let mut ctx = RecompContext::new();
        ctx.synchronize_cop0_timing(7, 0, 9);
        ctx.cop0_cause = 1 << 15;
        ctx.write_cop0(9, 7);
        ctx.write_cop0(11, 9);

        assert_eq!(ctx.cop0_cause & (1 << 15), 0);
        assert_eq!(ctx.take_cop0_timing_writes(), (Some(7), Some(9)));
        assert_eq!(ctx.take_cop0_timing_writes(), (None, None));
    }

    #[test]
    fn interior_count_reads_include_the_live_half_rate_phase() {
        let mut ctx = RecompContext::new();

        ctx.synchronize_cop0_timing(7, 0, 9);
        assert_eq!(ctx.read_cop0_count_interior(0), 7);
        assert_eq!(ctx.read_cop0_count_interior(1), 7);
        assert_eq!(ctx.read_cop0_count_interior(2), 8);

        ctx.synchronize_cop0_timing(7, 1, 9);
        assert_eq!(ctx.read_cop0_count_interior(0), 7);
        assert_eq!(ctx.read_cop0_count_interior(1), 8);
        assert_eq!(ctx.read_cop0_count_interior(2), 8);
    }

    #[test]
    #[should_panic(expected = "CP0 Count half-rate phase must be zero or one")]
    fn cop0_timing_sync_rejects_an_invalid_half_rate_phase() {
        let mut ctx = RecompContext::new();
        ctx.synchronize_cop0_timing(0, 2, 0);
    }

    #[test]
    fn rdram_write_observer_runs_after_committed_logical_ranges() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous = super::set_write_observer(Some(observe_write));
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_8000_0000, 0x1122_3344);
        mem.store_h(0xFFFF_FFFF_8000_0004, 0x5566);
        mem.store_h(0xFFFF_FFFF_8000_0004, 0x5566);
        mem.store_b(0xFFFF_FFFF_8000_0006, 0x77);
        mem.store_d(0xFFFF_FFFF_A000_0008, 0x8899_aabb_ccdd_eeff);

        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, 0x1122_3344);
        assert_eq!(mem.load_hu(0xFFFF_FFFF_8000_0004), 0x5566);
        assert_eq!(mem.load_bu(0xFFFF_FFFF_8000_0006), 0x77);
        assert_eq!(mem.load_d(0xFFFF_FFFF_8000_0008), 0x8899_aabb_ccdd_eeff);
        assert_eq!(
            OBSERVED_WRITES.with(|writes| writes.borrow().clone()),
            vec![
                GuestWriteEvent::Range {
                    channel: WriterChannel::CpuInstructionStore,
                    physical_offset: 0,
                    len: 4,
                },
                GuestWriteEvent::NonRdpWrite16 {
                    channel: WriterChannel::CpuInstructionStore,
                    logical_offset: 4,
                    value: 0x5566,
                },
                GuestWriteEvent::NonRdpWrite16 {
                    channel: WriterChannel::CpuInstructionStore,
                    logical_offset: 4,
                    value: 0x5566,
                },
                GuestWriteEvent::Range {
                    channel: WriterChannel::CpuInstructionStore,
                    physical_offset: 6,
                    len: 1,
                },
                GuestWriteEvent::Range {
                    channel: WriterChannel::CpuInstructionStore,
                    physical_offset: 8,
                    len: 8,
                },
            ]
        );
        super::set_write_observer(previous);
    }

    #[test]
    fn translated_rdram_read_observer_covers_every_ordinary_load() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 64];
        let mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();
        let base = 0xffff_ffff_8000_0000;

        assert!(mem.try_load_w_translated(&ctx, base).is_ok());
        assert!(mem.try_load_h_translated(&ctx, base + 4).is_ok());
        assert!(mem.try_load_hu_translated(&ctx, base + 6).is_ok());
        assert!(mem.try_load_b_translated(&ctx, base + 8).is_ok());
        assert!(mem.try_load_bu_translated(&ctx, base + 9).is_ok());
        assert!(mem.try_load_d_translated(&ctx, base + 16).is_ok());
        super::set_read_observer(previous);

        assert_eq!(
            OBSERVED_READS.with(|reads| reads.borrow().clone()),
            vec![
                GuestReadEvent {
                    physical_offset: 0,
                    len: 4,
                },
                GuestReadEvent {
                    physical_offset: 4,
                    len: 2,
                },
                GuestReadEvent {
                    physical_offset: 6,
                    len: 2,
                },
                GuestReadEvent {
                    physical_offset: 8,
                    len: 1,
                },
                GuestReadEvent {
                    physical_offset: 9,
                    len: 1,
                },
                GuestReadEvent {
                    physical_offset: 16,
                    len: 8,
                },
            ]
        );
    }

    #[test]
    fn translated_rdram_read_observer_reports_tlb_mapped_physical_offset() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 0x2000];
        let mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: (1 << 6) | 0b111,
            entry_lo1: 0b111,
        };

        assert!(mem.try_load_w_translated(&ctx, 0x0040_0020).is_ok());
        super::set_read_observer(previous);

        assert_eq!(
            OBSERVED_READS.with(|reads| reads.borrow().clone()),
            vec![GuestReadEvent {
                physical_offset: 0x1020,
                len: 4,
            }]
        );
    }

    #[test]
    fn translated_rdram_read_observer_unaligned_loads_cover_aligned_backing_ranges() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 32];
        let mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();
        let base = 0xffff_ffff_a000_0000;

        assert!(mem.try_load_wl_translated(&ctx, 0, base + 1).is_ok());
        assert!(mem.try_load_wr_translated(&ctx, 0, base + 2).is_ok());
        assert!(mem.try_load_dl_translated(&ctx, 0, base + 11).is_ok());
        assert!(mem.try_load_dr_translated(&ctx, 0, base + 14).is_ok());
        super::set_read_observer(previous);

        assert_eq!(
            OBSERVED_READS.with(|reads| reads.borrow().clone()),
            vec![
                GuestReadEvent {
                    physical_offset: 0,
                    len: 4,
                },
                GuestReadEvent {
                    physical_offset: 0,
                    len: 4,
                },
                GuestReadEvent {
                    physical_offset: 8,
                    len: 8,
                },
                GuestReadEvent {
                    physical_offset: 8,
                    len: 8,
                },
            ]
        );
    }

    #[test]
    fn translated_rdram_read_observer_ignores_failed_loads_and_host_snapshots() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 16];
        let mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();

        assert!(mem.try_load_w_translated(&ctx, 0x0040_0000).is_err());
        assert!(mem
            .try_load_w_translated(&ctx, 0xffff_ffff_8000_0040)
            .is_err());
        assert_eq!(mem.copy_physical_bytes(0, 4), vec![0; 4]);
        super::set_read_observer(previous);

        assert!(OBSERVED_READS.with(|reads| reads.borrow().is_empty()));
    }

    #[test]
    fn external_write_gateways_attribute_the_exact_fixed_denominator() {
        let gateways: [(WriterChannel, fn(u32, u32)); 8] = [
            (
                WriterChannel::CpuInstructionStore,
                super::notify_cpu_instruction_store,
            ),
            (WriterChannel::PiDma, super::notify_pi_dma_write),
            (WriterChannel::SiDma, super::notify_si_dma_write),
            (WriterChannel::SpDma, super::notify_sp_dma_write),
            (
                WriterChannel::RspExecutionOrHleWriteback,
                super::notify_rsp_execution_or_hle_writeback,
            ),
            (WriterChannel::RdpRenderer, super::notify_rdp_renderer_write),
            (WriterChannel::HostAbi, super::notify_host_abi_write),
            (
                WriterChannel::BootstrapOrImport,
                super::notify_bootstrap_or_import_write,
            ),
        ];
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous = super::set_write_observer(Some(observe_write));

        for (index, (_, gateway)) in gateways.iter().enumerate() {
            gateway(0x1000 + index as u32 * 4, 4);
        }
        // Preserve the existing zero-length notification behavior: it is not
        // a byte-producing event and therefore enters neither observer.
        super::notify_host_abi_write(0x2000, 0);

        let observed = OBSERVED_WRITES.with(|writes| writes.borrow().clone());
        assert_eq!(observed.len(), gateways.len());
        for (index, (event, (expected_channel, _))) in observed.iter().zip(gateways).enumerate() {
            assert_eq!(event.channel(), expected_channel);
            assert_eq!(event.range(), (0x1000 + index as u32 * 4, 4));
        }
        super::set_write_observer(previous);
    }

    #[test]
    fn write_events_canonicalize_cached_and_uncached_rdram_aliases() {
        assert_eq!(
            Rdram::physical_rdram_offset(0xffff_ffff_8000_1234),
            Some(0x1234)
        );
        assert_eq!(
            Rdram::physical_rdram_offset(0xffff_ffff_a000_1234),
            Some(0x1234)
        );
        assert_eq!(Rdram::physical_rdram_offset(0xffff_ffff_a440_0000), None);
        assert_eq!(
            Rdram::physical_rdram_offset(0x0000_0000_8000_1234),
            Some(0x1234)
        );
        assert_eq!(
            Rdram::physical_rdram_offset(0x0000_0000_a000_1234),
            Some(0x1234)
        );
        assert_eq!(Rdram::physical_rdram_offset(0x0000_0000_0000_1234), None);
        assert_eq!(Rdram::physical_rdram_offset(0xffff_ffff_c000_1234), None);
        assert_eq!(Rdram::physical_rdram_offset(0x0000_0001_8000_1234), None);
    }

    #[test]
    fn sparse_direct_windows_share_one_classifier_across_canonical_forms() {
        assert_eq!(
            Rdram::direct_storage_offset(0xffff_ffff_a600_0000),
            Some(0x2600_0000)
        );
        assert_eq!(
            Rdram::direct_storage_offset(0x0000_0000_a600_0000),
            Some(0x2600_0000)
        );
        assert_eq!(
            Rdram::direct_storage_offset(0xffff_ffff_8600_0000),
            Some(0x0600_0000)
        );
        assert_eq!(Rdram::direct_storage_offset(0xffff_ffff_a460_0000), None);
        assert_eq!(Rdram::direct_storage_offset(0x0000_0001_a600_0000), None);
        assert_eq!(Rdram::direct_storage_offset(0xffff_ffff_c600_0000), None);

        let mut bytes = vec![0u8; RDRAM_LEN + 4];
        bytes[RDRAM_LEN..RDRAM_LEN + 4].copy_from_slice(&0x1234_5678u32.to_ne_bytes());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xffff_ffff_8080_0000) as u32, 0x1234_5678);
        assert_eq!(mem.load_w(0x0000_0000_8080_0000) as u32, 0x1234_5678);
        assert_eq!(mem.try_load_w(0xffff_ffff_8080_0000), Ok(0x1234_5678));
    }

    #[test]
    fn kseg0_and_kseg1_loads_and_stores_share_visible_bytes() {
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let kseg0 = 0xffff_ffff_8000_0000;
        let kseg1 = 0xffff_ffff_a000_0000;

        mem.store_w(kseg1, 0x1122_3344);
        assert_eq!(mem.load_w(kseg0) as u32, 0x1122_3344);
        mem.store_h(kseg0 + 4, 0x8567);
        assert_eq!(mem.load_hu(kseg1 + 4), 0x8567);
        mem.store_b(kseg1 + 6, 0xa9);
        assert_eq!(mem.load_bu(kseg0 + 6), 0xa9);

        mem.store_w(0x0000_0000_8000_0008, 0xdead_beef);
        assert_eq!(mem.load_w(0x0000_0000_a000_0008) as u32, 0xdead_beef);
    }

    #[test]
    fn mapped_data_translation_selects_page_half_size_asid_and_access_bits() {
        let mut ctx = RecompContext::new();
        ctx.cop0_entry_hi = 0x0000_002a;
        ctx.tlb_entries[3] = TlbEntryRaw {
            page_mask: 0x0000_6000, // paired 16 KiB pages
            entry_hi: 0x0040_002a,
            entry_lo0: (0x20 << 6) | 0x6,
            entry_lo1: (0x30 << 6) | 0x2,
        };

        assert_eq!(
            ctx.translate_data_address(0x0040_1234, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x0002_1234))
        );
        assert_eq!(
            ctx.translate_data_address(0x0040_5234, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x0003_1234))
        );
        assert_eq!(
            ctx.translate_data_address(0x0040_5234, DataAccessKind::Store),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0x0040_5234,
                access: DataAccessKind::Store,
                kind: TlbFaultKind::Modified,
                extended: false,
            }))
        );

        ctx.cop0_entry_hi = 0x0000_002b;
        assert_eq!(
            ctx.translate_data_address(0x0040_1234, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0x0040_1234,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Refill,
                extended: false,
            }))
        );
    }

    #[test]
    fn libultra_invalid_tlb_layout_does_not_create_a_zero_address_multi_match() {
        let mut ctx = RecompContext::new();
        ctx.initialize_invalid_tlb_entries();
        assert_eq!(
            ctx.translate_data_address(4, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 4,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Invalid,
                extended: false,
            }))
        );
    }

    #[test]
    fn mapped_physical_address_above_direct_window_is_unbacked_not_aliased() {
        let mut ctx = RecompContext::new();
        ctx.cop0_entry_hi = 0x0040_002a;
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_002a,
            // Figure 3-10 PFN bit 17 becomes PA(29), the first physical byte
            // beyond the N64's 29-bit direct window.
            entry_lo0: (0x0002_0000 << 6) | 0x7,
            entry_lo1: 0x7,
        };
        assert_eq!(
            ctx.translate_data_address(0x0040_0000, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x2000_0000))
        );

        let mut bytes = 0x1000_0000u32.to_ne_bytes();
        let mem = Rdram::new(&mut bytes);
        assert_eq!(
            mem.try_load_w_translated(&ctx, 0x0040_0000),
            Err(DataAccessError::Unbacked { vaddr: 0x0040_0000 })
        );
        assert_eq!(bytes, 0x1000_0000u32.to_ne_bytes());
    }

    #[test]
    fn direct_segments_bypass_tlb_while_mapped_invalid_is_typed() {
        let mut ctx = RecompContext::new();
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0xc000_0000,
            entry_lo0: 1,
            entry_lo1: 1,
        };

        assert_eq!(
            ctx.translate_data_address(0xffff_ffff_8000_0040, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Direct(0xffff_ffff_8000_0040))
        );
        assert_eq!(
            ctx.translate_data_address(0xffff_ffff_a000_0040, DataAccessKind::Store),
            Ok(TranslatedDataAddress::Direct(0xffff_ffff_a000_0040))
        );
        assert_eq!(
            ctx.translate_data_address(0xffff_ffff_c000_0040, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0xffff_ffff_c000_0040,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Invalid,
                extended: false,
            }))
        );
    }

    #[test]
    fn extended_segments_enforce_region_privilege_and_xkphys_width() {
        const STATUS_KSU_USER: u32 = 0b10 << 3;
        const STATUS_KSU_SUPERVISOR: u32 = 0b01 << 3;
        const STATUS_UX: u32 = 1 << 5;
        const STATUS_SX: u32 = 1 << 6;
        const STATUS_KX: u32 = 1 << 7;
        const USER_VA: u64 = 0x0000_0012_3456_0040;
        const SUPERVISOR_VA: u64 = 0x4000_0012_3456_0040;

        let mut user = RecompContext::new();
        user.cop0_status = STATUS_KSU_USER | STATUS_UX;
        user.cop0_entry_hi = 0x2a;
        user.tlb_entries[4] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: (USER_VA & 0xc000_00ff_ffff_e000) | 0x2a,
            entry_lo0: 0x6,
            entry_lo1: 0x46,
        };
        assert_eq!(
            user.translate_data_address(USER_VA, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x40))
        );
        assert_eq!(
            user.translate_data_address(SUPERVISOR_VA, DataAccessKind::Load),
            Err(DataAccessError::AddressError {
                vaddr: SUPERVISOR_VA,
                access: DataAccessKind::Load,
            })
        );
        assert_eq!(
            user.translate_data_address(0x9000_0000_0000_0040, DataAccessKind::Store),
            Err(DataAccessError::AddressError {
                vaddr: 0x9000_0000_0000_0040,
                access: DataAccessKind::Store,
            })
        );

        let mut supervisor = RecompContext::new();
        supervisor.cop0_status = STATUS_KSU_SUPERVISOR | STATUS_SX;
        supervisor.cop0_entry_hi = 0x2a;
        supervisor.tlb_entries[4] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: (SUPERVISOR_VA & 0xc000_00ff_ffff_e000) | 0x2a,
            entry_lo0: 0x6,
            entry_lo1: 0x46,
        };
        assert_eq!(
            supervisor.translate_data_address(SUPERVISOR_VA, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x40))
        );
        assert!(matches!(
            supervisor.translate_data_address(0xc000_0012_3456_0040, DataAccessKind::Load),
            Err(DataAccessError::AddressError { .. })
        ));

        let mut kernel = RecompContext::new();
        kernel.cop0_status = STATUS_KX;
        assert_eq!(
            kernel.translate_data_address(0x9000_0000_0000_0040, DataAccessKind::Load),
            Ok(TranslatedDataAddress::DirectPhysical(0x40))
        );
        assert_eq!(
            kernel.translate_data_address(0x9000_0001_0000_0040, DataAccessKind::Load),
            Err(DataAccessError::AddressError {
                vaddr: 0x9000_0001_0000_0040,
                access: DataAccessKind::Load,
            })
        );
    }

    #[test]
    fn extended_tlb_faults_retain_full_address_and_refill_class() {
        const STATUS_KSU_USER: u32 = 0b10 << 3;
        const STATUS_UX: u32 = 1 << 5;
        const VA: u64 = 0x0000_0088_7654_2040;

        let mut ctx = RecompContext::new();
        ctx.cop0_status = STATUS_KSU_USER | STATUS_UX;
        ctx.cop0_entry_hi = 0x51;
        assert_eq!(
            ctx.translate_data_address(VA, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: VA,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Refill,
                extended: true,
            }))
        );

        ctx.tlb_entries[2] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: (VA & 0xc000_00ff_ffff_e000) | 0x51,
            entry_lo0: 0x6,
            entry_lo1: 0x46,
        };
        assert_eq!(
            ctx.translate_data_address(VA, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x40))
        );

        ctx.tlb_entries[2].entry_hi |= 0x4000_0000_0000_0000;
        assert!(matches!(
            ctx.translate_data_address(VA, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                kind: TlbFaultKind::Refill,
                extended: true,
                ..
            }))
        ));
    }

    #[test]
    fn erl_directs_only_the_low_user_segment_in_both_address_widths() {
        const STATUS_ERL: u32 = 1 << 2;
        const STATUS_KX: u32 = 1 << 7;

        for status in [STATUS_ERL, STATUS_ERL | STATUS_KX] {
            let mut ctx = RecompContext::new();
            ctx.cop0_status = status;
            assert_eq!(
                ctx.translate_data_address(0x1234_5040, DataAccessKind::Load),
                Ok(TranslatedDataAddress::DirectPhysical(0x1234_5040))
            );
        }

        let mut extended = RecompContext::new();
        extended.cop0_status = STATUS_ERL | STATUS_KX;
        assert_eq!(
            extended.translate_data_address(0x0000_0000_8000_0040, DataAccessKind::Load),
            Err(DataAccessError::AddressError {
                vaddr: 0x0000_0000_8000_0040,
                access: DataAccessKind::Load,
            })
        );
    }

    #[test]
    fn doubleword_cop0_moves_round_trip_entry_hi_and_xcontext() {
        let mut ctx = RecompContext::new();
        ctx.write_cop0_64(10, 0xc000_0088_7654_3051);
        ctx.write_cop0_64(20, 0x1234_5679_0abc_def0);
        assert_eq!(ctx.read_cop0_64(10), 0xc000_0088_7654_3051);
        assert_eq!(ctx.read_cop0_64(20), 0x1234_5679_0abc_def0);
        assert_eq!(ctx.read_cop0(10), 0x7654_3051);
        assert_eq!(ctx.read_cop0(20), 0x0abc_def0);
    }

    #[test]
    fn instruction_translation_returns_physical_identity_for_direct_and_mapped_aliases() {
        let mut ctx = RecompContext::new();
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: ((0x0010_0000 >> 6) & 0x03ff_ffc0) | 0b111,
            entry_lo1: ((0x0030_0000 >> 6) & 0x03ff_ffc0) | 0b111,
        };

        assert_eq!(
            ctx.translate_instruction_address(0x8000_0040),
            Ok(TranslatedInstructionAddress::new(0x40))
        );
        assert_eq!(
            ctx.translate_instruction_address(0xa000_0040),
            Ok(TranslatedInstructionAddress::new(0x40))
        );
        assert_eq!(
            ctx.translate_instruction_address(0x0040_0ffc),
            Ok(TranslatedInstructionAddress::new(0x0010_0ffc))
        );
        assert_eq!(
            ctx.translate_instruction_address(0x0040_1000),
            Ok(TranslatedInstructionAddress::new(0x0030_0000))
        );
    }

    #[test]
    fn diagnostic_instruction_translation_types_undefined_tlb_inputs() {
        let mut unsupported = RecompContext::new();
        unsupported.initialize_invalid_tlb_entries();
        unsupported.tlb_entries[4].page_mask = 0x0000_2000;
        assert_eq!(
            unsupported.translate_instruction_address_diagnostic_v1(0x0040_0000),
            Err(
                InstructionTranslationDiagnosticErrorV1::InvalidPageMaskEncoding {
                    index: 4,
                    page_mask_raw: 0x0000_2000,
                }
            )
        );

        let mut competing = RecompContext::new();
        competing.initialize_invalid_tlb_entries();
        let entry = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: ((0x0010_0000 >> 6) & 0x03ff_ffc0) | 0b111,
            entry_lo1: ((0x0030_0000 >> 6) & 0x03ff_ffc0) | 0b111,
        };
        competing.tlb_entries[1] = entry;
        competing.tlb_entries[2] = entry;
        assert_eq!(
            competing.translate_instruction_address_diagnostic_v1(0x0040_0040),
            Err(
                InstructionTranslationDiagnosticErrorV1::MultipleTlbMatches {
                    vaddr: 0x0040_0040,
                    first_index: 1,
                    second_index: 2,
                }
            )
        );
    }

    #[test]
    fn unsupported_instruction_width_stays_loud_while_privilege_is_typed() {
        let result = std::panic::catch_unwind(|| {
            RecompContext::new().translate_instruction_address(0x0000_0001_0000_0000)
        });
        let message = result
            .expect_err("64-bit instruction translation must remain loud")
            .downcast::<String>()
            .map(|message| *message)
            .unwrap_or_else(|payload| {
                payload
                    .downcast::<&'static str>()
                    .map(|message| (*message).to_owned())
                    .unwrap_or_default()
            });
        assert!(message.contains("64-bit instruction address translation is unsupported"));

        let mut user = RecompContext::new();
        user.cop0_status = 0b10 << 3;
        assert_eq!(
            user.translate_instruction_address(0x0040_0000),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0x0040_0000,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Refill,
                extended: false,
            }))
        );
        assert_eq!(
            user.translate_instruction_address(0xffff_ffff_8000_0000),
            Err(DataAccessError::AddressError {
                vaddr: 0xffff_ffff_8000_0000,
                access: DataAccessKind::Load,
            })
        );
    }

    #[test]
    fn mapped_low_physical_addresses_trap_instead_of_aliasing_rdram() {
        let mut bytes = [0u8; 4];
        let mem = Rdram::new(&mut bytes);
        for address in [
            0x0000_0000_0000_0000,
            0xffff_ffff_c000_0000,
            0x0000_0001_8000_0000,
        ] {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = mem.load_w(address);
            }));
            assert!(
                panic.is_err(),
                "mapped address {address:#018x} did not trap"
            );
        }
    }

    #[test]
    fn checked_accessors_return_typed_faults_for_non_rdram_segments() {
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let mmio = 0xffff_ffff_a460_0010;

        assert_eq!(mem.try_load_w(mmio), Err(mmio));
        assert_eq!(mem.try_load_h(mmio), Err(mmio));
        assert_eq!(mem.try_load_hu(mmio), Err(mmio));
        assert_eq!(mem.try_load_b(mmio), Err(mmio));
        assert_eq!(mem.try_load_bu(mmio), Err(mmio));
        assert_eq!(mem.try_load_wl(0, mmio + 1), Err(mmio + 1));
        assert_eq!(mem.try_load_wr(0, mmio + 2), Err(mmio + 2));
        assert_eq!(mem.try_load_d(mmio), Err(mmio));
        assert_eq!(mem.try_load_dl(0, mmio + 1), Err(mmio + 1));
        assert_eq!(mem.try_load_dr(0, mmio + 2), Err(mmio + 2));
        assert_eq!(mem.try_store_w(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_h(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_b(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_wl(mmio + 1, 0), Err(mmio + 1));
        assert_eq!(mem.try_store_wr(mmio + 2, 0), Err(mmio + 2));
        assert_eq!(mem.try_store_d(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_dl(mmio + 1, 0), Err(mmio + 1));
        assert_eq!(mem.try_store_dr(mmio + 2, 0), Err(mmio + 2));
        assert_eq!(mem.as_mut_slice(), [0; 16]);
    }

    #[test]
    fn checked_word_accessors_route_translated_mmio_before_backing_rejection() {
        const SI_STATUS: u64 = 0xffff_ffff_a480_0018;

        MMIO_CALLS.with(|calls| calls.set(0));
        let previous_mmio = super::set_mmio_hooks(Some(read_mmio), Some(consume_mmio));
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();

        assert_eq!(mem.try_load_w_translated(&ctx, SI_STATUS), Ok(0));
        assert_eq!(mem.try_store_w_translated(&ctx, SI_STATUS, 3), Ok(()));
        assert_eq!(MMIO_CALLS.with(std::cell::Cell::get), 2);
        assert_eq!(mem.as_mut_slice(), [0; 16]);

        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
    }

    #[test]
    fn subword_rcp_and_pif_accesses_use_big_endian_sysad_lanes() {
        MMIO_CALLS.with(|calls| calls.set(0));
        MMIO_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_mmio = super::set_mmio_hooks(Some(read_pattern_mmio), Some(consume_mmio));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);
        const RCP: u64 = 0xffff_ffff_a440_0000;
        const PIF: u64 = 0xffff_ffff_bfc0_07c0;

        assert_eq!(mem.load_b(RCP), -127);
        assert_eq!(mem.load_bu(RCP + 1), 0xa2);
        assert_eq!(mem.load_h(RCP), -32350);
        assert_eq!(mem.load_hu(RCP + 2), 0xc3e4);
        assert_eq!(mem.try_load_bu(PIF + 3), Ok(0xe4));
        assert_eq!(mem.try_load_hu(PIF + 2), Ok(0xc3e4));

        mem.store_b(RCP, 0x12);
        mem.store_b(RCP + 3, 0x34);
        mem.store_h(RCP, 0x5678);
        assert_eq!(mem.try_store_h(RCP + 2, 0x9abc), Ok(()));
        assert_eq!(mem.try_store_b(PIF + 1, 0xde), Ok(()));
        assert_eq!(
            MMIO_WRITES.with(|writes| writes.borrow().clone()),
            vec![
                (RCP, 0x1200_0000),
                (RCP, 0x0000_0034),
                (RCP, 0x5678_0000),
                (RCP, 0x0000_9abc),
                (PIF, 0x00de_0000),
            ]
        );
        assert_eq!(mem.as_mut_slice(), [0; 4]);
        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
    }

    #[test]
    fn unsupported_wide_and_partial_word_mmio_accesses_trap_before_any_side_effect() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        MMIO_CALLS.with(|calls| calls.set(0));
        MMIO_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_observer = super::set_write_observer(Some(observe_write));
        let previous_mmio = super::set_mmio_hooks(Some(read_mmio), Some(consume_mmio));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);

        let operations: [RdramOperation; 4] = [
            |mem| {
                let _ = mem.load_d(0xffff_ffff_a400_0000);
            },
            |mem| mem.store_d(0xffff_ffff_bfc0_07c0, 1),
            |mem| mem.store_wl(0xffff_ffff_a440_0001, 1),
            |mem| mem.store_wr(0xffff_ffff_a440_0002, 1),
        ];
        for operation in operations {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mut mem);
            }));
            assert!(panic.is_err(), "non-word MMIO access did not trap");
        }

        assert_eq!(MMIO_CALLS.with(std::cell::Cell::get), 0);
        assert!(MMIO_WRITES.with(|writes| writes.borrow().is_empty()));
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        assert_eq!(mem.as_mut_slice(), [0; 4]);

        mem.store_wl(0xffff_ffff_a440_0000, 0x1122_3344);
        mem.store_wr(0xffff_ffff_a440_0003, 0x5566_7788);
        assert_eq!(
            MMIO_CALLS.with(std::cell::Cell::get),
            2,
            "full-selector SWL/SWR must issue one write each with no MMIO pre-read"
        );
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn misaligned_aligned_accessors_trap_before_bytes_or_events_change() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_observer = super::set_write_observer(Some(observe_write));
        let mut bytes = [0x5au8; 16];
        let before = bytes;
        let mut mem = Rdram::new(&mut bytes);
        let operations: [RdramOperation; 6] = [
            |mem| {
                let _ = mem.load_h(0xffff_ffff_8000_0001);
            },
            |mem| mem.store_h(0xffff_ffff_a000_0001, 1),
            |mem| {
                let _ = mem.load_w(0xffff_ffff_8000_0002);
            },
            |mem| mem.store_w(0xffff_ffff_a000_0002, 1),
            |mem| {
                let _ = mem.load_d(0xffff_ffff_8000_0004);
            },
            |mem| mem.store_d(0xffff_ffff_a000_0004, 1),
        ];
        for operation in operations {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mut mem);
            }));
            assert!(panic.is_err(), "misaligned access did not trap");
        }
        assert_eq!(bytes, before);
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn consumed_mmio_store_does_not_report_an_rdram_write() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_observer = super::set_write_observer(Some(observe_write));
        let previous_mmio = super::set_mmio_hooks(None, Some(consume_mmio));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_A460_0000, 0x1234_5678);

        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        assert_eq!(bytes, [0; 4]);
        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn guest_write_tokens_change_only_for_intersecting_pages_and_new_sessions() {
        let previous = super::set_guest_write_boundary_observer(None);
        let first = super::guest_write_token(0x2000, 0x1000);
        super::notify_host_abi_write(0x5000, 4);
        assert_eq!(super::guest_write_token(0x2000, 0x1000), first);
        super::notify_host_abi_write(0x2fff, 2);
        let written = super::guest_write_token(0x2000, 0x1000);
        assert_ne!(written, first);

        super::set_guest_write_boundary_observer(None);
        assert_ne!(super::guest_write_token(0x2000, 0x1000), written);
        super::set_guest_write_boundary_observer(previous);
    }
