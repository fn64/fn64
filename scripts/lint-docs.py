#!/usr/bin/env python3
"""lint-docs -- catch the doc-drift class AGENTS.md's "mechanism over patch" wants.

Docs here are load-bearing: agents follow them to spec. So a doc that points at
a file that doesn't exist, or names a crate that isn't real, is a live bug that
sends the next session down a wrong path. On 2026-07-17 all five checks below
were failing at once -- AGENTS.md read-order item 3 pointed at a
`docs/ABI-SURFACE.md` that never existed, DECOUPLING told agents to build a
crate the project deliberately doesn't have, COMPLETENESS asserted an ABI
family was ABSENT while the code implemented it, and README described 5 of 11
crates. Each was found by a human asking the right question. That doesn't scale;
this does.

Usage: scripts/lint-docs.py [--verbose]   (exit 0 clean, 1 on any error)

ponytail: five greps and a file-exists check, no schema, no deps. If this ever
needs a real graph model, the answer is `keel` (github.com/jer/keel), not a
bigger script.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERBOSE = "--verbose" in sys.argv
errors: list[str] = []
warnings: list[str] = []
checked = 0


def check_generated_validators() -> None:
    validator = ROOT / "tools/check_unsupported_event_sites.py"
    result = subprocess.run(
        [sys.executable, str(validator)], cwd=ROOT, capture_output=True, text=True
    )
    if result.returncode != 0:
        fail("docs/unsupported-event-sites.json", result.stderr.strip() or result.stdout.strip())


def fail(where: str, msg: str) -> None:
    errors.append(f"{where}: {msg}")


def warn(where: str, msg: str) -> None:
    """A finding that is about FORM, not a broken claim -- reported, not gating.

    check_stale_deferrals separates "this deferral is factually wrong" (fail)
    from "this deferral is shaped so it will rot silently" (warn). Conflating
    them produces noise on prose that is currently correct, which is how a
    linter teaches people to ignore it."""
    warnings.append(f"{where}: {msg}")


def ignored(ref: str) -> bool:
    """True if .gitignore covers this path -- ask git, don't reimplement it."""
    return subprocess.run(
        ["git", "check-ignore", "-q", ref], cwd=ROOT, capture_output=True
    ).returncode == 0


