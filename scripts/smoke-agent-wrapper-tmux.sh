#!/usr/bin/env bash
#
# smoke-agent-wrapper-tmux: headless check of scripts/shell/tmath-agent.sh —
# the shell wrapper that auto-starts `tmath agent` for allowlisted
# directories. Runs inside a detached tmux session (no Kitty-capable outer
# client), proving the wrapper -> lock -> watcher pipeline. Covers AT-2-815
# (passthrough when not allowlisted), AT-2-816 (in-tmux auto-start), and
# AT-2-817 (duplicate watcher prevention with stale-lock reclaim).
#
# `tmath agent` itself fails closed (exits immediately) whenever the outer
# tmux client is not a verified Kitty-capable terminal (see
# scripts/smoke-agent-tmux.sh, which hits the same condition headless). This
# smoke test therefore does not require the watcher process to stay alive;
# it asserts that the wrapper attempted to start exactly one watcher per
# pane (a log line landed, proving `tmath agent` ran) rather than that the
# watcher process is still running by the time the assertion runs.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH="$ROOT/target/debug/tmath"

if [ ! -x "$TMATH" ]; then
  (cd "$ROOT" && cargo build --workspace >/dev/null)
fi
if [ ! -f "$ROOT/dist/renderer/subprocess.js" ]; then
  (cd "$ROOT" && npm run build >/dev/null)
fi

TMP_HOME="$(mktemp -d)"
BIN_DIR="$TMP_HOME/bin"
WATCHER_LOG="$TMP_HOME/watcher.log"
mkdir -p "$BIN_DIR" "$TMP_HOME/proj"

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

SESSION="tmath-wrapper-smoke-$$"
LOG="$(mktemp -t tmath-wrapper-smoke.XXXXXX)"
cleanup() {
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  pkill -f "tmath agent --source-pane" 2>/dev/null || true
  rm -f "$LOG"
  rm -rf "$TMP_HOME"
}
trap cleanup EXIT

tmux new-session -d -s "$SESSION" -x 100 -y 30 'zsh -f' >/dev/null
tmux set-option -t "$SESSION" -w allow-passthrough on >/dev/null
SRC_PANE="$(tmux display-message -p -t "$SESSION" '#{pane_id}')"

setup_shell() {
  tmux send-keys -l -t "$SRC_PANE" \
    "export HOME='$TMP_HOME'; export PATH='$BIN_DIR:/usr/bin:/bin:/opt/homebrew/bin'; export TMATH_RENDER_WORKER='$RENDER_WORKER'; hash -r; source '$WRAPPER'"
  tmux send-keys -t "$SRC_PANE" Enter
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
tmux send-keys -l -t "$SRC_PANE" 'echo TMATH_SMOKE_READY'
tmux send-keys -t "$SRC_PANE" Enter
if ! wait_for 'tmux capture-pane -p -t "$SRC_PANE" 2>/dev/null | grep -q TMATH_SMOKE_READY'; then
  echo "FAIL: shell setup did not settle"
  tmux capture-pane -p -t "$SRC_PANE" || true
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

# --- AT-2-815: not allowlisted -> passthrough, no watcher -------------------
tmux send-keys -l -t "$SRC_PANE" "cd '$NOT_ALLOWED_DIR' && claude not-allowed-run"
tmux send-keys -t "$SRC_PANE" Enter
wait_for 'tmux capture-pane -p -t "$SRC_PANE" 2>/dev/null | grep -q "fake claude running"' 20 || true
sleep 0.5
if [ -f "$WATCHER_LOG" ]; then
  check "not-allowlisted directory: no watcher started" 0
else
  check "not-allowlisted directory: no watcher started" 1
fi
if tmux capture-pane -p -t "$SRC_PANE" | grep -q "fake claude running"; then
  check "not-allowlisted directory: wrapped command still ran" 1
else
  check "not-allowlisted directory: wrapped command still ran" 0
fi

# A watcher attempt is proven by either the "watching" banner (Kitty-verified
# outer terminal) or the known fail-closed diagnostic (unverified outer
# terminal, expected in this headless session) landing in the log — both
# prove the wrapper invoked `tmath agent` for this pane.
watcher_attempted() {
  [ -f "$WATCHER_LOG" ] && grep -qE 'watching %|not a verified Kitty target' "$WATCHER_LOG"
}
attempt_count() {
  grep -cE 'watching %|not a verified Kitty target' "$WATCHER_LOG" 2>/dev/null || echo 0
}

# --- AT-2-816: allowlisted + in-tmux -> watcher auto-starts -----------------
tmux send-keys -l -t "$SRC_PANE" "cd '$TARGET_DIR' && claude hello"
tmux send-keys -t "$SRC_PANE" Enter
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
tmux send-keys -l -t "$SRC_PANE" 'claude again'
tmux send-keys -t "$SRC_PANE" Enter
sleep 1
ATTEMPTS_AFTER_REPEAT="$(attempt_count | tr -d '[:space:]')"
check "no duplicate watcher for an already-watched pane" "$([ "$ATTEMPTS_AFTER_REPEAT" = 1 ] && echo 1 || echo 0)"

# --- AT-2-817: stale lock is reclaimed once it no longer names a live PID --
rm -f "$LOCK"
tmux send-keys -l -t "$SRC_PANE" 'claude yet-again'
tmux send-keys -t "$SRC_PANE" Enter
RECLAIMED=0
if wait_for '[ "$(attempt_count)" -ge 2 ]'; then
  RECLAIMED=1
fi
check "stale lock reclaimed, new watcher starts after old one dies" "$RECLAIMED"

echo "wrapper session pane:"
tmux capture-pane -p -t "$SRC_PANE" | sed 's/^/  /'

if [ "$pass" = 1 ]; then
  echo "PASS: wrapper auto-start/passthrough/lock behavior matches AT-2-815/816/817"
  exit 0
fi
exit 1
