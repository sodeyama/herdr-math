# Runtime Reliability V1 Task Checklist

## Status

- Checklist state: **In progress — Phases R1 and R2 implemented; Phase R3 in
  progress (TR-301, TR-302 done)**
- Plan: `../plans/main.md`
- Acceptance contract: `../tests/main.md`
- Rules: one logical change per commit; a task is complete only when its listed
  acceptance tests pass with required evidence; spec/doc updates land as
  separate documentation commits immediately after the implementation commit.

Each task below is sized for one commit (≤ 300 changed source lines, 2–6
files) and lists the exact files, the change, and the validation commands.
Run the narrow validation during development and the full surface
(`cargo test --workspace && cargo clippy --all-targets` plus the named smoke
scripts) before checking a box.

## Phase R1 — Unbreak and detect

- [x] **TR-101** Atomic launcher install (AT-R-101).
      (commit `d79ad88`, with TR-102; smoke PASS via TR-103)
      File: `scripts/install.sh` (the "Launcher on PATH" section).
      Replace the `cat > "$BIN_HOME/tmath"` heredoc with: write the same
      heredoc to `"$BIN_HOME/.tmath.launcher.$$"`, `chmod +x` it, then
      `mv -f "$BIN_HOME/.tmath.launcher.$$" "$BIN_HOME/tmath"`. Keep the
      heredoc content byte-identical. Do not add new dependencies.
      Validate: `bash -n scripts/install.sh`; then TR-103's smoke script once
      it exists (write TR-103 first if working in spec order is inconvenient —
      the pair may land as two commits in either order).

- [x] **TR-102** Foreign-file warning before replacement (AT-R-102).
      (commit `d79ad88`, with TR-101 — one cohesive edit of the same block)
      File: `scripts/install.sh` (same section, before the write).
      If `$BIN_HOME/tmath` exists and `head -c 2` of it is not `#!`, print
      `tmath: replacing non-launcher file at $BIN_HOME/tmath` to stderr.
      Never print file contents or hashes. The install then proceeds
      normally (TR-101 already guarantees a fresh inode).
      Validate: covered by TR-103's smoke script.

- [x] **TR-103** Launcher install smoke test (AT-R-101, AT-R-102).
      (commit `347bb62`; PASS twice, deterministic)
      New file: `scripts/smoke-install-launcher.sh` (mirror the extraction
      style of `scripts/smoke-install-shell-integration.sh`: pull the
      launcher-install block out of `install.sh` with `awk` so the test
      exercises the real logic).
      Steps inside the test: create a temp dir as fake `BIN_HOME`; seed
      `tmath` there as a copy of `/bin/ls` (a real non-script executable);
      record its inode; run the extracted block with `APP` pointed at a temp
      app dir containing a stub `bin/tmath` script that prints `tmath 0.0.0`;
      assert (a) stderr contained the AT-R-102 warning, (b) the inode
      changed, (c) first line of the result is `#!/bin/sh`, (d) executing
      `"$BIN_HOME/tmath" --version` exits 0. Clean up with a trap.
      Add the script to `package.json` scripts as
      `smoke:install-launcher` next to the existing smoke entries.
      Validate: `bash scripts/smoke-install-launcher.sh` prints `PASS`.

- [x] **TR-104** `tmath diagnose` PATH-launcher check (AT-R-103, AT-R-104).
      (commit `8e6b3a5`; 3 unit tests, clippy clean, 221 crate tests green)
      Files: `engine/crates/tmath/src/main.rs` (diagnose section; if diagnose
      lives elsewhere, follow the `"diagnose"` match arm from `main.rs`).
      Add a function `path_launcher_report() -> (String, bool)` that:
      (1) resolves the first `tmath` entry in `PATH` (iterate
      `env::split_paths`, test file existence + executable bit; do NOT shell
      out to `which`); (2) if none, returns
      (`path launcher: not found on PATH`, false);
      (3) runs the resolved path with arg `--version` via
      `std::process::Command` with stdin null, capturing output, and a 5 s
      timeout (spawn + poll loop with `try_wait`, sleep 50 ms; kill on
      timeout); (4) on success parse `tmath <version>` and return
      (`path launcher: ok (tmath <version>)`, true); if the version differs
      from `env!("CARGO_PKG_VERSION")`, instead return the AT-R-104 skew line
      with ok=true; (5) on nonzero exit or signal death return
      (`path launcher: broken (exit <code>)`, false) using
      `status.code().unwrap_or(128 + signal)` semantics — on Unix use
      `std::os::unix::process::ExitStatusExt::signal()` so SIGKILL reports
      `exit 137`.
      Print the line with the other diagnose lines; make diagnose's process
      exit code nonzero when ok=false. Skip the check (print
      `path launcher: skipped (no PATH)`) when `PATH` is unset.
      Unit tests in the same file: fake PATH pointing at a temp dir with
      (a) a script that prints `tmath 9.9.9` (skew), (b) a script that
      `exit 7` (broken), (c) an empty dir (not found). Use `TMPDIR`-based
      dirs, no fixed paths.
      Validate: `cargo test -p tmath`, `cargo clippy --all-targets`,
      manual run: `target/debug/tmath diagnose`.

