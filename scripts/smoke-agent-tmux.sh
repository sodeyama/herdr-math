#!/usr/bin/env bash
#
# smoke-agent-tmux: headless pipeline test for `tmath agent`.
#
# Runs inside a detached tmux session on a private socket (no Kitty-capable
# outer client), proving the watcher -> socket -> viewer pipeline with an
# explicit TMATH_TMUX_TRANSPORT=passthrough on the watcher command line —
# matching the fail-closed outer-terminal gate (runtime-reliability-v1).
# Real-image placement is recorded separately in a Ghostty-attached session
# (see docs/evidence).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH="$ROOT/target/debug/tmath"

SOCKET="tmath-smoke-$$"
SESSION="$SOCKET"
tm() { tmux -L "$SOCKET" "$@"; }

DEFAULT_SESSIONS_BEFORE="$(tmux ls 2>/dev/null || true)"

# --- build prerequisites -----------------------------------------------------
if [ ! -x "$TMATH" ]; then
  (cd "$ROOT" && cargo build --workspace >/dev/null)
fi
LOG=""

# A scratch cwd for the source pane, outside this repository. `tmath agent`
# prefers a live Claude Code transcript over tmux capture-pane when one
# exists for the pane's cwd (see transcript_adapter.rs::project_transcript_dir,
# `~/.claude/projects/<cwd-as-slug>`); starting the pane inside this
# checkout would let it pick up this very session's own live transcript
# instead of the synthetic capture-pane content this smoke test emits below.
SCRATCH_DIR="$(mktemp -d -t tmath-agent-smoke-cwd.XXXXXX)"

cleanup() {
  tm kill-server 2>/dev/null || true
  rm -f "${LOG:-}"
  rm -rf "${SCRATCH_DIR:-}"
}
trap cleanup EXIT

tm new-session -d -s "$SESSION" -c "$SCRATCH_DIR" -x 100 -y 30 'zsh -f' >/dev/null
tm set-option -t "$SESSION" -w allow-passthrough on >/dev/null

# The watched pane (source): a fake coding agent with prompt + answer.
# PROMPT_EOL_MARK="" turns off zsh's reverse-video "%" it otherwise inserts
# whenever a command's output doesn't end in a newline (both printf calls
# below end with the prompt glyph and no trailing newline). Left on, that
# "%" lands on the same captured line as the idle prompt (e.g. "❯ %"),
# which fails the exact-match idle-prompt check in
# tmath-core::agent::boundary::is_idle_prompt_line and makes find_answer()
# report no boundary.
SRC_PANE="$(tm display-message -p -t "$SESSION" '#{pane_id}')"
tm send-keys -l -t "$SRC_PANE" 'PROMPT_EOL_MARK="" ; PS1="❯ " ; printf '\''❯ Compute the integral.\n'\'''
tm send-keys -t "$SRC_PANE" Enter
sleep 0.3

# A control pane that runs the watcher, logging bounded status to a file.
tm split-window -h -p 30 -t "$SESSION" 'zsh -f' >/dev/null
CTL_PANE="$(tm display-message -p -t "$SESSION" '#{pane_id}')"
LOG="$(mktemp -t tmath-agent-smoke.XXXXXX)"
WATCHER_CMD="TMATH_TMUX_TRANSPORT=passthrough $TMATH agent --source-pane $SRC_PANE --wait-ms 300 --poll-ms 100 --percent 30 2> $LOG"
tm send-keys -l -t "$CTL_PANE" "$WATCHER_CMD"
tm send-keys -t "$CTL_PANE" Enter

# --- wait for watcher startup ------------------------------------------------
for i in $(seq 1 40); do
  if [ -f "$LOG" ] && grep -q 'watching' "$LOG" 2>/dev/null; then
    ok=1; break
  fi
  sleep 0.25
done
if [ "${ok:-0}" != 1 ]; then
  echo "FAIL: watcher did not start"; cat "$LOG" 2>/dev/null || true
  exit 1
fi

# --- emit a fake agent answer ------------------------------------------------
# Emit only the answer text and let the shell's own next-prompt redraw supply
# the single trailing idle "❯ " (PS1, set above). An explicit trailing
# `printf '❯ '` here would draw a *second*, separate "❯" line right before
# the shell's own prompt; tmath-core::agent::boundary::prompt_delimited_answer
# takes the rightmost idle-prompt line as the boundary's end and searches
# for its start scanning left from there, so two idle "❯" lines in a row
# make it pick the first ❯ as both boundaries and return an empty body —
# the watcher then logs `boundary_failed` instead of emitting the answer.
tm send-keys -l -t "$SRC_PANE" 'printf '\''The relation is $E=mc^2$.\n'\'''
tm send-keys -t "$SRC_PANE" Enter
sleep 1.5

# --- assertions ----------------------------------------------------------------
pass=1
echo "source pane:"
tm capture-pane -p -t "$SRC_PANE" | sed 's/^/  /'

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
VIEWER_CAP="$(tm capture-pane -p -S -100 -t "$VIEWER_PANE" 2>/dev/null || true)"
if printf '%s' "$VIEWER_CAP" | grep -qE 'no Kitty graphics support|no usable cell size|placed image='; then
  printf '%s\n' "$VIEWER_CAP" | grep -v '^$' | tail -3 | sed 's/^/viewer: /'
else
  echo "viewer pane: already reaped by tmux (clean exit); image path recorded in Ghostty evidence"
fi

DEFAULT_SESSIONS_AFTER="$(tmux ls 2>/dev/null || true)"
if [ "$DEFAULT_SESSIONS_BEFORE" != "$DEFAULT_SESSIONS_AFTER" ]; then
  echo "FAIL: default tmux server session list changed"
  echo "  before: ${DEFAULT_SESSIONS_BEFORE:-<empty>}"
  echo "  after:  ${DEFAULT_SESSIONS_AFTER:-<empty>}"
  exit 1
fi

if [ "$pass" = 1 ]; then
  echo "PASS: watcher emitted the answer document; real-image placement recorded in Ghostty evidence."
  exit 0
fi
exit 1
