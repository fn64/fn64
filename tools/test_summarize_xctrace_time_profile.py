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
      <frame name="execute_raw_dpc"><binary ref="main" /></frame>
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


CPU_XML = """\
<trace-query-result>
  <row>
    <process id="p" fmt="fn64 (10)" />
    <cycle-weight id="c">41</cycle-weight>
    <tagged-backtrace><backtrace>
      <frame id="leaf" name="raster_triangle_scalar" addr="0x100001234">
        <binary id="main" name="fn64"
          UUID="01234567-89AB-CDEF-0123-456789ABCDEF"
          arch="arm64" load-addr="0x100000000" path="/private/build/fn64" />
      </frame>
      <frame name="execute_raw_triangle"><binary ref="main" /></frame>
    </backtrace></tagged-backtrace>
  </row>
  <row>
    <process ref="p" />
    <cycle-weight ref="c" />
    <tagged-backtrace><backtrace>
      <frame ref="leaf" />
      <frame name="execute_raw_triangle"><binary ref="main" /></frame>
    </backtrace></tagged-backtrace>
  </row>
</trace-query-result>
"""


ANCESTOR_XML = """\
<trace-query-result>
  <row>
    <process id="p" fmt="fn64 (10)" />
    <cycle-weight id="c">23</cycle-weight>
    <tagged-backtrace><backtrace>
      <frame id="sample" name="BoundPreparedTextureSampler::sample" addr="0x100000120">
        <binary id="main" name="fn64" load-addr="0x100000000" />
      </frame>
      <frame id="scalar" name="raw_triangle::raster_triangle_scalar::h1">
        <binary ref="main" />
      </frame>
      <frame ref="scalar" />
    </backtrace></tagged-backtrace>
  </row>
  <row>
    <process ref="p" />
    <cycle-weight ref="c" />
    <tagged-backtrace><backtrace>
      <frame id="blend" name="blend_fragment" addr="0x100000240">
        <binary ref="main" />
      </frame>
      <frame name="raw_triangle::raster_triangle_scalar::h2">
        <binary ref="main" />
      </frame>
    </backtrace></tagged-backtrace>
  </row>
  <row>
    <process ref="p" />
    <cycle-weight ref="c" />
    <tagged-backtrace><backtrace>
      <frame ref="blend" />
      <frame name="raw_triangle::raster_triangle_scalar::helper-copy">
        <binary name="plugin" />
      </frame>
    </backtrace></tagged-backtrace>
  </row>
</trace-query-result>
"""


class TimeProfileSummaryTests(unittest.TestCase):
    def test_exclusive_cost_and_main_image_callers_resolve_references(self) -> None:
        result = MODULE.summarize(
            XML, process="fn64", leaf_patterns=("memmove",), limit=10
        )
        self.assertEqual(result["samples"], 2)
        self.assertEqual(result["weight_unit"], "nanoseconds")
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
        paths = result["leaf_callers"]["memmove"]["call_paths"]
        self.assertEqual(
            {entry["symbol"] for entry in paths},
            {
                "_platform_memmove <- stage_color_commands <- execute_raw_dpc",
                "_platform_memmove <- execute_scheduled_raw_triangle",
            },
        )

    def test_cpu_cycles_rank_leaf_pcs_without_paths(self) -> None:
        result = MODULE.summarize(
            CPU_XML,
            process="fn64",
            leaf_patterns=("raster_triangle_scalar",),
            limit=10,
        )
        self.assertEqual(result["schema"], "fn64.xctrace-cpu-profile.v1")
        self.assertEqual(result["weight_unit"], "cycles")
        self.assertEqual(result["cycles"], 82.0)
        addresses = result["leaf_callers"]["raster_triangle_scalar"]["addresses"]
        self.assertEqual(
            addresses,
            [
                {
                    "address": "0x100001234",
                    "image": "fn64",
                    "image_arch": "arm64",
                    "image_load_address": "0x100000000",
                    "image_offset": "0x1234",
                    "image_uuid": "01234567-89AB-CDEF-0123-456789ABCDEF",
                    "cycles": 82.0,
                }
            ],
        )
        self.assertNotIn("/private/build", str(result))

    def test_mixed_weight_units_are_rejected(self) -> None:
        mixed = CPU_XML.replace(
            '<cycle-weight ref="c" />', '<weight>1000000</weight>'
        )
        with self.assertRaisesRegex(ValueError, "mixes nanosecond and cycle"):
            MODULE.summarize(mixed, process="fn64")

    def test_ancestor_population_is_main_image_only_and_deduplicated_per_row(
        self,
    ) -> None:
        result = MODULE.summarize(
            ANCESTOR_XML,
            process="fn64",
            ancestor_patterns=("raster_triangle_scalar",),
            limit=10,
        )
        population = result["ancestor_populations"]["raster_triangle_scalar"]
        self.assertEqual(population["cycles"], 46.0)
        self.assertEqual(population["fraction_of_profile"], 2 / 3)
        self.assertEqual(
            population["exclusive"],
            [
                {"symbol": "BoundPreparedTextureSampler::sample", "cycles": 23.0},
                {"symbol": "blend_fragment", "cycles": 23.0},
            ],
        )
        self.assertEqual(
            [entry["address"] for entry in population["addresses"]],
            ["0x100000120", "0x100000240"],
        )

    def test_same_absolute_pc_from_distinct_images_is_not_merged(self) -> None:
        distinct = CPU_XML.replace(
            '<cycle-weight ref="c" />',
            '<cycle-weight ref="c" />',
        ).replace(
            '<frame ref="leaf" />',
            '<frame name="raster_triangle_scalar" addr="0x100001234">'
            '<binary name="fn64" UUID="FEDCBA98-7654-3210-FEDC-BA9876543210" '
            'arch="arm64" load-addr="0x100000000" /></frame>',
        )
        result = MODULE.summarize(
            distinct,
            process="fn64",
            leaf_patterns=("raster_triangle_scalar",),
            limit=10,
        )
        addresses = result["leaf_callers"]["raster_triangle_scalar"]["addresses"]
        self.assertEqual(len(addresses), 2)
        self.assertEqual({entry["cycles"] for entry in addresses}, {41.0})
        self.assertEqual(len({entry["image_uuid"] for entry in addresses}), 2)

    def test_leaf_before_image_load_address_is_rejected(self) -> None:
        invalid = CPU_XML.replace('addr="0x100001234"', 'addr="0x0fffffff"')
        with self.assertRaisesRegex(ValueError, "precedes its image load address"):
            MODULE.summarize(
                invalid,
                process="fn64",
                leaf_patterns=("raster_triangle_scalar",),
            )

    def test_missing_reference_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing id"):
            MODULE.summarize(
                "<trace-query-result><row><weight ref='absent'/></row></trace-query-result>"
            )


if __name__ == "__main__":
    unittest.main()
