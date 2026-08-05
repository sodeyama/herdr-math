#!/usr/bin/env bash
#
# smoke-agent-tmux: headless pipeline test for `tmath agent`.
#
# Runs inside a detached tmux session (no Kitty-capable outer client), proving
# the watcher -> socket -> viewer pipeline: the watcher detects the newest
# answer and emits the document. Real-image placement is recorded separately
# in a Ghostty-attached session (see docs/evidence).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH="$ROOT/target/debug/tmath"

# --- build prerequisites -----------------------------------------------------
if [ ! -x "$TMATH" ]; then
  (cd "$ROOT" && cargo build --workspace >/dev/null)
fi
if [ ! -f "$ROOT/dist/renderer/subprocess.js" ]; then
  (cd "$ROOT" && npm run build >/dev/null)
fi

export TMATH_RENDER_WORKER="$ROOT/dist/renderer/subprocess.js"
SESSION="tmath-smoke-$$"

tmux new-session -d -s "$SESSION" -x 100 -y 30 'zsh -f' >/dev/null
tmux set-option -t "$SESSION" -w allow-passthrough on >/dev/null

# The watched pane (source): a fake coding agent with prompt + answer.
SRC_PANE="$(tmux display-message -p -t "$SESSION" '#{pane_id}')"
tmux send-keys -l -t "$SRC_PANE" 'PS1="❯ " ; printf '\''❯ Compute the integral.\n'\'''
tmux send-keys -t "$SRC_PANE" Enter
sleep 0.3

# A control pane that runs the watcher, logging bounded status to a file.
tmux split-window -h -p 30 -t "$SESSION" 'zsh -f' >/dev/null
CTL_PANE="$(tmux display-message -p -t "$SESSION" '#{pane_id}')"
LOG="$(mktemp -t tmath-agent-smoke.XXXXXX)"
WATCHER_CMD="$TMATH agent --source-pane $SRC_PANE --wait-ms 300 --poll-ms 100 --percent 30 2> $LOG"
tmux send-keys -l -t "$CTL_PANE" "$WATCHER_CMD"
tmux send-keys -t "$CTL_PANE" Enter

# --- wait for watcher startup ------------------------------------------------
for i in $(seq 1 40); do
  if [ -f "$LOG" ] && grep -q 'watching' "$LOG" 2>/dev/null; then
    ok=1; break
  fi
  sleep 0.25
done
if [ "${ok:-0}" != 1 ]; then
  echo "FAIL: watcher did not start"; cat "$LOG" 2>/dev/null || true
  tmux kill-session -t "$SESSION" 2>/dev/null || true; exit 1
fi

# --- emit a fake agent answer ------------------------------------------------
tmux send-keys -l -t "$SRC_PANE" 'printf '\''The relation is $E=mc^2$.\n'\'' ; printf '\''❯ '\'''
tmux send-keys -t "$SRC_PANE" Enter
sleep 1.5

# --- assertions ----------------------------------------------------------------
pass=1
echo "source pane:"
tmux capture-pane -p -t "$SRC_PANE" | sed 's/^/  /'

echo "watcher log ($LOG):"
cat "$LOG" | sed 's/^/  /' || true

# The watcher must detect the new answer and attempt to send it. In a detached
# (no Kitty-capable) session the viewer may exit after route or graphics
# validation fails, so the watcher logs `document_sent` or a disconnected-viewer
# record; either proves the boundary detection and emission path fired.
if ! grep -qE 'document_sent|viewer disconnected|no viewer connected' "$LOG"; then
  echo "FAIL: watcher never detected and emitted the answer"
  pass=0
fi

VIEWER_PANE="$(grep -o 'watching %[0-9]* → %[0-9]*' "$LOG" | sed 's/.*→ //' | head -1)"
if [ -z "$VIEWER_PANE" ]; then
  echo "FAIL: watcher did not report a created viewer pane"
  pass=0
fi
echo "viewer pane: $VIEWER_PANE"

# The viewer pane in a detached (no client) session may report a graphics or
# cell-size diagnostic, or be reaped by tmux after a clean exit; the
# watcher-side emit is the mandatory assertion here. Real-image placement is
# recorded separately in a Ghostty-attached session.
VIEWER_CAP="$(tmux capture-pane -p -S -100 -t "$VIEWER_PANE" 2>/dev/null || true)"
if printf '%s' "$VIEWER_CAP" | grep -qE 'no Kitty graphics support|no usable cell size|placed image='; then
  printf '%s\n' "$VIEWER_CAP" | grep -v '^$' | tail -3 | sed 's/^/viewer: /'
else
  echo "viewer pane: already reaped by tmux (clean exit); image path recorded in Ghostty evidence"
fi

# --- cleanup ------------------------------------------------------------------
tmux kill-session -t "$SESSION" 2>/dev/null || true
rm -f "$LOG"

if [ "$pass" = 1 ]; then
  echo "PASS: watcher emitted the answer document; real-image placement recorded in Ghostty evidence."
  exit 0
fi
exit 1
