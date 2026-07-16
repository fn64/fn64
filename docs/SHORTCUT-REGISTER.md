
## Addendum (2026-07-16): flaky fn64-abi subprocess-abort tests
`fn64-abi`'s `__*_abort_subprocess_entry` tests spawn child processes that
`abort()` to verify loud-trap behavior. Intermittently the child's abort signal
races into the PARENT test-runner's exit code -> `cargo test -p fn64-abi` exits
101 even though every `test result:` line says `ok` and 0 assertions fail
(verified: 5 runs, 0 real failures, exit flakes 101/0/0). This makes
"is the workspace green?" UNRELIABLE and can mask a real regression. Fix: run
the abort-subprocess entries in a way that doesn't leak the child's exit into
the harness (e.g. a dedicated harness that catches the child status, or gate
them behind an explicit `--ignored` + a separate checked runner). Severity: B
(dev-ergonomics + it undermines the test signal). NOTE: lesson recorded — do
NOT push on an exit-101 without first confirming (per-`test result:` line) that
no real assertion failed; treat exit-101 as investigate, not assume-flake.
