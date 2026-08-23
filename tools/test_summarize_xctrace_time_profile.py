import importlib.util
import pathlib
import sys
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("summarize_xctrace_time_profile.py")
SPEC = importlib.util.spec_from_file_location("summarize_xctrace_time_profile", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


XML = """\
<trace-query-result>
  <row>
    <process id="p" fmt="fn64 (10)" />
    <weight id="w">1000000</weight>
    <tagged-backtrace><backtrace>
      <frame id="memmove" name="_platform_memmove"><binary name="libsystem" /></frame>
      <frame name="stage_color_commands"><binary id="main" name="fn64" /></frame>
    </backtrace></tagged-backtrace>
  </row>
  <row>
    <process ref="p" />
    <weight ref="w" />
    <tagged-backtrace><backtrace>
      <frame ref="memmove" />
      <frame name="execute_scheduled_raw_triangle"><binary ref="main" /></frame>
    </backtrace></tagged-backtrace>
  </row>
  <row>
    <process fmt="helper (11)" />
    <weight ref="w" />
    <tagged-backtrace><backtrace><frame ref="memmove" /></backtrace></tagged-backtrace>
  </row>
</trace-query-result>
"""


class TimeProfileSummaryTests(unittest.TestCase):
    def test_exclusive_cost_and_main_image_callers_resolve_references(self) -> None:
        result = MODULE.summarize(
            XML, process="fn64", leaf_patterns=("memmove",), limit=10
        )
        self.assertEqual(result["samples"], 2)
        self.assertEqual(result["weight_ms"], 2.0)
        self.assertEqual(
            result["exclusive"],
            [{"symbol": "_platform_memmove", "weight_ms": 2.0}],
        )
        callers = result["leaf_callers"]["memmove"]["callers"]
        self.assertEqual({entry["symbol"] for entry in callers}, {
            "stage_color_commands",
            "execute_scheduled_raw_triangle",
        })
        self.assertTrue(all(entry["weight_ms"] == 1.0 for entry in callers))

    def test_missing_reference_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing id"):
            MODULE.summarize(
                "<trace-query-result><row><weight ref='absent'/></row></trace-query-result>"
            )


if __name__ == "__main__":
    unittest.main()
