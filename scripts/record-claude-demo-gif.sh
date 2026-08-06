#!/usr/bin/env bash
# Record a sanitized tmath + Claude Code demo GIF for public GitHub media.
#
# Uses an isolated tmux server, a scratch cwd under /tmp, synthetic Q&A (no
# real transcripts), and Ghostty window capture. Output stays under
# docs/media/ in the repository.
#
# Requires: Ghostty, tmux 3.3+, ffmpeg, python3 (Quartz for window capture).
#
# Confirmed demo recipe (2026-08): Ghostty 140×40 cells, terminal font 12 pt,
# tmath render 16 pt (`TMATH_FONT_SIZE_PT`), release `tmath` with viewer font
# forwarding and physical-pixel-aware `TMATH_DPR` handling.

set -euo pipefail

export LANG="${LANG:-en_US.UTF-8}"
export LC_ALL="${LC_ALL:-en_US.UTF-8}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH_SRC="${TMATH_BIN:-$ROOT/target/release/tmath}"
OUT_DIR="$ROOT/docs/media"
DEMO_ANSWER="${DEMO_ANSWER:-long}"
DEMO_PS1="${DEMO_PS1:-❯ }"

# Preset content for localized Bayes demos (override piecemeal via env if needed).
case "$DEMO_ANSWER" in
  bayes-ja)
    DEMO_PROMPT="${DEMO_PROMPT:-ベイズ統計の要点を数式付きで要約して。}"
    OUT_GIF="${OUT_GIF:-$OUT_DIR/claude-code-demo-bayes-ja.gif}"
    ;;
  bayes-en)
    DEMO_PROMPT="${DEMO_PROMPT:-Summarize Bayesian statistics with all key formulas.}"
    OUT_GIF="${OUT_GIF:-$OUT_DIR/claude-code-demo-bayes-en.gif}"
    ;;
  *)
    DEMO_PROMPT="${DEMO_PROMPT:-Summarize quadratic equations with all key formulas.}"
    OUT_GIF="${OUT_GIF:-$OUT_DIR/claude-code-demo.gif}"
    ;;
esac

DEMO_ROOT="/tmp/tmath-public-demo-$$"
TMP="$DEMO_ROOT/scratch"
SOCKET="tmath-demo-$$"
SESSION="demo"
FPS=8
DURATION=50

SCROLL_DOWN_START=26
SCROLL_DOWN_END=40
SCROLL_UP_START=41
SCROLL_UP_END=46

# Ghostty window-width/window-height are terminal CELLS, not pixels.
GHOSTTY_COLS="${GHOSTTY_COLS:-140}"
GHOSTTY_ROWS="${GHOSTTY_ROWS:-40}"
GHOSTTY_FONT_SIZE="${GHOSTTY_FONT_SIZE:-12}"
TMATH_FONT_SIZE_PT="${TMATH_FONT_SIZE_PT:-16}"
TMUX_COLS=$GHOSTTY_COLS
TMUX_ROWS=$GHOSTTY_ROWS
GIF_WIDTH="${GIF_WIDTH:-720}"

tm() { tmux -L "$SOCKET" "$@"; }

cleanup() {
  tm kill-server 2>/dev/null || true
  rm -rf "$DEMO_ROOT"
}
trap cleanup EXIT

mkdir -p "$OUT_DIR" "$TMP/workspace"

if [ ! -x "$TMATH_SRC" ]; then
  echo "building tmath..."
  (cd "$ROOT" && cargo build --release -p tmath >/dev/null)
fi

TMATH="$DEMO_ROOT/tmath"
cp "$TMATH_SRC" "$TMATH"
chmod +x "$TMATH"
STREAM_PY="$DEMO_ROOT/stream-answer.py"
cp "$ROOT/scripts/demo-stream-answer.py" "$STREAM_PY"
chmod +x "$STREAM_PY"

cat > "$DEMO_ROOT/answer1.txt" <<'EOF'
The quadratic formula solves $ax^2+bx+c=0$:

$$x = \frac{-b \pm \sqrt{b^2-4ac}}{2a}$$

Use it when factoring is awkward. The discriminant $b^2-4ac$ tells you how many real roots exist.
EOF

cat > "$DEMO_ROOT/answer2.txt" <<'EOF'
Vertex form: $$y = a(x-h)^2 + k$$

The vertex is at $(h,k)$. For example, $y = 2(x-3)^2 + 1$ has vertex $(3,1)$.
EOF

# Kept for reference; streaming uses demo-stream-answer.py instead of cat.

ATTACH_SCRIPT="$DEMO_ROOT/ghostty-attach.sh"
cat > "$ATTACH_SCRIPT" <<EOF
#!/bin/zsh
export LANG="${LANG}"
export LC_ALL="${LC_ALL}"
export PATH="/usr/local/bin:/opt/homebrew/bin:\$PATH"
exec tmux -L "$SOCKET" attach -t "$SESSION"
EOF
chmod +x "$ATTACH_SCRIPT"