- [x] **TR-105** Wrapper failure visibility (AT-R-201, AT-R-202).
      (commit `01ff271`; allowlist smoke extended, PASS)
      File: `scripts/shell/tmath-agent.sh`.
      In `__tmath_wrap_agent`, replace the single
      `tmath agent-allowed >/dev/null 2>&1 || { ...pass through... }` line
      with: run `tmath agent-allowed >/dev/null 2>&1`; capture `$?` into
      `allowed_status`; `case` on it — `0` continue; `1` pass through
      silently; `*` print
      `tmath: agent-allowed failed (exit $allowed_status); run 'tmath diagnose'`
      to stderr, then pass through. Preserve `"$@"` exactly and return the
      wrapped command's status in all branches. Keep the function
      POSIX-compatible with both zsh and bash (no arrays, no local -n).
      Extend `scripts/smoke-agent-allowlist.sh`: add a case that puts a stub
      `tmath` (a two-line script: `#!/bin/sh` / `exit 137`) first on PATH,
      wraps a command that echoes its args, and asserts stdout is unchanged,
      stderr contains exactly one `agent-allowed failed (exit 137)` line, and
      the exit status is the wrapped command's.
      Validate: `bash scripts/smoke-agent-allowlist.sh` prints `PASS`.

- [x] **TR-106** Docs commit for Phase R1.
      Files: `docs/getting-started.md` (the Diagnose section README links as
      troubleshooting), `specs/runtime-reliability-v1/tasks/main.md`.
      Documented the symptom "tmath exits silently with code 137 on macOS"
      with the cause (in-place overwrite of the PATH entry) and the fix
      (re-run `scripts/install.sh`; never `cp` a binary over
      `~/.local/bin/tmath`). Checked off completed R1 tasks with hashes.

## Phase R2 — tmux gate correctness

- [x] **TR-201** Propagate transport env in the wrapper's new-session path
      (commit `29f791a`, with TR-205)
      (AT-R-301).
      File: `scripts/shell/tmath-agent.sh`
      (`__tmath_start_in_new_tmux_session` and `__tmath_start_watcher_for_pane`).
      Build an env prefix string: for each of `TMATH_TMUX_TRANSPORT`,
      `TMATH_DPR`, `TMATH_DEBUG_LOG` that is set and non-empty in the current
      shell, append `NAME=<quoted value>` using `__tmath_shell_quote_args`'s
      quoting (factor a `__tmath_quote_one` helper if needed). Prepend
      `env <pairs> ` to the watcher command passed to `tmux split-window`
      (`tmath agent --source-pane ...`). Leave the wrapped command itself
      untouched. The in-tmux background start (`__tmath_start_watcher_for_pane`)
      inherits the shell env already — do not change its invocation, only add
      a comment stating why.
      Validate: extend `scripts/smoke-agent-wrapper-tmux.sh` (see TR-302) or,
      before that lands, assert manually with
      `TMATH_TMUX_TRANSPORT=passthrough` that
      `tmux list-panes -F '#{pane_start_command}'` shows the `env` prefix.

- [x] **TR-202** Split the gate refusal into two diagnostics (AT-R-302).
      (commit `52f1677`; classifier unit tests cover all branches)
      File: `engine/crates/tmath/src/terminal_output.rs`.
      Refactor `known_outer_terminal()` to return an enum
      `OuterTerminal { Verified, Unverified(String), NoClient }`:
      `NoClient` when `client_termname` query returns `None`/empty AND
      `query_client_tty_path()` is `None`; `Unverified(name)` with the
      observed termname otherwise; `Verified` per the existing checks.
      In `selected_route()`, when the transport env is unset, map
      `NoClient` → error
      `tmux has no attached client; graphics need an attached Kitty-capable terminal`
      and `Unverified(name)` → error
      `tmux outer terminal '<name>' is not a verified Kitty target; set TMATH_TMUX_TRANSPORT=client-tty or passthrough to override`.
      Add a test seam: extract the tmux queries behind a small trait or a
      function pointer struct so unit tests can inject
      (termname, tty-path) pairs without running tmux; test all three
      classifications and both message strings.
      Privacy: the termname is an advertised terminal identifier, never
      document content; do not include tty paths in messages.
      Validate: `cargo test -p tmath`, `cargo clippy --all-targets`.

