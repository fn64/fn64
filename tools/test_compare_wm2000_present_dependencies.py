import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import compare_wm2000_present_dependencies as comparator


def row(
    pump: int,
    mode: str,
    *,
    sha256: str = "01" * 32,
    generation: int = 7,
    overscan: int = 3,
    hit: bool = False,
) -> str:
    disposition = "Suppress" if mode == "Suppress" and hit else "Redraw"
    return (
        f"[present-dependency-seq] pump={pump} mode={mode} dependency=Cacheable "
        f"overscan={overscan} zoom_fill=1 generation={generation} invalidations=2 probe_ns=125000 "
        f"start=16 src_stride=320 dst_width=319 dst_height=240 blanked=0 "
        f"bytes=153600 fnv_digest=0123456789abcdef sha256={sha256} "
        f"exact_hit={int(hit)} disposition={disposition}"
    )


def log(mode: str, count: int, *, mutant: int | None = None) -> str:
    return "\n".join(
        row(
            pump,
            mode,
            sha256="fe" * 32 if pump == mutant else "01" * 32,
            hit=pump > 0,
        )
        for pump in range(count)
    )


class PresentDependencyComparatorTests(unittest.TestCase):
    def test_equal_identity_allows_observe_suppress_disposition_difference(self) -> None:
        result = comparator.compare_logs(log("Observe", 3), log("Suppress", 3), 3)
        self.assertEqual(result["pumps"], 3)
        self.assertEqual(result["observe"]["suppressed"], 0)
        self.assertEqual(result["suppress"]["suppressed"], 2)

    def test_one_identity_mutation_fails_at_its_pump(self) -> None:
        with self.assertRaisesRegex(ValueError, "differs at pump 2"):
            comparator.compare_logs(
                log("Observe", 3), log("Suppress", 3, mutant=2), 3
            )

    def test_default_contract_is_exactly_1600_contiguous_rows(self) -> None:
        with self.assertRaisesRegex(ValueError, "requires 1600 contiguous rows"):
            comparator.compare_logs(log("Observe", 1599), log("Suppress", 1599))

    def test_policy_is_canonical_but_generation_is_diagnostic(self) -> None:
        observe = "\n".join(row(i, "Observe") for i in range(3))
        changed_generation = "\n".join(
            row(i, "Suppress", generation=99, hit=i > 0) for i in range(3)
        )
        comparator.compare_logs(observe, changed_generation, 3)
        changed_policy = "\n".join(
            row(i, "Suppress", overscan=4, hit=i > 0) for i in range(3)
        )
        with self.assertRaisesRegex(ValueError, "differs at pump 0"):
            comparator.compare_logs(observe, changed_policy, 3)


if __name__ == "__main__":
    unittest.main()