GHOSTTY_CFG="$DEMO_ROOT/ghostty-demo.conf"
cat > "$GHOSTTY_CFG" <<EOF
window-width = $GHOSTTY_COLS
window-height = $GHOSTTY_ROWS
font-size = $GHOSTTY_FONT_SIZE
window-save-state = never
EOF

wait_for_window() {
  local wid=""
  for _ in $(seq 1 60); do
    wid="$(/usr/bin/python3 -c "
import Quartz
target = 'ghostty-attach.sh'
wins = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID)
for win in wins:
    name = (win.get('kCGWindowOwnerName') or '').lower()
    title = (win.get('kCGWindowName') or '').lower()
    if 'ghostty' in name and target in title:
        print(win['kCGWindowNumber']); break
" 2>/dev/null || true)"
    [ -n "$wid" ] && break
    sleep 0.4
  done
  [ -n "$wid" ] || return 1
  printf '%s' "$wid"
}

window_bounds() {
  WID="$1" /usr/bin/python3 -c "
import os, Quartz
wid = int(os.environ['WID'])
wins = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID)
for win in wins:
    if win.get('kCGWindowNumber') == wid:
        b = win['kCGWindowBounds']
        print(int(b['Width']), int(b['Height']))
        break
"
}

resize_demo_geometry() {
  tm resize-window -t "$SESSION" -x "$GHOSTTY_COLS" -y "$GHOSTTY_ROWS" 2>/dev/null || true
}

find_viewer_pane() {
  tm list-panes -t "$SESSION" -F '#{pane_id}' | while read -r pane; do
    [ "$pane" != "$SRC_PANE" ] && printf '%s\n' "$pane"
  done | tail -1
}

scroll_viewer() {
  local dir="$1"
  local hex
  if [ "$dir" = down ]; then
    hex=$'\033[<65;10;20M'
  else
    hex=$'\033[<64;10;20M'
  fi
  tm send-keys -t "$VIEWER_PANE" -H "$hex"
}

crop_filter_for_window() {
  WID="$1" /usr/bin/python3 -c "
import os, Quartz
wid = int(os.environ['WID'])
wins = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID)
for win in wins:
    if win.get('kCGWindowNumber') == wid:
        b = win['kCGWindowBounds']
        w, h = int(b['Width']), int(b['Height'])
        print(f'crop={w}:{h - 32}:0:32')
        break
"
}

ensure_demo_geometry() {
  local cols rows
  for _ in $(seq 1 8); do
    read -r cols rows <<< "$(tm display-message -p -t "$SESSION" '#{window_width} #{window_height}')"
    if [ "${cols:-0}" -le $((GHOSTTY_COLS + 2)) ] && [ "${cols:-0}" -ge $((GHOSTTY_COLS - 2)) ] \
      && [ "${rows:-0}" -le $((GHOSTTY_ROWS + 2)) ] && [ "${rows:-0}" -ge $((GHOSTTY_ROWS - 2)) ]; then
      bounds="$(window_bounds "$1" 2>/dev/null || true)"
      echo "Ghostty: ${cols}x${rows} cells (${bounds:-?} px)"
      return 0
    fi
    resize_demo_geometry
    sleep 0.35
  done
  read -r cols rows <<< "$(tm display-message -p -t "$SESSION" '#{window_width} #{window_height}')"
  echo "WARN: tmux geometry ${cols:-?}x${rows:-?}, wanted ${GHOSTTY_COLS}x${GHOSTTY_ROWS}" >&2
  return 0
}

# --- isolated tmux session + compact Ghostty window --------------------------
tm new-session -d -s "$SESSION" -c "$TMP/workspace" -n "Terminal Math demo" \
  -x "$TMUX_COLS" -y "$TMUX_ROWS" \
  "env LANG=$LANG LC_ALL=$LC_ALL zsh -f" >/dev/null
tm set-option -t "$SESSION" -w allow-passthrough on >/dev/null
tm set-option -t "$SESSION" -w mouse on >/dev/null
tm set-option -t "$SESSION" status off >/dev/null

SRC_PANE="$(tm display-message -p -t "$SESSION" '#{pane_id}')"

cat > "$DEMO_ROOT/run-agent.sh" <<EOF
#!/bin/zsh
export LANG="${LANG}"
export LC_ALL="${LC_ALL}"
exec env TMATH_DPR=2 TMATH_FONT_SIZE_PT=$TMATH_FONT_SIZE_PT "$TMATH" agent --source-pane $SRC_PANE --wait-ms 150 --poll-ms 60 --percent 45
EOF
chmod +x "$DEMO_ROOT/run-agent.sh"

printf '%s' "$DEMO_PROMPT" > "$DEMO_ROOT/demo-prompt.txt"

tm send-keys -l -t "$SRC_PANE" "PROMPT_EOL_MARK=\"\" ; PS1=\"$DEMO_PS1\" ; clear"
tm send-keys -t "$SRC_PANE" Enter

open -na Ghostty.app --args \
  "--config-file=$GHOSTTY_CFG" \
  "--window-width=$GHOSTTY_COLS" \
  "--window-height=$GHOSTTY_ROWS" \
  "--window-save-state=never" \
  -e "$ATTACH_SCRIPT" >/dev/null 2>&1 || true

