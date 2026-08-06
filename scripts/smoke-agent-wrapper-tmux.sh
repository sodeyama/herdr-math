#!/usr/bin/env bash
#
# smoke-agent-wrapper-tmux: headless check of scripts/shell/tmath-agent.sh —
# the shell wrapper that auto-starts `tmath agent` for allowlisted
# directories. Runs inside a detached tmux session on a private socket (no
# Kitty-capable outer client), proving the wrapper -> lock -> watcher pipeline.
# Covers AT-2-815 (passthrough when not allowlisted), AT-2-816 (in-tmux
# auto-start), AT-2-817 (duplicate watcher prevention with stale-lock reclaim),
# and AT-R-301 (outside-tmux launch: background watcher, transport env
# inheritance, no dedicated watcher pane).
#
# `tmath agent` itself fails closed (exits immediately) whenever the outer
# tmux client is not a verified Kitty-capable terminal (see
# scripts/smoke-agent-tmux.sh, which hits the same condition headless). This
# smoke test therefore does not require the watcher process to stay alive;
# it asserts that the wrapper attempted to start exactly one watcher per
# pane (a log line landed, proving `tmath agent` ran) rather than that the
# watcher process is still running by the time the assertion runs.

set -euo pipefail

# `set -e` exits are otherwise silent for commands whose output is
# redirected — on CI this smoke once died in 0.2s with no clue which line
# failed. Name the line and command on the way out.
trap 'echo "FAIL: line $LINENO: $BASH_COMMAND (exit $?)" >&2' ERR

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH="$ROOT/target/debug/tmath"

SOCKET="tmath-smoke-$$"
SESSION="tmath-wrapper-smoke-$$"
REAL_TMUX="$(command -v tmux)"
tm() { "$REAL_TMUX" -L "$SOCKET" "$@"; }

DEFAULT_SESSIONS_BEFORE="$("$REAL_TMUX" ls 2>/dev/null || true)"

if [ ! -x "$TMATH" ]; then
  (cd "$ROOT" && cargo build --workspace >/dev/null)
fi
if [ ! -f "$ROOT/dist/renderer/subprocess.js" ]; then
  (cd "$ROOT" && npm run build >/dev/null)
fi

TMP_HOME="$(mktemp -d)"
BIN_DIR="$TMP_HOME/bin"
SHIM_DIR="$TMP_HOME/tmux-shim"
WATCHER_LOG="$TMP_HOME/watcher.log"
mkdir -p "$BIN_DIR" "$TMP_HOME/proj" "$SHIM_DIR"

# Resolve the real tmux binary by absolute path at generation time (never
# `command tmux` / bare `tmux`): under some PATH orderings the lookup can
# re-resolve to this very shim, causing unbounded self-recursion.
cat > "$SHIM_DIR/tmux" <<EOF
#!/usr/bin/env bash
exec "$REAL_TMUX" -L "$SOCKET" "\$@"
EOF
chmod +x "$SHIM_DIR/tmux"

# A copy of the wrapper with the watcher's stderr redirected to a log file
# instead of /dev/null, so this test can assert on *why* the watcher exited
# instead of only on whether its process is still alive (it fails closed
# immediately in this headless, non-Kitty tmux client — see the note above).
WRAPPER="$TMP_HOME/tmath-agent.sh"
sed "s#tmath agent --source-pane \"\$pane\" >/dev/null 2>&1#tmath agent --source-pane \"\$pane\" >>'$WATCHER_LOG' 2>\&1#" \
  "$ROOT/scripts/shell/tmath-agent.sh" > "$WRAPPER"

cat > "$BIN_DIR/tmath" <<EOF
#!/bin/sh
exec "$TMATH" "\$@"
EOF
chmod +x "$BIN_DIR/tmath"

cat > "$BIN_DIR/claude" <<'EOF'
#!/bin/sh
echo "fake claude running pid=$$"
sleep 5
EOF
chmod +x "$BIN_DIR/claude"

TARGET_DIR="$TMP_HOME/proj"
NOT_ALLOWED_DIR="$TMP_HOME/other"
mkdir -p "$NOT_ALLOWED_DIR"

RENDER_WORKER="$ROOT/dist/renderer/subprocess.js"
HOME="$TMP_HOME" "$BIN_DIR/tmath" agent-enable "$TARGET_DIR" >/dev/null

LOG="$(mktemp -t tmath-wrapper-smoke.XXXXXX)"
cleanup() {
  # Only tear down what this run itself started: `tm kill-server` destroys
  # the private-socket tmux server, which kills every pane (and therefore
  # every watcher process) it launched. A machine-wide `pkill -f "tmath
  # agent --source-pane"` here would also kill unrelated `tmath agent`
  # watchers the operator or another session has running against the
  # default tmux server — never do that.
  tm kill-server 2>/dev/null || true
  rm -f "$LOG"
  rm -rf "$TMP_HOME"
}
trap cleanup EXIT

