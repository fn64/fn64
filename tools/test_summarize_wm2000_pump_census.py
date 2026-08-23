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


class PumpCensusSummaryTests(unittest.TestCase):
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