echo "waiting for Ghostty attach (${GHOSTTY_COLS}x${GHOSTTY_ROWS} cells)..."
wid="$(wait_for_window)" || { echo "FAIL: Ghostty demo window not found" >&2; exit 1; }
resize_demo_geometry
sleep 0.5

# --- demo inside attached Ghostty --------------------------------------------
# Start the watcher in the background so only source + viewer panes remain.
tm send-keys -l -t "$SRC_PANE" "$DEMO_ROOT/run-agent.sh >/dev/null 2>&1 &"
tm send-keys -t "$SRC_PANE" Enter

started=0
for _ in $(seq 1 80); do
  pane_count="$(tm list-panes -t "$SESSION" | wc -l | tr -d ' ')"
  if [ "$pane_count" -ge 2 ]; then
    started=1
    break
  fi
  sleep 0.25
done
[ "$started" = 1 ] || { echo "FAIL: tmath agent did not start" >&2; exit 1; }
sleep 0.5
resize_demo_geometry
sleep 0.4
wid="$(wait_for_window)" || { echo "FAIL: lost Ghostty demo window" >&2; exit 1; }
ensure_demo_geometry "$wid"

VIEWER_PANE="$(find_viewer_pane)"
[ -n "$VIEWER_PANE" ] || { echo "FAIL: viewer pane not found" >&2; exit 1; }
echo "viewer pane: $VIEWER_PANE"

tm send-keys -l -t "$SRC_PANE" 'clear'
tm send-keys -t "$SRC_PANE" Enter
sleep 0.3

crop_vf="$(crop_filter_for_window "$wid"),scale=${GIF_WIDTH}:-1:flags=lanczos"
[ -n "$crop_vf" ] || { echo "FAIL: could not compute crop for window $wid" >&2; exit 1; }

echo "recording window $wid (${DURATION}s, drive demo during capture)..."
frames_dir="$TMP/frames"
mkdir -p "$frames_dir"
interval="$(python3 -c "print(f'{1/$FPS:.4f}')")"
end=$((SECONDS + DURATION))
i=0
demo_step=0
while [ "$SECONDS" -lt "$end" ]; do
  elapsed=$((DURATION - (end - SECONDS)))
  case "$demo_step" in
    0)
      if [ "$elapsed" -ge 1 ]; then
        tm send-keys -l -t "$SRC_PANE" "cat '$DEMO_ROOT/demo-prompt.txt'"
        tm send-keys -t "$SRC_PANE" Enter
        demo_step=1
      fi
      ;;
    1)
      if [ "$elapsed" -ge 3 ]; then
        tm send-keys -l -t "$SRC_PANE" "'$STREAM_PY' '$DEMO_ANSWER'"
        tm send-keys -t "$SRC_PANE" Enter
        demo_step=2
      fi
      ;;
  esac

  if [ "$elapsed" -ge "$SCROLL_DOWN_START" ] && [ "$elapsed" -lt "$SCROLL_DOWN_END" ]; then
    scroll_viewer down
  elif [ "$elapsed" -ge "$SCROLL_UP_START" ] && [ "$elapsed" -lt "$SCROLL_UP_END" ]; then
    scroll_viewer up
  fi

  raw="$frames_dir/raw-$i.png"
  out="$frames_dir/frame-$(printf '%04d' "$i").png"
  /usr/sbin/screencapture -x -o -l "$wid" "$raw" 2>/dev/null
  ffmpeg -y -i "$raw" -vf "$crop_vf" "$out" 2>/dev/null
  rm -f "$raw"
  i=$((i + 1))
  sleep "$interval"
done

if tm capture-pane -p -S -200 -t "$SRC_PANE" 2>/dev/null | grep -q 'boundary_failed'; then
  echo "FAIL: watcher boundary_failed during demo" >&2
  exit 1
fi

frame_count="$(find "$frames_dir" -name 'frame-*.png' | wc -l | tr -d ' ')"
[ "$frame_count" -ge 6 ] || { echo "FAIL: too few frames ($frame_count)" >&2; exit 1; }

sample="$frames_dir/frame-0003.png"
if /usr/bin/strings "$sample" 2>/dev/null | grep -qE '/Users/|sodeyama|obsidian|MacBook|failed to launch|boundary_failed|/var/folders/'; then
  echo "FAIL: sample frame contains personal data or errors" >&2
  exit 1
fi

palette="$TMP/palette.png"
ffmpeg -y -framerate "$FPS" -i "$frames_dir/frame-%04d.png" \
  -vf "palettegen=stats_mode=diff" "$palette" 2>/dev/null
ffmpeg -y -framerate "$FPS" -i "$frames_dir/frame-%04d.png" -i "$palette" \
  -lavfi "paletteuse=dither=bayer:bayer_scale=3" -loop 0 "$OUT_GIF" 2>/dev/null

ls -lh "$OUT_GIF"
echo "saved: $OUT_GIF"