def docs() -> list[Path]:
    """Every tracked markdown file (untracked scratch and vendored trees are noise)."""
    out = subprocess.run(
        ["git", "ls-files", "*.md"], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return [ROOT / p for p in out.stdout.split()]


def superseded(text: str) -> bool:
    """True if a doc marks ITSELF not-yet-live or no-longer-live up top.

    A superseded design record documents the paths and env vars it PROPOSED
    or REJECTED, which by definition need not exist in code -- the drift
    rules ("a named file/var that isn't real is a silent no-op instruction")
    are about live, actionable docs, not history. The opt-out is a
    `> **SUPERSEDED` banner in the first few lines -- the doc's OWN status,
    not a doc that merely mentions the word elsewhere.

    A STAGED RUNBOOK is the same case pointing forward instead of back. The
    VPW2 bring-up runbook names `examples/vpw2-block-boot` and
    `FN64_DISCOVER_NA2J_ROM` because step 4 is where you create them; it says
    "**Nothing here has been executed.**" in its header. Those names are the
    instruction, not drift, and the drift rule would push toward deleting the
    very steps the runbook exists to record. Widened to the first 10 lines so
    a title plus a paragraph of framing still reaches the banner."""
    head = text.splitlines()[:10]
    return any("**SUPERSEDED" in line or "**Nothing here has been executed" in line
               for line in head)


# --- 1. every repo-relative path a doc names must exist -----------------------
# Both shapes occur: `docs/X.md` in backticks (the common one) and bare
# docs/X.md. The leading class must therefore ALLOW a backtick and only reject
# a preceding path char -- an earlier version excluded backticks and so matched
# 4 refs instead of 24, passing green while blind. Regression-tested below.
REF = re.compile(r"(?:^|[^\w/.-])((?:docs|crates|scripts|examples)/[\w./-]+)")
# Trailing punctuation bleeds into the match; strip it before testing.
TRAIL = ".,;:)]}'\""


def check_refs() -> None:
    global checked
    for doc in docs():
        text = doc.read_text()
        if superseded(text):
            continue
        for lineno, line in enumerate(text.splitlines(), 1):
            # The generated workflow dashboard deliberately reserves exact
            # destination paths before a writer creates them. Its source
            # manifest separately enforces bounded repo-relative paths and the
            # dashboard checker rejects malformed state. Treat only that
            # machine-owned table row as a reservation, not a live file claim.
            if doc.name == "RT64-PORT-DASHBOARD.md" and line.startswith("| writable paths |"):
                continue
            # A path inside ANOTHER repository is that repo's, not ours.
            # n64recomp-comparison.md:493 lists ~/Code/aki-recomp's own
            # README/AGENTS/docs -- naming them is the comparison's whole
            # point, and they can never exist here (aki-recomp is GPL-3.0 and
            # deliberately absent).
            # RT64-SHADER-ARTIFACTS.md cites files inside the separately pinned
            # DirectXShaderCompiler source tree.  Its exact source audit owns
            # those paths; treating `docs/...` there as fn64-local would make
            # the ordinary documentation gate reject an honest upstream
            # citation after the generated report becomes tracked.
            if "aki-recomp" in line or "~/Code/" in line or "DirectXShaderCompiler" in line:
                continue
            for raw in REF.findall(line):
                ref = raw.rstrip(TRAIL)
                # Illustrative globs/wildcards aren't claims about one file.
                # REF's character class stops at `*`, so `shard*/` arrives here
                # already truncated to `shard` -- check the ORIGINAL line for a
                # wildcard immediately after the match, or the glob loses its
                # own exemption and gets reported as a missing file.
                after = line[line.index(raw) + len(raw):]
                if "*" in ref or ref.endswith("/") or after[:1] == "*":
                    continue
                # Build artifacts (target/, recompiled/) exist only after a
                # build and are gitignored; a doc naming one is telling you to
                # delete or inspect it, not claiming it's checked in.
                if ignored(ref):
                    continue
                checked += 1
                if not (ROOT / ref).exists():
                    rel = doc.relative_to(ROOT)
                    fail(f"{rel}:{lineno}", f"references {ref} which does not exist")


# --- 2. every workspace member appears in README's crate table ---------------
def check_readme_crates() -> None:
    manifest = (ROOT / "Cargo.toml").read_text()
    members = re.findall(r"fn64-[\w-]+", manifest.split("members = [")[1].split("]")[0])
    readme = (ROOT / "README.md").read_text()
    for crate in members:
        if crate not in readme:
            fail("README.md", f"workspace member {crate} is not mentioned")


# --- 3. every documented env var exists in code ------------------------------
# A doc naming a var the code never reads is an instruction that silently no-ops.
# Env vars owned by the extracted game harnesses (recomps/wm2000), verified
# present there at extraction time. See check_env_vars for why this list
# exists rather than a live scan.
GAME_HARNESS_ENV = {
    "FN64_AUDIO_VALIDATION_SKIP_GRAPHICS",
    "FN64_BLOCK_CONTINUE_AFTER_OVERLAY",
    "FN64_BLOCK_DEVICE_TRACE",
    "FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS",
    "FN64_BLOCK_HOST_TRACE",
    "FN64_BLOCK_INSTRUCTION_BUDGET",
    "FN64_BLOCK_MIN_GUEST_INSTRUCTIONS",
    "FN64_BLOCK_PC_TRACE",
    "FN64_BLOCK_PROGRESS_ONLY",
    "FN64_BLOCK_WATCHDOG",
    "FN64_DENSE_MANIFEST_ONLY",
    "FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY",
    "FN64_FRAME_PACE_MS",
    "FN64_PRESENT_MODE",
    "FN64_PROFILE_AOT_BANKS",
    "FN64_PROFILE_AOT_RECENT",
    "FN64_PROFILE_BUILD",
    "FN64_PROFILE_HOST_RECENT",
    "FN64_PROFILE_STOP_AT_GENERATION",
    "FN64_PROFILE_STOP_AT_PC",
    "FN64_RENDER_DUMP_DIR",
    "FN64_RS_EXECUTION",
    "FN64_RT64_SETTINGS_FILE",
    "FN64_SHELL_EXIT_AFTER_PRESENTS",
    "FN64_SHELL_HEADLESS_FRAMES",
    "FN64_SHELL_SPIKE_MS",
    "FN64_WM2000_FRONTIER_BIN",
    "FN64_WM_AOT_BINARY",
    "FN64_WM_DYNAMIC_BINARY",
    "FN64_WM_PAIR_CARGO_CACHE_ROOT",
    "FN64_WM_PAIR_CARGO_CACHE_SEED",
    "FN64_WM_PAIR_RECEIPT",
    "OOT_ASPMAIN",
    "OOT_MAX_STEPS",
    "OOT_PERF_NO_CAPTURE",
    "OOT_RENDER_DUMP_START",
    "OOT_SCRIPT_INTERACTIVE",
    "OOT_STATE_TRACE",
    "OOT_STOP_ON_FRAME",
    "OOT_TRACE",
}

ENV = re.compile(r"\b((?:FN64|OOT|RECOMP)_[A-Z0-9_]+)\b")


def check_env_vars() -> None:
    src = subprocess.run(
        [
            "rg",
            "-o",
            "--no-filename",
            "--glob",
            "*.rs",
            "--glob",
            "*.sh",
            "--glob",
            "*.zsh",
            "--glob",
            "*.toml",
            "--glob",
            "*.c",
            "--glob",
            "*.cc",
            "--glob",
            "*.cpp",
            "--glob",
            "*.cxx",
            "--glob",
            "*.h",
            "--glob",
            "*.hpp",
            "--glob",
            # Python tooling reads env vars too; without this a doc could never
            # cite a variable that only a script in scripts/ consumes.
            "*.py",
            r"(FN64|OOT|RECOMP)_[A-Z0-9_]+",
            ".",
        ],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    live = set(src.split())
    # The game harnesses were extracted to their own repository, but fn64's docs
    # still cite the variables those harnesses consume -- and correctly so: the
    # variables are real, they just are not in this tree any more. Scanning the
    # extracted repo when it is present keeps the drift rule honest instead of
    # flagging 61 live variables as absent.
    #
    # `FN64_SHARD_ROOT` points at the game packages (see fn64-boot-harness's
    # build.rs). Unset or missing, this is a no-op: an absent game repo simply
    # means those citations cannot be checked, not that they are wrong.
    shard_root = os.environ.get("FN64_SHARD_ROOT")
    if shard_root:
        game_repo = Path(shard_root).parent
        if game_repo.is_dir():
            live |= set(subprocess.run(
                ["grep", "-rhoE", r"(FN64|OOT|RECOMP)_[A-Z0-9_]+", str(game_repo)],
                capture_output=True, text=True,
            ).stdout.split())
    # Variables OWNED by the extracted game harnesses. The scan above only
    # helps when a game checkout happens to be present, which it is not on CI
    # or in a plain clone -- so without this list the drift rule fails for
    # everyone but the one developer who has the repo beside fn64.
    #
    # These are still real and still documented here on purpose: fn64's design
    # and status docs describe how the harnesses drive the runtime. The check
    # this file exists to make -- "a documented variable is not a silent
    # no-op" -- is satisfied by the harness that reads them, which lives in
    # recomps/wm2000. Anything NOT on this list must still exist in fn64.
    live |= GAME_HARNESS_ENV
    for doc in docs():
        text = doc.read_text()
        if superseded(text):
            continue
        for lineno, line in enumerate(text.splitlines(), 1):
            low = line.lower()
            # A doc ASSERTING A VAR'S ABSENCE is the drift rule's ally, not its
            # target. runtime-parity-gap.md:262 says FN64_SAVE/FN64_SRAM/...
            # "have zero hits repo-wide" -- that is the measurement, and
            # demanding the names exist would invert the claim.
            if any(k in low for k in ("zero hits", "no environment override",
                                      "appears nowhere", "nothing grades",
                                      "does not exist", "no such")):
                continue
            for var in ENV.findall(line):
                # RECOMP_FUNC is a generated-C symbol prefix, not an env var.
                if var == "RECOMP_FUNC":
                    continue
                # A PREFIX or PLACEHOLDER names a family, not a variable:
                # `FN64_PROFILE_*`, `FN64_PROFILE_<SUBSYSTEM>`, and the
                # `FN64_X=${FN64_X:-<unset>}` shell idiom are all describing a
                # shape. Only a bare, fully-spelled name is a live instruction
                # a reader could paste and have silently no-op.
                rest = line[line.index(var) + len(var):]
                if rest[:1] in ("*", "<") or var in ("FN64_X",):
                    continue
                if var not in live:
                    rel = doc.relative_to(ROOT)
                    fail(f"{rel}:{lineno}", f"documents {var}, which appears nowhere in code")


# --- 3b. closed roadmap items stay short -------------------------------------
# A finished item's detail belongs in git, not in the doc every session reads.
# This check exists because prose did not work: the policy is stated IN
# ROADMAP.md, and the session that wrote it then filed two 15-line completion
# records within the hour. An invariant enforced by review is a bug with a
# delay timer (AGENTS.md), so it is enforced here instead.
CLOSED_ITEM_MAX_LINES = 4


def _scan_closed_items(lines: list[str]) -> None:
    start, label = None, ""
    for i, line in enumerate(lines):
        # An item ends at the next item, or at the next top-level block.
        if start is not None and (line.startswith("- [") or (line and not line[0].isspace())):
            if i - start > CLOSED_ITEM_MAX_LINES:
                fail(
                    f"docs/ROADMAP.md:{start + 1}",
                    f"closed item {label!r} is {i - start} lines (max {CLOSED_ITEM_MAX_LINES}) -- "
                    f"summarize it; the detail is in git",
                )
            start = None
        if line.startswith("- [x]"):
            start, label = i, line[5:47].strip(" *")


def check_closed_roadmap_items() -> None:
    roadmap = ROOT / "docs/ROADMAP.md"
    if roadmap.exists():
        _scan_closed_items(roadmap.read_text().splitlines())


# --- 3c. a doc asserting a hash must have a test that checks it ---------------
# A SHA in a doc claims a REPRODUCIBLE fact ("these two lanes render
# byte-identically at swap 499"). Verified once by hand and never again, it is
# decorative: it cannot fail, so it cannot warn. All 5 hashes in docs/ were
# unbacked when this check was written (ROADMAP V1) -- including the framebuffer
# SHA that is fn64's central c/rs lane-parity claim.
#
# ponytail: a hash is either load-bearing (then a test owns it) or prose (then
# do not write it as evidence). This check forces that choice.
HASH = re.compile(r"\b[0-9a-f]{40,64}\b")


def check_doc_hashes_are_tested() -> None:
    src = subprocess.run(
        ["git", "grep", "-hoE", r"[0-9a-f]{40,64}", "--", "*.rs", "*.sh", "*.py", "*/CMakeLists.txt"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    tested = set(src.split())
    for doc in docs():
        for lineno, line in enumerate(doc.read_text().splitlines(), 1):
            low = line.lower()
            # A COMMIT PIN is provenance ("checked against CEN64 at e0641c8"),
            # not a reproducible measurement -- it is exactly the clean-room
            # citation AGENTS.md requires, and no test should own it. Only
            # CONTENT hashes (a framebuffer SHA, a golden digest) assert a fact
            # a test can re-check. Distinguish by context, not by the hash.
            if any(k in low for k in ("commit", "github.com", "pinned", "tree/", "blob/")):
                continue
            # A ROM/capture IDENTITY is provenance too. `normalized_rom_sha256`
            # names WHICH cartridge image a run used -- the discriminator that
            # caught No Mercy's boot context binding fc561fce…, a different ROM.
            # The file is deliberately not in the repository, so no test can
            # re-derive it; demanding one would push these toward deletion and
            # lose the identity that makes a filed capture auditable.
            if any(k in low for k in ("rom_sha256", "rom `", "sha256  ", "cart ")):
                continue
            # An explicitly-unverified claim is honest prose, not a false gate.
            if "not tested" in low or "no test" in low:
                continue
            for h in HASH.findall(line):
                if h in tested:
                    continue
                rel = doc.relative_to(ROOT)
                fail(
                    f"{rel}:{lineno}",
                    f"asserts content hash {h[:12]}… that no test checks -- either gate it "
                    f"or stop citing it as evidence",
                )


# --- 4. COMPLETENESS's generated ABI inventory must match live code ----------
# It once grepped lib.rs alone; the crate had been split into modules, so it
# matched ZERO of 73 shims and "regenerating" re-asserted a falsehood. The
# dedicated checker now owns both the clean-room 116-name manifest and the
# generated doc block; run it here so every ordinary doc-lint invocation is a
# mechanical surface-drift gate.
def check_completeness_recipe() -> None:
    try:
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/check-nmr-surface.py"), "--check-doc"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except subprocess.TimeoutExpired as error:
        fail(
            "COMPLETENESS.md",
            f"NMR surface checker exceeded 30s (stdout={error.stdout!r}, stderr={error.stderr!r})",
        )
        return
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("COMPLETENESS.md", detail)
    elif VERBOSE:
        print("  NMR surface manifest, live ABI, and completeness doc agree")


# --- 4b. RT64's advertised feature denominator must remain complete ---------
# The ordinary doc gate checks schema, rejection guards, evidence shape, and
# generated-doc drift without requiring an external checkout. Release owners
# additionally pass --rt64-dir directly to the inventory tool to prove the
# pinned source identity and line anchors.
def check_rt64_feature_inventory() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/check_rt64_feature_inventory.py")],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PUBLIC-FEATURE-INVENTORY.md", detail)
    elif VERBOSE:
        print("  RT64 public feature manifest and generated inventory agree")


# --- 4ba. RT64 port source/license/overlay authority must remain closed ------
def check_rt64_port_authority() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/check_rt64_port_authority.py")],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PORT-AUTHORITY.md", detail)
    elif VERBOSE:
        print("  RT64 port source, license, and overlay authority agree")


# --- 4baa. RT64 measurement contract and generated doc remain identical ---
def check_rt64_render_measurement() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/run_rt64_render_baseline.py"), "--check-doc"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-RENDER-MEASUREMENT.md", detail)
    elif VERBOSE:
        print("  RT64 render-measurement schema and generated document agree")


# --- 4bab. RT64 port work inventory must remain a reproducible denominator -
def check_rt64_port_inventory() -> None:
    tool = str(ROOT / "tools/rt64_port_inventory.py")
    mutation = subprocess.run(
        [sys.executable, tool, "--self-test"], cwd=ROOT,
        capture_output=True, text=True,
    )
    if mutation.returncode != 0:
        detail = mutation.stderr.strip() or mutation.stdout.strip() or "mutation checker failed silently"
        fail("RT64-PORT-INVENTORY.md", detail)
        return

    oracle_dir = os.environ.get("FN64_RT64_ORACLE_DIR")
    port_dir = os.environ.get("FN64_RT64_PORT_DIR")
    if bool(oracle_dir) != bool(port_dir):
        fail(
            "RT64-PORT-INVENTORY.md",
            "FN64_RT64_ORACLE_DIR and FN64_RT64_PORT_DIR must be set together",
        )
        return
    command = [sys.executable, tool, "--check"]
    if oracle_dir and port_dir:
        command.extend(["--oracle-dir", oracle_dir, "--port-dir", port_dir])
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PORT-INVENTORY.md", detail)
    elif VERBOSE:
        suffix = "including dual-pin source rederivation" if oracle_dir else "structural; source dirs not configured"
        print(f"  RT64 port inventory schema, mutations, and generated report agree ({suffix})")


# --- 4bac. backend-neutral RT64/Rust parity denominator stays closed --------
def check_rt64_port_parity() -> None:
    tools_path = str(ROOT / "tools")
    sys.path.insert(0, tools_path)
    try:
        import check_rt64_port_parity as parity_checker

        parity_environment = parity_checker.environment_without_ambient_code_injection_variables(
            os.environ
        )
    finally:
        sys.path.remove(tools_path)
    hostile = subprocess.run(
        [sys.executable, str(ROOT / "tools/test_check_rt64_port_parity.py")],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=parity_environment,
    )
    if hostile.returncode != 0:
        detail = hostile.stderr.strip() or hostile.stdout.strip() or "hostile tests failed silently"
        fail("RT64-PORT-PARITY.md", detail)
        return
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools/check_rt64_port_parity.py"),
            "--structural-only",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=parity_environment,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PORT-PARITY.md", detail)
    elif VERBOSE:
        print("  RT64/Rust conformance denominator and fail-closed evidence guards agree")


# --- 4bb. cross-platform RT64 case/blocker matrix must not shrink ----------
def check_rt64_platform_certification() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/rt64_platform_certification.py"), "--check"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PLATFORM-CERTIFICATION.md", detail)
    elif VERBOSE:
        print("  RT64 platform/API case and blocker denominators agree")


