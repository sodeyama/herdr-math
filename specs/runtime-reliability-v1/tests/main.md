# Runtime Reliability V1 Acceptance Tests

## Status

- Contract state: **Draft** — ids are stable; no test is implemented or passed yet.
- Plan: `../plans/main.md`
- Task checklist: `../tasks/main.md`
- Conventions follow `specs/terminal-math-v2/tests/main.md`: a failed, skipped,
  retried, or unimplemented case is not a pass; terminal cases record evidence
  under `docs/evidence/`.

Test id scheme: `AT-R-<group><nn>` — groups: 1 installer/launcher, 2 shell
wrapper, 3 tmux transport gate, 4 smoke/CI isolation, 5 hygiene.

Background: on 2026-08-05 every `tmath` invocation on PATH died with SIGKILL
(exit 137, no output) because the PATH entry had been overwritten in place with
a raw Mach-O binary, poisoning the macOS kernel code-signature cache for that
inode. The shell wrapper then silently passed through (no watcher, no message),
so both plain-terminal use and coding-agent use appeared "dead" with no
diagnostic anywhere. Independently, the tmux outer-terminal gate refused or
killed viewers in real sessions, and the headless smoke test for the agent
pipeline no longer passes. This contract makes each failure loud, recoverable,
and covered by an automated test.

## Group 1 — Installer and launcher integrity

- **AT-R-101** Atomic launcher install: `scripts/install.sh` writes the PATH
  launcher to a temporary file in the same directory and renames it over
  `$BIN_HOME/tmath` (never truncate-in-place on an existing file). After
  install, `"$BIN_HOME/tmath" --version` exits 0 and the file's first line is
  `#!/bin/sh`. Verified by `scripts/smoke-install-launcher.sh` (fake
  `BIN_HOME`, no real home mutation).
- **AT-R-102** Foreign-file replacement: when the existing `$BIN_HOME/tmath` is
  not a launcher script (first two bytes are not `#!`), the installer prints
  exactly one bounded stderr line
  `tmath: replacing non-launcher file at $BIN_HOME/tmath` and still installs a
  working launcher at a fresh inode. The smoke test seeds a copy of a compiled
  binary at that path and asserts the inode changed and the result executes.
- **AT-R-103** `tmath diagnose` PATH-launcher check: diagnose resolves `tmath`
  through `$PATH`, runs the resolved file with `--version` as a subprocess
  (5 s timeout), and prints one of:
  `path launcher: ok (tmath <version>)`,
  `path launcher: broken (exit <code>)`,
  `path launcher: not found on PATH`.
  A broken or missing launcher makes diagnose exit nonzero (so the installer
  gate fails loudly). The subprocess result must be reported even when the
  child is killed by a signal (report `exit 137`, not a panic).
- **AT-R-104** Version skew check: when the resolved PATH launcher reports a
  version different from the running binary's version, diagnose prints
  `path launcher: version skew (path <a>, this binary <b>)` as a warning line
  without failing the run.

## Group 2 — Shell wrapper failure visibility

- **AT-R-201** Wrapper distinguishes crash from "not allowlisted":
  `__tmath_wrap_agent` captures the exit code of `tmath agent-allowed`.
  Exit 0 → start the watcher as today. Exit 1 → silent passthrough (existing
  AT-2-815 behavior). Any other exit code (2, 126, 127, 137, ...) → print one
  bounded stderr line
  `tmath: agent-allowed failed (exit <code>); run 'tmath diagnose'` and pass
  through. The wrapped command runs with unchanged arguments in every branch.
- **AT-R-202** Failure-mode passthrough regression: with a stub `tmath` on PATH
  that exits 137, running a wrapped command produces the wrapped command's own
  stdout unchanged plus exactly one wrapper warning line on stderr; the
  wrapper's exit status equals the wrapped command's exit status.
- **AT-R-203** No orphan sessions: when the wrapper created a dedicated tmux
  session (`tmath-$$`) and the wrapped command exits, the session is destroyed
  within 5 seconds (no pane may linger on `exec $SHELL`). The background
  watcher exits on its own when the source pane no longer exists. Verified
  headless on a private tmux socket.

