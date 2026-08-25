import importlib.util
import pathlib
import sys
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("summarize_wm2000_pump_census.py")
SPEC = importlib.util.spec_from_file_location("summarize_wm2000_pump_census", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def row(
    index: int,
    wall_ms: float,
    swapped: bool,
    *,
    gfx_tasks: int = 0,
    gfx_lle_rdp_ms: float = 0.0,
) -> str:
    return (
        f"[pump-seq] {index},{wall_ms:.4f},1,{int(swapped)},{gfx_tasks},0,0,0,0,"
        f"{gfx_lle_rdp_ms:.4f},0,0,0,0,0"
    )


def armed_row(
    index: int,
    wall_ms: float,
    swapped: bool,
    completion_before: int,
    completion_after: int,
    phases: tuple[float, ...],
    abi_phases: tuple[float, ...] = (),
    abi_tasks: int = 0,
) -> str:
    assert len(phases) == 11
    assert len(abi_phases) in (0, 11)
    return ",".join(
        (
            row(index, wall_ms, swapped),
            str(completion_before),
            str(completion_after),
            *(f"{value:.4f}" for value in phases),
            *(f"{value:.4f}" for value in abi_phases),
            *((str(abi_tasks),) if abi_phases else ()),
        )
    )


def cadence_row(
    index: int,
    *,
    interval_ms: float,
    start_debt_ms: float = 0.0,
    wake_overshoot_ms: float = 0.0,
    reanchored: bool = False,
    pump_ms: float = 10.0,
    present_ms: float = 1.0,
    wait_ms: float = 5.0,
    outside_ms: float = 0.0,
) -> str:
    return (
        f"[wall-cadence-seq] {index},{index * 16.0:.4f},{index * 16.0:.4f},"
        f"{interval_ms:.4f},{start_debt_ms:.4f},{wake_overshoot_ms:.4f},"
        f"{int(reanchored)},{pump_ms:.4f},"
        f"{present_ms:.4f},{wait_ms:.4f},{outside_ms:.4f}"
    )


def dependency_row(
    pump: int,
    mode: str = "Observe",
    *,
    sha256: str = "01" * 32,
    exact_hit: bool = False,
    suppress: bool = False,
) -> str:
    return (
        f"[present-dependency-seq] pump={pump} mode={mode} dependency=Cacheable "
        f"overscan=3 zoom_fill=1 generation=7 invalidations=2 probe_ns=125000 "
        f"start=16 src_stride=320 dst_width=319 dst_height=240 blanked=0 "
        f"bytes=153600 fnv_digest=0123456789abcdef sha256={sha256} exact_hit={int(exact_hit)} "
        f"disposition={'Suppress' if suppress else 'Redraw'}"
    )


class PumpCensusSummaryTests(unittest.TestCase):
    def test_present_dependencies_cover_final_pump_and_keep_disposition_separate(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 1.0, True),
                row(1, 2.0, False),
                row(2, 3.0, True),
                dependency_row(0),
                dependency_row(1, exact_hit=True),
                dependency_row(2, exact_hit=True),
            ]
        )
        result = MODULE.summarize(text)["present_dependencies"]
        self.assertEqual(result["receipts"], 3)
        self.assertEqual(result["exact_hits"], 2)
        self.assertEqual(result["suppressed"], 0)
        self.assertEqual(len(result["canonical_identity_sha256"]), 64)

    def test_present_dependency_parser_rejects_gaps_modes_and_policy_lies(self) -> None:
        with self.assertRaisesRegex(ValueError, "expected pump 1"):
            MODULE.parse_present_dependencies(
                "\n".join((dependency_row(0), dependency_row(2)))
            )
        with self.assertRaisesRegex(ValueError, "mixes Observe and Suppress"):
            MODULE.parse_present_dependencies(
                "\n".join((dependency_row(0), dependency_row(1, "Suppress")))
            )
        with self.assertRaisesRegex(ValueError, "Observe.*cannot suppress"):
            MODULE.parse_present_dependencies(
                dependency_row(0, exact_hit=True, suppress=True)
            )

    def test_uncacheable_dependency_reason_is_canonical_identity(self) -> None:
        text = (
            "[present-dependency-seq] pump=0 mode=Observe dependency=Uncacheable "
            "overscan=3 zoom_fill=1 generation=7 invalidations=2 probe_ns=125000 "
            "reason=Overlay exact_hit=0 disposition=Redraw"
        )
        sample = MODULE.parse_present_dependencies(text, 1)[0]
        self.assertEqual(sample.canonical_identity(), (3, True, "Uncacheable", "Overlay"))

    def test_legacy_rows_remain_supported_without_phase_claims(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 1.0, True),
                row(1, 2.0, False),
                row(2, 3.0, True),
            ]
        )
        result = MODULE.summarize(text)
        self.assertEqual(len(MODULE.parse_pumps(text)), 3)
        self.assertEqual(result["task_cpu_phase_frames"], {"available": False})

    def test_expanded_rows_fold_completion_range_and_phase_totals(self) -> None:
        zero = (0.0,) * 11
        first = (10.0, 2.0, 5.0, 3.0, 8.0, 1.5, 0.4, 0.1, 0.0, 2.0, 1.0)
        second = (20.0, 4.0, 11.0, 6.0, 17.0, 3.0, 0.8, 0.2, 0.0, 3.0, 2.0)
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                armed_row(0, 1.0, True, 7, 7, zero),
                armed_row(1, 2.0, False, 7, 8, first),
                armed_row(2, 3.0, True, 8, 9, second),
            ]
        )
        result = MODULE.summarize(text)
        phases = result["task_cpu_phase_frames"]
        self.assertTrue(phases["available"])
        self.assertEqual(phases["metrics"]["task_completions"]["mean"], 2.0)
        self.assertEqual(phases["metrics"]["task_envelope_ms"]["mean"], 30.0)
        self.assertEqual(phases["metrics"]["task_renderer_work_ms"]["mean"], 25.0)
        self.assertEqual(phases["metrics"]["task_outer_residual_ms"]["mean"], 5.0)
        self.assertEqual(
            phases["metrics"]["task_rdp_outside_envelope_ms"]["mean"], 3.0
        )
        self.assertEqual(result["abi_task_phase_frames"], {"available": False})

    def test_existing_abi_clocks_fold_and_close_without_new_timing_fields(self) -> None:
        zero = (0.0,) * 11
        task = (18.0, 2.0, 6.0, 4.0, 10.0, 1.0, 0.5, 0.1, 0.0, 8.0, 7.0)
        # plan, finalize, execute, commit, total, setup, plan-bind, guest-reads,
        # staged-writes, copyback, publication
        abi = (1.0, 0.5, 13.0, 0.7, 25.0, 1.2, 0.3, 0.8, 0.9, 0.4, 0.6)
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                armed_row(0, 1.0, True, 7, 7, zero, zero, 0),
                armed_row(1, 2.0, False, 7, 8, task, abi, 1),
                armed_row(2, 3.0, True, 8, 8, zero, zero, 0),
            ]
        )
        metrics = MODULE.summarize(text)["abi_task_phase_frames"]["metrics"]
        self.assertEqual(metrics["execute_outer_ms"]["mean"], 3.0)
        self.assertEqual(metrics["post_execute_outer_ms"]["mean"], 5.0)
        self.assertEqual(metrics["post_execute_accounted_ms"]["mean"], 2.6)
        self.assertAlmostEqual(
            metrics["post_execute_unattributed_ms"]["mean"], 2.4
        )
        self.assertEqual(metrics["pre_execute_accounted_ms"]["mean"], 3.8)
        self.assertEqual(metrics["outside_unattributed_ms"]["mean"], 3.2)
        abi_summary = MODULE.summarize(text)["abi_task_phase_frames"]
        self.assertEqual(abi_summary["identity_closed_frames"], 1.0)
        self.assertEqual(abi_summary["identity_mismatch_frames"], 0)

    def test_drawn_frames_are_measured_between_consecutive_swaps(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 4.0, False),
                row(1, 20.0, True),
                row(2, 5.0, False),
                row(3, 21.0, True),
                row(4, 6.0, False),
                row(5, 22.0, True),
            ]
        )
        result = MODULE.summarize(text)
        self.assertEqual(result["swap_gap_histogram"], {"2": 2})
        self.assertEqual(result["gap_two_fraction"], 1.0)
        self.assertEqual(result["drawn_frame_ms"]["mean"], 27.0)
        self.assertEqual(result["drawn_frame_ms"]["p95"], 28.0)
        self.assertEqual(result["over_budget"]["count"], 0)

    def test_a_non_two_pump_cadence_is_visible_in_the_receipt(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 1.0, True),
                row(1, 2.0, False),
                row(2, 3.0, False),
                row(3, 4.0, True),
            ]
        )
        result = MODULE.summarize(text)
        self.assertEqual(result["swap_gap_histogram"], {"3": 1})
        self.assertEqual(result["gap_two_fraction"], 0.0)
        self.assertEqual(result["drawn_frame_ms"]["mean"], 9.0)

    def test_wall_cadence_rows_join_exact_indices_and_preserve_stalls(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 1.0, True),
                row(1, 2.0, False),
                row(2, 3.0, True),
                cadence_row(index=0, interval_ms=16.0, outside_ms=0.0),
                cadence_row(
                    index=1,
                    interval_ms=365.0,
                    start_debt_ms=348.0,
                    wake_overshoot_ms=338.0,
                    reanchored=True,
                    outside_ms=338.0,
                ),
                "[wall-swap-seq] 2,381.0000",
            ]
        )
        cadence = MODULE.summarize(text)["wall_cadence"]
        self.assertTrue(cadence["available"])
        self.assertEqual(cadence["completed_intervals"], 2)
        self.assertEqual(cadence["reanchors"], 1)
        self.assertEqual(cadence["swap_to_swap_ms"]["max"], 381.0)
        self.assertEqual(cadence["totals_ms"]["outside_residual_ms"], 338.0)
        self.assertEqual(
            cadence["distributions_ms"]["wake_overshoot_ms"]["max"], 338.0
        )

    def test_wall_cadence_index_without_a_pump_is_rejected(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 1.0, True),
                row(1, 2.0, False),
                row(2, 3.0, True),
                cadence_row(index=3, interval_ms=16.0),
            ]
        )
        with self.assertRaisesRegex(ValueError, "has no matching pump row"):
            MODULE.summarize(text)

    def test_wall_cadence_rejects_negative_indices_and_non_boolean_reanchor(self) -> None:
        base = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 1.0, True),
                row(1, 2.0, False),
                row(2, 3.0, True),
            ]
        )
        with self.assertRaisesRegex(ValueError, "must be non-negative"):
            MODULE.summarize(base + "\n" + cadence_row(index=-1, interval_ms=16.0))
        invalid_reanchor = cadence_row(index=0, interval_ms=16.0).split(",")
        invalid_reanchor[6] = "2"
        with self.assertRaisesRegex(ValueError, "must be 0 or 1"):
            MODULE.summarize(base + "\n" + ",".join(invalid_reanchor))
        with self.assertRaisesRegex(ValueError, "must be non-negative"):
            MODULE.summarize(base + "\n[wall-swap-seq] -1,33.0000")

    def test_missing_sequence_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "no \\[pump-seq\\] rows"):
            MODULE.summarize("[pump-census] RENDERER: wgpu\n")

    def test_drawn_frame_populations_attribute_the_over_budget_tail(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                row(0, 1.0, True),
                row(1, 4.0, False),
                row(2, 20.0, True, gfx_tasks=2, gfx_lle_rdp_ms=18.0),
                row(3, 5.0, False),
                row(4, 40.0, True, gfx_tasks=3, gfx_lle_rdp_ms=38.0),
            ]
        )
        populations = MODULE.summarize(text)["drawn_frame_populations"]
        self.assertEqual(populations["within_budget_mean"]["gfx_tasks"], 2.0)
        self.assertEqual(populations["over_budget_mean"]["gfx_tasks"], 3.0)
        self.assertEqual(populations["over_minus_within"]["gfx_lle_rdp_ms"], 20.0)


if __name__ == "__main__":
    unittest.main()