# --- 4bc. private inputs stay external while the admission contract works --
def check_private_input_admission() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/private_input_admission.py"), "--check"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("PRIVATE-INPUT-ADMISSION.md", detail)
    elif VERBOSE:
        print("  private-input admission rejects identity/content leakage")


# --- 4c. base-renderer accuracy is a generated, non-shrinking denominator --
def check_base_renderer_matrix() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/check_base_renderer_matrix.py")],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("BASE-RENDERER-BEHAVIOR-MATRIX.md", detail)
    elif VERBOSE:
        print("  base-renderer behavior matrix and generated report agree")


# --- 4d. RT64 port workflow status and generated views agree ----------------
def check_rt64_port_dashboard() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/rt64_port_dashboard.py"), "--check"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PORT-DASHBOARD.md", detail)
    elif VERBOSE:
        print("  RT64 port ticket manifest and generated dashboards agree")


# --- 4e. a module's "deliberately not ported" list must still be true --------
# RT64 port modules document what they DID NOT port. Those deferrals are
# load-bearing: the next card reads them to decide what is out of scope. When
# one goes stale -- the symbol has since landed under its snake_case name --
# the card either skips real work or re-ports a function that already exists
# and is `pub`. That cost real work four times in one day: `rt64_math.rs`
# still refuses nine symbols that `rt64_math_matrix.rs`/`rt64_math_decompose.rs`
# have since landed, `rt64_rsp_segment.rs` still refuses `matrixCommon`, and
# `rt64_blender_analysis.rs` still refuses `EmulationRequirements`.
#
# The rule: inside a sentence that REFUSES to port something, every backticked
# upstream C++ identifier is mapped to its Rust spelling and looked up in the
# same crate's public surface. A hit means the refusal is false.
#
# False positives are low BY CONSTRUCTION, not by an exception list: the
# deferrals that are correct refuse things that were never ported under any
# name -- XXH3, `nlohmann::json`, RHI/COM types, GBI dispatch orchestrators.
# A symbol-existence check passes all of those silently, because no such
# symbol exists to find.
# `\*{0,2}` absorbs markdown emphasis: the live prose writes "does **not**
# port" and "is **deliberately not ported**", and a regex that misses those
# picks up a LATER verb in the same sentence instead, which silently moves the
# extraction scope past the symbols the refusal actually names.
_NOT = r"\*{0,2}not\*{0,2}"
DEFERRAL = re.compile(
    rf"deliberately {_NOT} ported|does {_NOT} port|{_NOT} re-ported"
    # "is/are/were not ported", and the heading forms that introduce a list:
    # "Specifically not ported, by category:", "Explicitly not ported:".
    rf"|(?:are|is|were|was|specifically|explicitly|deliberately)\s+{_NOT} ported"
    r"|\(deferred|deferred\s*--|out of scope",
    re.I,
)
# A bare ticket ID with no symbol beside it rots silently: when the ticket
# lands under a different module name, nothing connects the two. Form, not fact.
TICKET = re.compile(r"\bM\d\.\d+[a-z]?\b")
DOC_LINE = re.compile(r"^\s*//[/!] ?")
# `[`Foo`]` is a rustdoc intra-doc link -- a LIVE reference to an item in this
# crate, which is the opposite of a deferral. Strip before extracting.
INTRA_DOC_LINK = re.compile(r"\[`[^`\n]+`\]")
BACKTICKED = re.compile(r"`([^`\n]+)`")
RUST_ITEM = re.compile(
    r"^([^:]+):(\d+):(\s*)(pub(?:\([^)]*\))? )?(fn|struct|enum) ([A-Za-z_][A-Za-z0-9_]*)"
)


