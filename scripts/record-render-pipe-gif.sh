#!/usr/bin/env bash
# Record a sanitized `tmath render -` pipe demo GIF for public GitHub media.
#
# Uses an isolated tmux session, synthetic Markdown streamed on stdin, and
# Ghostty window capture. Output stays under docs/media/ in the repository.
#
# Requires: Ghostty, tmux 3.3+, ffmpeg, python3 (Quartz for window capture).

set -euo pipefail

export LANG="${LANG:-en_US.UTF-8}"
export LC_ALL="${LC_ALL:-en_US.UTF-8}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH_SRC="${TMATH_BIN:-$ROOT/target/release/tmath}"
OUT_DIR="$ROOT/docs/media"
OUT_GIF="${OUT_GIF:-$OUT_DIR/render-pipe-demo.gif}"
DEMO_ROOT="/tmp/tmath-render-pipe-demo-$$"
TMP="$DEMO_ROOT/scratch"
SOCKET="tmath-render-pipe-$$"
SESSION="render-pipe"
FPS=8
DURATION=38

SCROLL_DOWN_START=22
SCROLL_DOWN_END=34
SCROLL_UP_START=35
SCROLL_UP_END=38

GHOSTTY_COLS="${GHOSTTY_COLS:-100}"
GHOSTTY_ROWS="${GHOSTTY_ROWS:-40}"
GHOSTTY_FONT_SIZE="${GHOSTTY_FONT_SIZE:-12}"
TMATH_FONT_SIZE_PT="${TMATH_FONT_SIZE_PT:-16}"
GIF_WIDTH="${GIF_WIDTH:-720}"

tm() { tmux -L "$SOCKET" "$@"; }

cleanup() {
  tm kill-server 2>/dev/null || true
  rm -rf "$DEMO_ROOT"
}
trap cleanup EXIT

mkdir -p "$OUT_DIR" "$TMP"

if [ ! -x "$TMATH_SRC" ]; then
  echo "building tmath..."
  (cd "$ROOT" && cargo build --release -p tmath >/dev/null)
fi

TMATH="$DEMO_ROOT/tmath"
cp "$TMATH_SRC" "$TMATH"
chmod +x "$TMATH"
STREAM_PY="$DEMO_ROOT/stream-render-pipe.py"
cp "$ROOT/scripts/demo-stream-render-pipe.py" "$STREAM_PY"
chmod +x "$STREAM_PY"
cp "$ROOT/scripts/demo-render-pipe.md" "$DEMO_ROOT/demo-render-pipe.md"

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

scroll_pane() {
  local dir="$1"
  local hex
  if [ "$dir" = down ]; then
    hex=$'\033[<65;10;20M'
  else
    hex=$'\033[<64;10;20M'
  fi
  tm send-keys -t "$PANE" -H "$hex"
}

# Derive the crop from a real captured frame instead of the Quartz logical
# bounds: on Retina displays `screencapture -l` writes physical (2x) pixels,
# so a logical-bounds crop would keep only the top-left quadrant.
crop_filter_for_frame() {
  local png="$1" logical_h="$2"
  local pw ph scale title
  pw="$(sips -g pixelWidth "$png" 2>/dev/null | awk '/pixelWidth/{print $2}')"
  ph="$(sips -g pixelHeight "$png" 2>/dev/null | awk '/pixelHeight/{print $2}')"
  [ -n "$pw" ] && [ -n "$ph" ] || return 1
  scale=$(( (ph + logical_h / 2) / logical_h ))
  [ "$scale" -ge 1 ] || scale=1
  title=$((32 * scale))
  printf 'crop=%s:%s:0:%s' "$pw" "$((ph - title))" "$title"
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
tm new-session -d -s "$SESSION" -c "$TMP" -n "tmath render pipe" \
  -x "$GHOSTTY_COLS" -y "$GHOSTTY_ROWS" \
  "env LANG=$LANG LC_ALL=$LC_ALL zsh -f" >/dev/null
tm set-option -t "$SESSION" -w allow-passthrough on >/dev/null
tm set-option -t "$SESSION" -w mouse on >/dev/null
tm set-option -t "$SESSION" status off >/dev/null

PANE="$(tm display-message -p -t "$SESSION" '#{pane_id}')"

tm send-keys -l -t "$PANE" 'PROMPT_EOL_MARK="" ; PS1="$ " ; clear'
tm send-keys -t "$PANE" Enter

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
ensure_demo_geometry "$wid"

RENDER_CMD="env TMATH_DPR=2 TMATH_FONT_SIZE_PT=$TMATH_FONT_SIZE_PT '$STREAM_PY' | '$TMATH' render -"

read -r _bounds_w _bounds_h <<< "$(window_bounds "$wid")"
[ -n "${_bounds_h:-}" ] || { echo "FAIL: could not read bounds for window $wid" >&2; exit 1; }
probe_png="$TMP/probe.png"
/usr/sbin/screencapture -x -o -l "$wid" "$probe_png" 2>/dev/null
crop_vf="$(crop_filter_for_frame "$probe_png" "$_bounds_h"),scale=${GIF_WIDTH}:-1:flags=lanczos"
rm -f "$probe_png"
[ -n "$crop_vf" ] || { echo "FAIL: could not compute crop for window $wid" >&2; exit 1; }
echo "crop filter: $crop_vf"

echo "recording window $wid (${DURATION}s)..."
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
        tm send-keys -l -t "$PANE" "printf '%s\\n' \"cat demo.md | tmath render -\""
        tm send-keys -t "$PANE" Enter
        demo_step=1
      fi
      ;;
    1)
      if [ "$elapsed" -ge 3 ]; then
        tm send-keys -l -t "$PANE" "$RENDER_CMD"
        tm send-keys -t "$PANE" Enter
        demo_step=2
      fi
      ;;
  esac

  if [ "$elapsed" -ge "$SCROLL_DOWN_START" ] && [ "$elapsed" -lt "$SCROLL_DOWN_END" ]; then
    scroll_pane down
  elif [ "$elapsed" -ge "$SCROLL_UP_START" ] && [ "$elapsed" -lt "$SCROLL_UP_END" ]; then
    scroll_pane up
  fi

  raw="$frames_dir/raw-$i.png"
  out="$frames_dir/frame-$(printf '%04d' "$i").png"
  /usr/sbin/screencapture -x -o -l "$wid" "$raw" 2>/dev/null
  ffmpeg -y -i "$raw" -vf "$crop_vf" "$out" 2>/dev/null
  rm -f "$raw"
  i=$((i + 1))
  sleep "$interval"
done

frame_count="$(find "$frames_dir" -name 'frame-*.png' | wc -l | tr -d ' ')"
[ "$frame_count" -ge 6 ] || { echo "FAIL: too few frames ($frame_count)" >&2; exit 1; }

sample="$frames_dir/frame-0005.png"
if /usr/bin/strings "$sample" 2>/dev/null | grep -qE '/Users/|sodeyama|obsidian|MacBook|failed to launch|/var/folders/'; then
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