- [x] **TR-203** Transient no-client tolerance in the viewer (AT-R-303).
      (commit `e5984a4`; retry-budget decision helper unit-tested)
      Files: `engine/crates/tmath/src/agent_viewer.rs` (emission call sites
      of `write_operations`/`selected_route`), `terminal_output.rs` (expose
      the refusal cause from TR-202's enum to callers).
      Behavior: keep a `consecutive_route_failures: u32` counter in the
      viewer loop. When an emission fails with the `NoClient` cause and the
      viewer has previously emitted successfully at least once: log one
      bounded status line `route_unavailable clients=0` (counts only),
      skip the emission (do not tear down placements), reset the counter on
      the next success, and exit with the existing error path only when the
      counter reaches 30. All other refusal causes keep today's fail-closed
      behavior. Unit-test the counter as a pure function
      (`should_exit(cause, previously_ok, count) -> bool`); integration
      coverage comes from TR-301's headless smoke.
      Validate: `cargo test -p tmath`.

- [x] **TR-204** Diagnose reports gate inputs (AT-R-304).
      (commit `5464361`)
      File: `engine/crates/tmath/src/terminal_output.rs`
      (`tmux_diagnostics()`).
      Add lines: `tmux attached clients: <n>` (from
      `tmux list-clients | wc -l` equivalent via `tmux_value` on
      `#{client_termname}` list — use `tmux list-clients -F x` and count
      lines), and change the existing route line to print either the route
      label or the full refusal message from TR-202. Keep every line free of
      paths and document content.
      Validate: `cargo test -p tmath`; manual `tmath diagnose` inside and
      outside tmux.

- [x] **TR-205** No orphan wrapper sessions (AT-R-203).
      (commit `29f791a`; watcher already exited cleanly on a closed source pane — no Rust change needed)
      Files: `scripts/shell/tmath-agent.sh`
      (`__tmath_start_in_new_tmux_session`),
      `engine/crates/tmath/src/agent_watcher.rs` (source-pane liveness).
      Change the watcher pane command from
      `"tmath agent --source-pane $agent_pane; exec \$SHELL"` to the watcher
      command alone (with TR-201's env prefix), so the pane closes when the
      watcher exits. Confirm (and add a test if missing) that the watcher
      exits cleanly when its source pane disappears — `agent_watcher.rs`
      already polls the pane; make a failed poll because the pane is gone a
      clean exit (status 0, one bounded `source pane closed` line) rather
      than an error loop. With both panes gone, tmux destroys the session.
      Extend the headless smoke (TR-301) to assert the private session is
      gone within 5 s after the fake agent exits.
      Validate: `cargo test -p tmath`, smoke script.

- [x] **TR-206** Docs commit for Phase R2: updated `docs/architecture.md`
      transport-gate section (decision inputs, messages, retry budget) and
      checked off R2 tasks.

## Phase R3 — Smoke and CI isolation

- [x] **TR-301** Private-socket rewrite of `scripts/smoke-agent-tmux.sh`
      (AT-R-401).
      (commit `3c5dd07`; PASS 3x consecutive, default tmux server session
      list unchanged each run)
      Define `SOCKET="tmath-smoke-$$"` and a helper `tm() { tmux -L "$SOCKET" "$@"; }`;
      replace every `tmux` call with `tm`; add
      `trap 'tm kill-server 2>/dev/null || true; ...' EXIT`.
      Record `tmux ls` (default server, `|| true`) into a variable before and
      after; assert equality. Embed the transport in the watcher command:
      `WATCHER_CMD="TMATH_TMUX_TRANSPORT=passthrough $TMATH agent ..."` and
      keep `allow-passthrough on` (already present). Update the header
      comment: the script proves the watcher→socket→viewer pipeline headless
      with an explicit transport assertion, matching the gate added in
      commit `ad579b9`.
      Validate: run twice in a row while a personal tmux server is up:
      both runs print `PASS`, `tmux ls` output unchanged.
      Fixed two pre-existing failures the private-socket run exposed (both
      present, latent, before this task): the source pane's default cwd
      (this checkout) let `tmath agent` pick up this session's own live
      Claude Code transcript instead of the synthetic pane content, and
      zsh's `PROMPT_EOL_MARK` plus a redundant trailing prompt printf broke
      the exact-match idle-prompt check in
      `tmath-core::agent::boundary::is_idle_prompt_line`, so `find_answer()`
      never proved a boundary. See the commit message for detail.

- [x] **TR-302** Private-socket + new assertions for
      `scripts/smoke-agent-wrapper-tmux.sh` (AT-R-402, closes the loop on
      AT-R-201/202/301).
      (commit `4cca50e`; PASS 3x consecutive, default tmux server session
      list unchanged each run)
      Apply the same `tm()`/trap isolation. Add two cases:
      (a) broken-stub tmath (exit 137) → wrapper warning line + passthrough
      (may reuse the allowlist smoke's stub approach if simpler — then this
      case lives in `smoke-agent-allowlist.sh` and this task only asserts
      isolation); (b) with `TMATH_TMUX_TRANSPORT=passthrough` exported,
      assert the watcher pane's start command contains
      `TMATH_TMUX_TRANSPORT=` (via `tm list-panes -F '#{pane_start_command}'`).
      Validate: script prints `PASS` with a personal tmux server running.
      (a) confirmed already covered by `smoke-agent-allowlist.sh`'s
      AT-R-201/202 stub cases; this script asserts isolation plus (b), the
      latter by running `__tmath_start_in_new_tmux_session`'s own
      `new-session`/`split-window` sequence directly (stopping short of its
      blocking `tmux attach`, which would otherwise fight this test's own
      outer tmux client) rather than invoking the function unmodified.
      Also widened the `watching %`/`not a verified Kitty target` log-match
      pair used to prove a watcher attempt to include TR-202's third
      refusal message, `no attached client` — the one this detached,
      no-client smoke session actually hits.

- [x] **TR-303** CI job for the tmux smokes (AT-R-403). Implemented in
      42a3fe0; CI-green confirmation is still pending on the PR (see below).
      File: `.github/workflows/ci.yml`. All existing jobs
      (`macos-arm64`, `rust-gates`) run on `runs-on: macos-14`, not Linux, so
      the added `tmux-smokes` job also runs on `macos-14` and installs tmux
      with `brew install tmux` instead of the `sudo apt-get install -y tmux`
      originally sketched here for a Linux runner; zsh ships preinstalled on
      the macOS runner image, so no separate zsh install step was needed.
      Steps: checkout, Node setup, `npm ci`, `npm run build`,
      `cargo build --workspace`, then run `scripts/smoke-agent-tmux.sh`,
      `scripts/smoke-agent-wrapper-tmux.sh`, `scripts/smoke-agent-allowlist.sh`,
      `scripts/smoke-install-launcher.sh` as separate steps.
      The workflow has no summary-gating pattern to join, so the job
      participates via the default required-status-checks surface.
      Validate: local run confirmed `scripts/smoke-agent-allowlist.sh` and
      `scripts/smoke-install-launcher.sh` pass end-to-end; the two tmux-driving
      scripts (`smoke-agent-tmux.sh`, `smoke-agent-wrapper-tmux.sh`) were
      checked with `bash -n` only, since this session's sandbox denies direct
      tmux operations — their full pass/fail run happens on CI. CI green on
      the PR is still outstanding and must be confirmed before treating this
      task as fully closed.

## Phase R4 — Hygiene and bounded investigation

- [x] **TR-401** Remove committed debug scaffolding (AT-R-501). (1adaeb5)
      Files: `engine/crates/tmath/src/terminal_output.rs` and any other file
      matched by `grep -rln "region agent log" engine/`.
      Delete every `// #region agent log ... // #endregion` block and the
      `debug_log` / `debug_log_current` helpers with hardcoded
      `sessionId`/`runId` literals. If any deleted call site carried
      diagnostics still worth keeping (route selection, client-tty
      validation), re-express them as bounded lines through the existing
      `TMATH_DEBUG_LOG`-gated `write_debug_line` with static event names and
      counts only — or drop them. No behavior change otherwise.
      Validate: `grep -rn "region agent log" engine/` empty;
      `cargo test --workspace`; `cargo clippy --all-targets`;
      `cargo fmt --check`.

- [ ] **TR-402** Bounded investigation: viewer `RendererFailed` on
      sync/status-bar (AT-R-502).
      Field evidence (2026-08-05): after ~650 placed blocks the viewer logged
      `sync_failed (RendererFailed)` and `status_bar_failed (RendererFailed)`
      and exited. Budget: reproduce via an integration test that drives the
      viewer sync path with a render engine forced to fail (inject via the
      existing engine seam in `agent_viewer.rs`; add a narrow test hook if
      none exists). Assert: the stable error code surfaces once, earlier
      placements survive, and the next successful document renders. If the
      field failure does not reproduce after the render-failure injection
      plus a cache-exhaustion attempt (drive > 1000 blocks through the viewer
      in the fake-tty harness), write
      `docs/evidence/<date>-viewer-rendererfailed-investigation.md` recording
      the attempts and close without a code change.
      Validate: `cargo test --workspace` green either way.

- [ ] **TR-403** Docs commit closing the spec: mark checklist state, update
      `README.md` troubleshooting pointer if TR-106 placed it elsewhere, and
      record final evidence links.