## Group 3 — tmux transport gate

- **AT-R-301** Transport env propagation from the wrapper: the
  `__tmath_start_in_new_tmux_session` path starts the watcher as a background
  process of the launching shell (never as a tmux-spawned pane command), so
  `TMATH_TMUX_TRANSPORT` (and `TMATH_DPR`, `TMATH_DEBUG_LOG` when set in the
  launching shell) reach the watcher by ordinary environment inheritance and
  the new session contains only the wrapped command's pane plus the watcher's
  own viewer split — no dedicated watcher pane. Verified behaviorally: with
  `TMATH_TMUX_TRANSPORT=passthrough` exported in the launching shell, a
  headless (no attached client) launch still reaches `watching %` instead of
  the fail-closed `no attached client` refusal.
- **AT-R-302** Distinct refusal diagnostics: the gate in
  `terminal_output.rs::selected_route()` distinguishes and reports:
  - no tmux client attached →
    `tmux has no attached client; graphics need an attached Kitty-capable terminal`
  - client attached but unverified →
    `tmux outer terminal '<client_termname>' is not a verified Kitty target; set TMATH_TMUX_TRANSPORT=client-tty or passthrough to override`
  The messages contain only the advertised termname (bounded, no document
  content, no paths). Unit-tested through a seam that injects the tmux query
  results (no real tmux required).
- **AT-R-303** Transient no-client tolerance: a viewer/watcher that has already
  validated its route once does not exit permanently on a later refusal whose
  cause is "no attached client"; it skips the emission, logs one bounded event
  (`route_unavailable clients=0`), and retries on the next emission. It may
  exit only after 30 consecutive unavailable emissions. Covered by a unit test
  on the retry counter plus a headless integration run.
- **AT-R-304** Diagnose shows gate inputs: inside tmux, `tmath diagnose` prints
  the attached-client count, `client_termname`, `allow-passthrough` value,
  transport env value or `<unset>`, and the resulting route or the refusal
  reason. (Extends the existing `tmux_diagnostics()` block.)

## Group 4 — Smoke and CI isolation

- **AT-R-401** `scripts/smoke-agent-tmux.sh` passes headless on a machine that
  already runs a user tmux server: every tmux call uses a private socket
  (`tmux -L tmath-smoke-$$`), the transport env is embedded in the watcher
  command line (`TMATH_TMUX_TRANSPORT=passthrough`), the private server is
  killed on exit (trap), and the user's default server session list is
  byte-identical before and after the run.
- **AT-R-402** `scripts/smoke-agent-wrapper-tmux.sh` uses the same private
  socket isolation and cleanup; it additionally asserts AT-R-201/202 with a
  broken-stub tmath and AT-R-301 (background watcher inherits the transport
  env; no dedicated watcher pane in the session).
- **AT-R-403** CI runs both smoke scripts on Linux with tmux installed as a
  required job; a FAIL line from either script fails the job.

## Group 5 — Hygiene

- **AT-R-501** Debug scaffolding removed: no `#region agent log` blocks, no
  hardcoded `sessionId`/`runId` literals, and no `debug_log`/`debug_log_current`
  helpers remain under `engine/crates/`. Generic bounded diagnostics may stay
  only behind the documented `TMATH_DEBUG_LOG` variable through a single shared
  helper. `cargo test --workspace` and `cargo clippy --all-targets` stay green;
  `grep -rn "region agent log" engine/` returns nothing.
- **AT-R-502** Viewer `RendererFailed` on sync/status-bar: an integration test
  drives the viewer sync path with a failing render and asserts the viewer
  reports the stable error code once, keeps earlier placements intact, and
  continues serving subsequent documents. If the 2026-08-05 field failure
  (`sync_failed (RendererFailed)` followed by viewer exit) cannot be reproduced
  after a bounded investigation, the evidence file records the attempted
  reproductions and the task closes without a code change.
