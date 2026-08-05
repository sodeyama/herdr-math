# Runtime Reliability V1 Plan

## Status

- Plan state: **Draft**
- Acceptance contract: `../tests/main.md`
- Task checklist: `../tasks/main.md`
- Scope: make tmath's installed runtime fail loudly and recover cleanly. This
  plan complements `specs/terminal-math-v3` (render engine migration) and does
  not change the render pipeline itself.

## Incident summary (2026-08-05, macOS/Ghostty)

Observed: `tmath` produced no output anywhere — not in a plain terminal, not
from coding agents (Claude Code and others).

Root causes found by live investigation, in order of impact:

1. **Poisoned PATH launcher (primary).** `~/.local/bin/tmath` should be the
   `#!/bin/sh` launcher that `scripts/install.sh` writes, but it had been
   overwritten in place with a copy of `target/release/tmath` (a 45 MB Mach-O).
   On macOS, overwriting an already-executed file in place invalidates the
   kernel's per-inode code-signature cache: the kernel then kills the process
   at exec with SIGKILL (exit 137, zero output), even though `codesign
   --verify` passes and the identical bytes run fine from any other inode.
   Every `tmath` invocation on PATH died instantly.
2. **Silent wrapper passthrough (amplifier).** The shell wrapper
   (`scripts/shell/tmath-agent.sh`) probes `tmath agent-allowed` and treats
   any nonzero exit as "not allowlisted", so the SIGKILL (137) looked
   identical to "directory not enabled": the wrapper passed through without
   any message. Users saw coding agents run with no viewer and no hint that
   tmath was broken.
3. **tmux gate refusals in real sessions (independent).** The fail-closed gate
   in `terminal_output.rs::selected_route()` refuses graphics when
   `TMATH_TMUX_TRANSPORT` is unset and the outer terminal cannot be verified.
   Field logs from 2026-08-05 show a live Ghostty session where the viewer
   placed 649 blocks and then hit repeated
   `tmux outer terminal is not a verified Kitty target` refusals (client
   detach makes `client_termname` unavailable) and `sync_failed
   (RendererFailed)`, after which viewers exited. Two aggravating factors:
   - tmux-spawned commands inherit the tmux **server's** environment, so a
     user's `export TMATH_TMUX_TRANSPORT=...` in `.zshrc` never reaches
     watchers started via `tmux new-session`/`split-window` command strings.
     (The watcher→viewer hop already forwards the variable explicitly —
     `agent_watcher.rs` builds an `env NAME=value` prefix — but the
     wrapper→watcher hop does not.)
   - the gate reports one generic message for two different states (no client
     attached vs. unverified terminal), which makes field debugging guesswork.
4. **Headless smoke regression (test debt).** `scripts/smoke-agent-tmux.sh`
   fails at "watcher did not start" because the gate (added after the script
   was written) refuses headless sessions, and because the script runs on the
   user's default tmux server where exported variables do not propagate and
   stale `tmath-*` sessions and global environment (`TMATH_VIEWER_LOG*`)
   accumulate.
5. **Leftover debug scaffolding (hygiene).** `terminal_output.rs` carries
   committed `#region agent log` instrumentation with hardcoded
   `sessionId`/`runId` literals from a past debugging session.

Field repair already applied on the affected machine (not a repo change):
the raw binary at `~/.local/bin/tmath` was replaced by the proper launcher
script at a fresh inode; `tmath --version`, `tmath agent-allowed`, and the
wrapper smoke test then passed.

## Design decisions

- **D-R1 Fresh-inode installs.** The installer never truncates an existing
  executable in place. It writes to a sibling temporary file and `mv`s over
  the target. This makes the macOS signature-cache poisoning impossible to
  recreate through supported install paths, and cheap to detect when users
  copy binaries manually (AT-R-101/102).
- **D-R2 Diagnose owns runtime health.** `tmath diagnose` is the single
  documented answer to "tmath does nothing": it must detect a broken PATH
  launcher by actually executing it as a subprocess and reporting the exit
  code, including signal deaths (AT-R-103/104). The installer's post-install
  gate already runs diagnose, so a broken launcher fails the install loudly.
- **D-R3 Wrapper is quiet on policy, loud on breakage.** Exit 1 from
  `agent-allowed` stays silent (policy: directory not enabled). Every other
  nonzero exit prints exactly one actionable stderr line and never blocks the
  wrapped command (AT-R-201/202). No new exit codes are added to tmath; the
  0/1 contract is unchanged.
- **D-R4 Explicit env propagation across tmux.** Any command line handed to
  tmux (`new-session`, `split-window`) that runs tmath embeds the transport
  and DPR variables as an `env` prefix, reusing the forwarding list already
  proven in `agent_watcher.rs`. Relying on tmux session environment is
  forbidden for correctness-relevant variables (AT-R-301).
- **D-R5 Gate reports what it saw and tolerates transients.** Refusals name
  the observed `client_termname` or the no-client state (AT-R-302); a
  previously validated viewer treats a no-client refusal as a skippable
  transient with a bounded retry budget instead of dying (AT-R-303).
- **D-R6 Smoke tests own their tmux server.** All tmux smoke scripts run on a
  private `-L` socket, embed their env in command strings, and kill their
  server on exit, so they pass on developer machines with live tmux servers
  and in CI (AT-R-401/402/403).

## Phases

- **Phase R1 — Unbreak and detect (highest value, lowest risk).**
  Installer atomic launcher + foreign-file replacement; diagnose PATH-launcher
  check; wrapper failure visibility. Tasks TR-101..TR-105.
  Exit gate: AT-R-101, AT-R-102, AT-R-103, AT-R-104, AT-R-201, AT-R-202.
- **Phase R2 — tmux gate correctness.**
  Env propagation from the wrapper; split refusal diagnostics; transient
  no-client tolerance; diagnose gate-input reporting; orphan-session cleanup.
  Tasks TR-201..TR-205.
  Exit gate: AT-R-203, AT-R-301, AT-R-302, AT-R-303, AT-R-304.
- **Phase R3 — Smoke and CI isolation.**
  Private-socket smoke scripts; CI job. Tasks TR-301..TR-303.
  Exit gate: AT-R-401, AT-R-402, AT-R-403.
- **Phase R4 — Hygiene and bounded investigation.**
  Remove debug scaffolding; RendererFailed reproduction. Tasks TR-401..TR-402.
  Exit gate: AT-R-501, AT-R-502.

Phases R1 and R3 are independent and may proceed in parallel. R2's TR-203
(transient tolerance) is the only task with meaningful design risk; it must
land behind its own commit and may be deferred without blocking the others.

## Non-goals

- No change to the render engines, the Kitty escape emitters, or the
  placement model.
- No new tmath subcommands and no change to the `agent-allowed` 0/1 contract.
- No auto-repair that deletes user files: the installer replaces only
  `$BIN_HOME/tmath`, and only with a warning when it was not a launcher.