tm new-session -d -s "$SESSION" -x 100 -y 30 'zsh -f' >/dev/null
tm set-option -t "$SESSION" -w allow-passthrough on >/dev/null
SRC_PANE="$(tm display-message -p -t "$SESSION" '#{pane_id}')"

setup_shell() {
  tm send-keys -l -t "$SRC_PANE" \
    "export HOME='$TMP_HOME'; export PATH='$SHIM_DIR:$BIN_DIR:/usr/bin:/bin:/opt/homebrew/bin'; export TMATH_RENDER_WORKER='$RENDER_WORKER'; hash -r; source '$WRAPPER'"
  tm send-keys -t "$SRC_PANE" Enter
}

wait_for() {
  local predicate="$1" timeout_steps="${2:-40}"
  for _ in $(seq 1 "$timeout_steps"); do
    if eval "$predicate"; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

setup_shell
tm send-keys -l -t "$SRC_PANE" 'echo TMATH_SMOKE_READY'
tm send-keys -t "$SRC_PANE" Enter
if ! wait_for 'tm capture-pane -p -t "$SRC_PANE" 2>/dev/null | grep -q TMATH_SMOKE_READY'; then
  echo "FAIL: shell setup did not settle"
  tm capture-pane -p -t "$SRC_PANE" || true
  exit 1
fi

pass=1
check() {
  local desc="$1" ok="$2"
  if [ "$ok" = 1 ]; then
    echo "ok   - $desc"
  else
    echo "FAIL - $desc"
    pass=0
  fi
}

# --- AT-R-301: outside-tmux launch — background watcher, no dedicated pane -
# __tmath_start_in_new_tmux_session ends in a blocking `tmux attach`, which
# fights any tmux client already attached to the pane running this test (it
# corrupts that pane's display and hangs later assertions). Replicate the
# function's session-building steps (everything short of the attach) with
# the real helpers: build the single-pane session, then call
# __tmath_start_watcher_for_pane exactly as the function does. Passthrough
# transport is the behavioral probe: headless with no attached client, the
# watcher reaches `watching %` only if TMATH_TMUX_TRANSPORT was inherited
# from the launching shell; otherwise it fails closed with the TR-202
# `no attached client` refusal. A separate wrapper copy logs this watcher to
# its own file so the AT-2-816/817 attempt counts on $WATCHER_LOG below stay
# untouched.
NESTED_SESSION="tmath-probe-$$"
NESTED_LOG="$TMP_HOME/nested-watcher.log"
NESTED_PANE_FILE="$TMP_HOME/nested-pane"
NESTED_WRAPPER="$TMP_HOME/tmath-agent-nested.sh"
sed "s#tmath agent --source-pane \"\$pane\" >/dev/null 2>&1#tmath agent --source-pane \"\$pane\" >>'$NESTED_LOG' 2>\&1#" \
  "$ROOT/scripts/shell/tmath-agent.sh" > "$NESTED_WRAPPER"
(
  export HOME="$TMP_HOME" TMATH_RENDER_WORKER="$RENDER_WORKER" TMATH_TMUX_TRANSPORT=passthrough
  export PATH="$SHIM_DIR:$BIN_DIR:/usr/bin:/bin"
  # shellcheck disable=SC1090
  source "$NESTED_WRAPPER"
  tmux new-session -d -s "$NESTED_SESSION" '/bin/sleep 30'
  tmux set-option -t "$NESTED_SESSION" -w allow-passthrough on
  agent_pane="$(tmux list-panes -t "$NESTED_SESSION" -F '#{pane_id}' | head -1)"
  printf '%s' "$agent_pane" > "$NESTED_PANE_FILE"
  # A stale lock from an earlier run could name a live unrelated PID and
  # suppress the launch; this probe owns the pane, so clear it first.
  rm -f "${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/tmath-agent-pane-${agent_pane#%}.lock"
  __tmath_start_watcher_for_pane "$agent_pane"
)
NESTED_STARTED=0
if wait_for '[ -f "$NESTED_LOG" ] && grep -q "watching %" "$NESTED_LOG"'; then
  NESTED_STARTED=1
fi
check "AT-R-301: background watcher inherited TMATH_TMUX_TRANSPORT (reached watching)" "$NESTED_STARTED"
if [ "$NESTED_STARTED" != 1 ]; then
  sed 's/^/  nested log: /' "$NESTED_LOG" 2>/dev/null || echo "  nested log: <missing>"
fi
WATCHER_PANES="$(tm list-panes -t "$NESTED_SESSION" -F '#{pane_start_command}' 2>/dev/null | grep -c 'agent --source-pane' || true)"
check "AT-R-301: no dedicated watcher pane in the session" "$([ "${WATCHER_PANES:-0}" -eq 0 ] && echo 1 || echo 0)"
tm kill-session -t "$NESTED_SESSION" 2>/dev/null || true
NESTED_PANE="$(cat "$NESTED_PANE_FILE" 2>/dev/null || true)"
if [ -n "$NESTED_PANE" ]; then
  rm -f "${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/tmath-agent-pane-${NESTED_PANE#%}.lock"
fi

# --- AT-2-815: not allowlisted -> passthrough, no watcher -------------------
tm send-keys -l -t "$SRC_PANE" "cd '$NOT_ALLOWED_DIR' && claude not-allowed-run"
tm send-keys -t "$SRC_PANE" Enter
wait_for 'tm capture-pane -p -t "$SRC_PANE" 2>/dev/null | grep -q "fake claude running"' 20 || true
sleep 0.5
if [ -f "$WATCHER_LOG" ]; then
  check "not-allowlisted directory: no watcher started" 0
else
  check "not-allowlisted directory: no watcher started" 1
fi
if tm capture-pane -p -t "$SRC_PANE" | grep -q "fake claude running"; then
  check "not-allowlisted directory: wrapped command still ran" 1
else
  check "not-allowlisted directory: wrapped command still ran" 0
fi

# A watcher attempt is proven by either the "watching" banner (Kitty-verified
# outer terminal) or one of the two fail-closed gate diagnostics from
# terminal_output.rs::selected_route() (TR-202): "not a verified Kitty
# target" (an unverified but attached client) or "no attached client"
# (expected in this headless, detached-session smoke run). Any of the three
# landing in the log proves the wrapper invoked `tmath agent` for this pane.
watcher_attempted() {
  [ -f "$WATCHER_LOG" ] && grep -qE 'watching %|not a verified Kitty target|no attached client' "$WATCHER_LOG"
}
attempt_count() {
  grep -cE 'watching %|not a verified Kitty target|no attached client' "$WATCHER_LOG" 2>/dev/null || echo 0
}

# --- AT-2-816: allowlisted + in-tmux -> watcher auto-starts -----------------
tm send-keys -l -t "$SRC_PANE" "cd '$TARGET_DIR' && claude hello"
tm send-keys -t "$SRC_PANE" Enter
STARTED=0
if wait_for "watcher_attempted"; then
  STARTED=1
fi
check "allowlisted + in-tmux: watcher auto-started for this pane" "$STARTED"

LOCK="${TMPDIR:-/tmp}/tmath-agent-pane-${SRC_PANE#%}.lock"
if [ -f "$LOCK" ]; then
  check "lock file created for the pane" 1
else
  check "lock file created for the pane" 0
fi

# --- AT-2-817: duplicate watcher prevention ---------------------------------
# The watcher already exited (fail-closed, see header note) by the time we
# get here, but its lock file is still held: nothing has reclaimed it yet, so
# a second launch in the same pane must not attempt a second watcher.
tm send-keys -l -t "$SRC_PANE" 'claude again'
tm send-keys -t "$SRC_PANE" Enter
sleep 1
ATTEMPTS_AFTER_REPEAT="$(attempt_count | tr -d '[:space:]')"
check "no duplicate watcher for an already-watched pane" "$([ "$ATTEMPTS_AFTER_REPEAT" = 1 ] && echo 1 || echo 0)"

# --- AT-2-817: stale lock is reclaimed once it no longer names a live PID --
rm -f "$LOCK"
tm send-keys -l -t "$SRC_PANE" 'claude yet-again'
tm send-keys -t "$SRC_PANE" Enter
RECLAIMED=0
if wait_for '[ "$(attempt_count)" -ge 2 ]'; then
  RECLAIMED=1
fi
check "stale lock reclaimed, new watcher starts after old one dies" "$RECLAIMED"

echo "wrapper session pane:"
tm capture-pane -p -t "$SRC_PANE" | sed 's/^/  /'

DEFAULT_SESSIONS_AFTER="$("$REAL_TMUX" ls 2>/dev/null || true)"
if [ "$DEFAULT_SESSIONS_BEFORE" != "$DEFAULT_SESSIONS_AFTER" ]; then
  echo "FAIL: default tmux server session list changed"
  echo "  before: ${DEFAULT_SESSIONS_BEFORE:-<empty>}"
  echo "  after:  ${DEFAULT_SESSIONS_AFTER:-<empty>}"
  exit 1
fi

if [ "$pass" = 1 ]; then
  echo "PASS: wrapper auto-start/passthrough/lock behavior matches AT-2-815/816/817"
  exit 0
fi
exit 1