def _is_upstream_symbol(token: str) -> bool:
    """True for an upstream C++ name: camelCase, PascalCase, or a dimension
    suffix (`extract3x3`) that has no case transition at all."""
    if len(token) < 3:
        return False
    return bool(re.search(r"[a-z][A-Z]", token) or re.search(r"[a-z][0-9]", token))


def _to_snake(name: str) -> str:
    """camelCase -> snake_case, splitting at the digit boundary.

    `extract3x3` -> `extract_3x3` and `lerpMatrix3x3` -> `lerp_matrix_3x3`:
    the letter->digit boundary separates, but a DIMENSION RUN (`3x3`) is one
    token and must not become `3x_3`."""
    s = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", name)
    s = re.sub(r"(?<=[A-Z])(?=[A-Z][a-z])", "_", s)
    s = re.sub(r"(?<=[A-Za-z])(?=[0-9])", "_", s).lower()
    s = re.sub(r"(?<=[0-9]x)_(?=[0-9])", "", s)
    return re.sub(r"__+", "_", s).strip("_")


def _to_snake_joined(name: str) -> str:
    """The other defensible digit reading: `mat4Mul` -> `mat4_mul`.

    Which spelling a porter picked is not knowable from the C++ name, so try
    both -- a deferral is stale if the symbol landed under EITHER."""
    s = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", name)
    s = re.sub(r"(?<=[A-Z])(?=[A-Z][a-z])", "_", s).lower()
    return re.sub(r"__+", "_", s).strip("_")


