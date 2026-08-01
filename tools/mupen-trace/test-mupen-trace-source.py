#!/usr/bin/env python3
"""Source-level contract checks for the bounded Mupen trace producer."""

from pathlib import Path


SOURCE = Path(__file__).with_name("mupen_trace.c").read_text(encoding="utf-8")


def main() -> None:
    required = (
        'getenv("FN64_FAST_FORWARD_PC")',
        "aligned resident VA",
        "capture_start_pc = (uint32_t)parsed",
        "if (pc == capture_start_pc)",
        "int entered_recording = 0;",
        "if (entered_recording)",
        "have_pending_pause = 1;",
        "if (!have_pending_pause)",
        "emit_boot_context(boot_context_path",
        "DebugSetCallbacks(dbg_init, dbg_update, dbg_vi)",
    )
    missing = [fragment for fragment in required if fragment not in SOURCE]
    if missing:
        raise SystemExit("mupen trace source contract missing: " + ", ".join(missing))
    recording_transition = SOURCE.index("recording = 1;")
    image_capture = SOURCE.index("if (image_pending && pc == image_pc)")
    stop_after_image = SOURCE.index("if (stop_after_image)", image_capture)
    zero_step_completion = SOURCE.index(
        "complete_trace(out, out_path, seq, recorded);", stop_after_image
    )
    entry_step = SOURCE.index("if (entered_recording)", zero_step_completion)
    if not (
        recording_transition
        < image_capture
        < stop_after_image
        < zero_step_completion
        < entry_step
    ):
        raise SystemExit(
            "mupen trace source contract: recording-start capture and stop must run before stepping"
        )
    print("mupen-trace source contract: ok")


if __name__ == "__main__":
    main()