def _refused_symbol(span: str) -> str | None:
    """The one symbol a backticked span refuses.

    `Foo::bar` refuses the MEMBER, not the parent type -- crediting the parent
    reports `ProfilingTimer` as landed when the deferral is about
    `ProfilingTimer::start`."""
    s = span.strip()
    if "::" in s:
        s = s.split("::")[-1]
    s = re.sub(r"\(.*$", "", s).strip()
    return s if re.fullmatch(r"[A-Za-z][A-Za-z0-9]*", s) else None


def _crate_of(path: str) -> str:
    return path.split("/")[1] if path.startswith("crates/") else ""


def _rust_item_index() -> tuple[dict, dict]:
    """(public, private) maps of (crate, name) -> "file:line".

    Indexed PER CRATE because a deferral in crate X is a claim about crate X's
    port. Workspace-wide lookup makes every common name (`Vec3`, `LoadTile`,
    `DepthMode`) collide with an unrelated definition elsewhere. Tests,
    examples and bins are excluded: they are not the crate's ported surface."""
    out = subprocess.run(
        ["git", "grep", "-nE",
         r"^\s*(pub(\([^)]*\))? )?(fn|struct|enum) [A-Za-z_]",
         "--", "crates/*.rs", "crates/**/*.rs"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    public: dict[tuple[str, str], str] = {}
    private: dict[tuple[str, str], str] = {}
    for line in out.splitlines():
        m = RUST_ITEM.match(line)
        if not m:
            continue
        path, lineno, _, vis, _kind, name = m.groups()
        if "/tests/" in path or "/examples/" in path or "/bin/" in path:
            continue
        # `pub(crate)`/`pub(super)` is NOT reachable from another module the
        # way a card would need; count it with private.
        target = public if (vis and "pub(" not in vis) else private
        target.setdefault((_crate_of(path), name), f"{path}:{lineno}")
    return public, private


def _doc_blocks(lines: list[str]):
    """Yield (first_lineno, [text...]) for each contiguous doc-comment run."""
    i = 0
    while i < len(lines):
        if DOC_LINE.match(lines[i]):
            j = i
            while j < len(lines) and DOC_LINE.match(lines[j]):
                j += 1
            yield i, [DOC_LINE.sub("", line) for line in lines[i:j]]
            i = j
        else:
            i += 1


def check_stale_deferrals() -> None:
    """A module's deferral list must not name a symbol that has since landed.

    Three severities, deliberately kept apart:

      fail  -- the refused symbol exists and is `pub` in this crate. The
               deferral is FALSE; a card reading it will skip landed work or
               duplicate a public function.
      warn  -- the refused symbol exists but is PRIVATE. Different situation,
               different fix (widen the visibility, do not re-port), and the
               prose saying so may be exactly correct -- `rt64_rigid_body.rs`
               correctly documents that `Quat::dot`/`neg`/`normalize` are
               private and unreachable. Reporting that as stale would punish
               accurate prose.
      warn  -- the deferral cites only a bare ticket ID (M4.x) with no symbol.
               It rots silently when the ticket lands under another module
               name. A complaint about FORM; all such sites currently resolve.

    LIMITATION 1 -- LIST-FORM deferrals. Findings are scoped to the sentence
    that does the refusing, because every wider scope tried was measurably
    worse (a 6-line window reported 141 findings against this tree, mostly
    from neighbouring sentences; inheriting a colon heading across its whole
    bullet run reported 88). The cost is that a deferral written as a bare
    heading over a list -- "Specifically not ported, by category:" followed by
    bullets, as in `rt64_rsp_segment.rs:232` -- is only caught when a bullet
    carries its own refusal verb. That site's stale `matrixCommon` claim is
    still reported here, but via `rt64_rsp_matrix_stack.rs:142`, which repeats
    it in sentence form. Do not widen the scope to close this without
    re-measuring the false-positive count.

    LIMITATION 2 -- what this check CANNOT catch at all. It is referential: it asks
    "does this name exist?". It cannot catch a GRAMMATICAL failure, where
    prose describing an UN-PORTED upstream construct's requirements parses as
    the module's own requirement. "it requires `WorkloadQueue`, `Workload`"
    names symbols that correctly do NOT exist, so this check passes it
    silently -- yet that one sentence made five landed modules read as
    blocked. `rt64_frame_compatibility.rs:225-228` models the fix in prose:
    "Read the exclusions below as refusals, not as a dependency list." No
    symbol-existence check covers that class; do not assume it does."""
    public, private = _rust_item_index()
    sources = [
        f for f in subprocess.run(
            ["git", "ls-files", "crates/"], cwd=ROOT,
            capture_output=True, text=True, check=True,
        ).stdout.split()
        if f.endswith(".rs") and "/tests/" not in f and "/examples/" not in f
    ]
    for path in sources:
        crate = _crate_of(path)
        lines = (ROOT / path).read_text().splitlines()
        for start, body in _doc_blocks(lines):
            text = " ".join(body)
            offsets, pos = [], 0
            for k, chunk in enumerate(body):
                offsets.append((pos, start + k + 1))
                pos += len(chunk) + 1

            def lineno_at(offset: int, _o=offsets, _s=start) -> int:
                found = _s + 1
                for begin, num in _o:
                    if begin <= offset:
                        found = num
                    else:
                        break
                return found

            # A module that QUOTES the old deferral to announce it CLOSED it is
            # citing history, not making a claim -- rt64_math_matrix.rs opens by
            # quoting rt64_math.rs's refusal of the very cluster it just landed.
            # Mask quoted spans so that citation is not read as a live refusal.
            masked = re.sub(r'"[^"]*"', lambda m: " " * len(m.group(0)), text)

            # A deferral often takes LIST form: a heading refuses ("Specifically
            # not ported, by category:") and the symbols sit in the bullets
            # under it. rt64_rsp_segment.rs's `matrixCommon` -- one of the three
            # sites this check exists for -- is exactly that shape, so scoping
            # to the trigger's own sentence alone would miss it. When a trigger
            # sentence ends in a colon, the bullet run that follows inherits it.
            # Scope each finding to the SENTENCE that does the refusing.
            # Anything wider misfires: a 6-line window drags in symbols from
            # neighbouring sentences, and inheriting a list heading across a
            # whole bullet run sweeps in unrelated lists. Measured on this
            # tree, sentence scope reports 30 findings that are all genuine,
            # where window scope reported 141 and run-inheritance 88.
            #
            # The cost of this precision is stated in the docstring: a
            # deferral written as a bare colon heading over a bullet list
            # (rt64_rsp_segment.rs's `matrixCommon`) is NOT caught unless a
            # bullet carries its own refusal verb.
            # Split on sentence punctuation, but NOT on a period that is inside
            # a ticket ID or version number ("M4.3", "1,314") -- otherwise the
            # bare-ticket rule below can never see the ticket it is about.
            spans: list[tuple[int, int]] = []
            # A colon ends the refusing clause too: "...`.cpp` (SHA-256 ...):
            # Only the `ShaderDescription` struct and its `toShader()`..." is
            # two claims, and letting the first run into the second credits the
            # refusal with names the module actually DOES port.
            for m in re.finditer(r"(?:[^.;:]|\.(?=\d))*(?:\.|;|:|$)", masked):
                if m.group(0).strip() and DEFERRAL.search(m.group(0)):
                    spans.append((m.start(), m.end()))

            for begin, finish in spans:
                sentence = text[begin:finish]
                # A split can land INSIDE a backticked span, leaving an
                # unpaired backtick. That inverts every pair after it, so
                # extraction yields the prose BETWEEN symbols instead of the
                # symbols -- silently losing real findings. Re-pair by walking
                # back to the split's own opening tick: everything before the
                # first tick is a fragment of a span that started in the
                # previous sentence, so only that fragment is dropped.
                # The orphan may be at EITHER end: a split can cut into a span
                # that opened in the previous sentence (leading orphan) or one
                # that closes in the next (trailing orphan). Close a trailing
                # opener rather than dropping it -- `lerpMatrix3x3` in
                # rt64_math.rs's refusal is cut by its own "3." and is a real
                # finding, not a fragment.
                if sentence.count("`") % 2:
                    # Two repairs are possible and which is right depends on
                    # where the split fell: close a trailing opener (the span
                    # continues past the cut, as with `lerpMatrix3x3` cut by
                    # its own "3."), or drop a leading closer (the span opened
                    # in the previous sentence). Guessing wrong silently loses
                    # findings in the other case, so try both and keep whichever
                    # actually yields upstream symbols.
                    # Prefer closing a trailing opener: it keeps the symbol the
                    # split cut in half without absorbing any text the refusal
                    # does not govern. Dropping the leading fragment instead
                    # re-pairs the WHOLE sentence, which on a survey paragraph
                    # ("the closest sibling defines `A`, `B` ... and it
                    # deliberately does not port `C`") credits the refusal with
                    # every name in the survey. Only fall back to that when
                    # closing yields nothing at all.
                    closed = sentence + "`"
                    if any(
                        (tok := _refused_symbol(span)) and _is_upstream_symbol(tok)
                        for span in BACKTICKED.findall(closed)
                    ):
                        sentence = closed
                    else:
                        head, _, rest = sentence.partition("`")
                        sentence = head + " " + rest
                where = f"{path}:{lineno_at(begin)}"
                # A refusal governs what FOLLOWS it, not what precedes it.
                # "Only the `ShaderDescription` struct and its `toShader()`
                # method are ported (the `hash()` ... are explicitly not
                # ported)" refuses only the parenthetical -- crediting the
                # whole clause reports the two symbols the module DOES port.
                verb = DEFERRAL.search(sentence)
                scope = sentence[verb.start():] if verb else sentence
                # "does not port X" puts its objects AFTER the verb; the
                # trailing form ("`X`/`Y` ... are explicitly not ported") puts
                # them before it. Distinguish by the word order actually used:
                # only a trailing-form verb (one with no backticked object
                # following it) may look backwards, and then only as far as the
                # nearest clause break, so a "these ARE ported" subject earlier
                # in the sentence is not swept in.
                if verb and not BACKTICKED.search(scope):
                    head = sentence[:verb.start()]
                    # Split on a parenthetical opener -- `\(\s` or `\((?=the)`
                    # style -- never on the `()` inside a C++ method name like
                    # `maskUnusedParameters()`, which would shred the clause.
                    # A parenthetical opens after whitespace; the `()` inside a
                    # C++ method name (`maskUnusedParameters()`) never does, so
                    # requiring the space keeps such names intact.
                    clause = re.split(r"(?<=\s)\(|--|—|,\s+(?:and|but)\s",
                                      head)[-1]
                    scope = clause if BACKTICKED.search(clause) else head
                seen: set[str] = set()
                found_symbol = False
                for span in BACKTICKED.findall(INTRA_DOC_LINK.sub("", scope)):
                    token = _refused_symbol(span)
                    if not token or not _is_upstream_symbol(token) or token in seen:
                        continue
                    seen.add(token)
                    found_symbol = True
                    spellings = {token, _to_snake(token), _to_snake_joined(token)}
                    hit = next(
                        (public[(crate, s)] for s in spellings if (crate, s) in public),
                        None,
                    )
                    if hit:
                        fail(where, f"defers `{token}`, but it is public at {hit} "
                                    f"-- the deferral is stale")
                        continue
                    hidden = next(
                        (private[(crate, s)] for s in spellings if (crate, s) in private),
                        None,
                    )
                    if hidden:
                        warn(where, f"defers `{token}`, which exists but is private at "
                                    f"{hidden} -- widening, not porting, is the fix")
                if not found_symbol and TICKET.search(sentence):
                    ticket = TICKET.findall(sentence)[0]
                    warn(where, f"defers to bare ticket {ticket} with no symbol named "
                                f"-- this rots silently if {ticket} lands elsewhere")


# --- 5. no doc may cite a scripts/ entry point that isn't executable ---------
def check_scripts() -> None:
    for doc in docs():
        for raw in REF.findall(doc.read_text()):
            ref = raw.rstrip(TRAIL)
            if ref.startswith("scripts/") and ref.endswith(".sh"):
                p = ROOT / ref
                if p.exists() and not p.stat().st_mode & 0o111:
                    fail(str(doc.relative_to(ROOT)), f"cites {ref}, which is not executable")


def _closed_item_check_fires() -> bool:
    """Feed the closed-item check a bloated item and assert it complains.
    Reconstructs the exact mistake this check exists to catch: a [x] item that
    carries its verification detail instead of a summary."""
    global errors
    saved, errors = errors, []
    try:
        bloated = [
            "- [x] **H1 vendor `recomp.h`** (2026-07-17) — in-tree.",
            "  Verified: the C lane builds with RECOMP_H_DIR unset;",
            "  override still honored; 621/621 workspace tests pass;",
            "  vendored copy byte-identical to upstream a8e2200.",
            "  Two things the inventory got wrong, found by doing it:",
            "  only recomp.h was needed, and oot.toml must not move.",
            "- [ ] **H3 next item**",
        ]
        _scan_closed_items(bloated)
        return len(errors) == 1
    finally:
        errors = saved


def selftest() -> int:
    """Prove the checks can FAIL. A linter that passes because it isn't looking
    is worse than none -- the first cut of REF excluded backticks, matched 4
    refs instead of 53, and reported clean while blind to every real bug."""
    global errors
    cases = [
        ("dangling ref", "AGENTS.md:1: references docs/NOPE.md",
         lambda: REF.findall("see `docs/NOPE.md` for more") == ["docs/NOPE.md"]),
        ("backticked ref matches", "the blindness regression",
         lambda: REF.findall("`crates/fn64-abi/src/lib.rs`") == ["crates/fn64-abi/src/lib.rs"]),
        ("bare ref matches", "unbackticked form",
         lambda: REF.findall("run scripts/native-emit.sh now") == ["scripts/native-emit.sh"]),
        ("glob skipped", "illustrative wildcards aren't claims",
         lambda: all("*" in r for r in REF.findall("`crates/*/tests/`"))),
        ("env var regex", "catches a fake var",
         lambda: ENV.findall("set `FN64_TOTALLY_FAKE=1`") == ["FN64_TOTALLY_FAKE"]),
        ("closed-item cap fires", "a bloated [x] item must fail",
         _closed_item_check_fires),
        # The digit boundary is the part that silently gets this wrong.
        ("camel->snake digit boundary", "extract3x3 must not become extract_3x_3",
         lambda: _to_snake("extract3x3") == "extract_3x3"
         and _to_snake("lerpMatrix3x3") == "lerp_matrix_3x3"
         and _to_snake("rotationFrom3x3") == "rotation_from_3x3"
         and _to_snake("matrixRotationX") == "matrix_rotation_x"
         and _to_snake("matrixCommon") == "matrix_common"),
        ("upstream-symbol detection", "C++ names in, non-symbols out",
         lambda: all(map(_is_upstream_symbol,
                         ["extract3x3", "matrixCommon", "checkEmulationRequirements"]))
         and not any(map(_is_upstream_symbol,
                         # the correct deferrals this must stay silent on
                         ["XXH3", "json", "RHI", "COM", "to_json", "hlslpp"]))),
        ("qualified name credits the member", "Foo::bar defers bar, not Foo",
         lambda: _refused_symbol("ProfilingTimer::start") == "start"
         and _refused_symbol("Quat::dot") == "dot"
         and _refused_symbol("Float4ToRGBA16(float4 i, uint d)") == "Float4ToRGBA16"),
        ("deferral trigger fires and abstains", "refusals yes, incidental prose no",
         lambda: DEFERRAL.search("Does not port `matrixScale`")
         and DEFERRAL.search("**deliberately not ported**")
         and not DEFERRAL.search("a YUV-deferred Tile/Block contract")
         and not DEFERRAL.search("no deferred-selector rejection path")),
    ]
    bad = [name for name, why, fn in cases if not fn()]
    for name in bad:
        print(f"  SELFTEST FAIL: {name}")
    if bad:
        return 1
    print(f"lint-docs selftest: {len(cases)} checks can fail correctly")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    for fn in (check_refs, check_readme_crates, check_env_vars,
               check_closed_roadmap_items, check_doc_hashes_are_tested,
               check_completeness_recipe, check_rt64_feature_inventory,
               check_rt64_port_authority, check_rt64_render_measurement,
               check_rt64_port_inventory,
               check_rt64_port_parity,
               check_rt64_platform_certification,
               check_private_input_admission,
               check_base_renderer_matrix,
               check_rt64_port_dashboard,
               check_generated_validators,
               check_stale_deferrals,
               check_scripts):
        fn()
    if warnings:
        print(f"lint-docs: {len(warnings)} warning(s)\n")
        for w in warnings:
            print(f"  {w}")
        print()
    if errors:
        print(f"lint-docs: {len(errors)} error(s)\n")
        for e in errors:
            print(f"  {e}")
        return 1
    print(f"lint-docs: clean ({checked} refs across {len(docs())} docs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
