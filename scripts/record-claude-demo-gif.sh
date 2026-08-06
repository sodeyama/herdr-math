#!/usr/bin/env bash
# Record a sanitized tmath + Claude Code demo GIF for public GitHub media.
#
# Uses an isolated tmux server, a scratch cwd under /tmp, synthetic Q&A (no
# real transcripts), and Ghostty window capture. Output stays under
# docs/media/ in the repository.
#
# Requires: Ghostty, tmux 3.3+, ffmpeg, python3 (Quartz for window capture).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH_SRC="${TMATH_BIN:-$ROOT/target/release/tmath}"
OUT_DIR="$ROOT/docs/media"
OUT_GIF="$OUT_DIR/claude-code-demo.gif"
DEMO_ROOT="/tmp/tmath-public-demo-$$"
TMP="$DEMO_ROOT/scratch"
SOCKET="tmath-demo-$$"
SESSION="demo"
FPS=7
DURATION=14

tm() { tmux -L "$SOCKET" "$@"; }

cleanup() {
  tm kill-server 2>/dev/null || true
  rm -rf "$DEMO_ROOT"
}
trap cleanup EXIT

mkdir -p "$OUT_DIR" "$TMP/bin" "$TMP/workspace"

if [ ! -x "$TMATH_SRC" ]; then
  echo "building tmath..."
  (cd "$ROOT" && cargo build --release -p tmath >/dev/null)
fi

TMATH="$DEMO_ROOT/tmath"
cp "$TMATH_SRC" "$TMATH"
chmod +x "$TMATH"

cat > "$DEMO_ROOT/answer1.txt" <<'EOF'
The quadratic formula solves $ax^2+bx+c=0$:

$$x = \frac{-b \pm \sqrt{b^2-4ac}}{2a}$$

Use it when factoring is awkward. The discriminant $b^2-4ac$ tells you how many real roots exist.
EOF

cat > "$DEMO_ROOT/answer2.txt" <<'EOF'
Vertex form: $$y = a(x-h)^2 + k$$

The vertex is at $(h,k)$. For example, $y = 2(x-3)^2 + 1$ has vertex $(3,1)$.
EOF

ATTACH_SCRIPT="$DEMO_ROOT/ghostty-attach.sh"
cat > "$ATTACH_SCRIPT" <<EOF
#!/bin/zsh
export PATH="/usr/local/bin:/opt/homebrew/bin:\$PATH"
exec tmux -L "$SOCKET" attach -t "$SESSION"
EOF
chmod +x "$ATTACH_SCRIPT"

wait_for_window() {
  local wid=""
  for _ in $(seq 1 60); do
    wid="$(DEMO_ROOT="$DEMO_ROOT" /usr/bin/python3 -c "
import os, Quartz
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

# --- isolated tmux session + Ghostty attach ----------------------------------
tm new-session -d -s "$SESSION" -c "$TMP/workspace" -n "Terminal Math demo" -x 120 -y 34 'zsh -f' >/dev/null
tm set-option -t "$SESSION" -w allow-passthrough on >/dev/null
tm set-option -t "$SESSION" -w mouse on >/dev/null
tm set-option -t "$SESSION" status off >/dev/null

SRC_PANE="$(tm display-message -p -t "$SESSION" '#{pane_id}')"
tm send-keys -l -t "$SRC_PANE" 'PROMPT_EOL_MARK="" ; PS1="❯ " ; clear'
tm send-keys -t "$SRC_PANE" Enter

open -na Ghostty.app --args -e "$ATTACH_SCRIPT" >/dev/null 2>&1 || true
echo "waiting for Ghostty attach..."
wid="$(wait_for_window)" || { echo "FAIL: Ghostty demo window not found" >&2; exit 1; }
/usr/bin/osascript -e 'tell application "Ghostty" to activate' >/dev/null 2>&1 || true
sleep 1.0

# --- demo inside attached Ghostty --------------------------------------------
tm send-keys -l -t "$SRC_PANE" 'printf "Explain the quadratic formula and when to use it.\n"'
tm send-keys -t "$SRC_PANE" Enter
sleep 0.7

tm split-window -h -p 38 -t "$SESSION" 'zsh -f' >/dev/null
WATCHER_PANE="$(tm display-message -p -t "$SESSION" '#{pane_id}')"
tm send-keys -l -t "$WATCHER_PANE" "PS1='' ; clear ; TMATH_DPR=2 $TMATH agent --source-pane $SRC_PANE --wait-ms 400 --poll-ms 100 --percent 38"
tm send-keys -t "$WATCHER_PANE" Enter

started=0
for _ in $(seq 1 80); do
  pane_count="$(tm list-panes -t "$SESSION" | wc -l | tr -d ' ')"
  if [ "$pane_count" -ge 3 ]; then
    started=1
    break
  fi
  sleep 0.25
done
[ "$started" = 1 ] || { echo "FAIL: tmath agent did not start" >&2; exit 1; }

tm send-keys -l -t "$WATCHER_PANE" 'clear'
tm send-keys -t "$WATCHER_PANE" Enter
sleep 0.8

tm send-keys -l -t "$SRC_PANE" "cat '$DEMO_ROOT/answer1.txt'"
tm send-keys -t "$SRC_PANE" Enter
sleep 3.5

if tm capture-pane -p -S -200 -t "$WATCHER_PANE" 2>/dev/null | grep -q 'boundary_failed'; then
  echo "FAIL: watcher boundary_failed on first answer" >&2
  exit 1
fi

tm send-keys -l -t "$SRC_PANE" 'printf "Also show the vertex form.\n"'
tm send-keys -t "$SRC_PANE" Enter
sleep 0.6
tm send-keys -l -t "$SRC_PANE" "cat '$DEMO_ROOT/answer2.txt'"
tm send-keys -t "$SRC_PANE" Enter
sleep 3.0

tm resize-pane -t "$WATCHER_PANE" -x 1 >/dev/null 2>&1 || true
sleep 0.5

read -r crop_w crop_h crop_x crop_y <<EOF
$(WID="$wid" /usr/bin/python3 -c "
import os, Quartz
wid = int(os.environ['WID'])
wins = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID)
for win in wins:
    if win.get('kCGWindowNumber') == wid:
        b = win['kCGWindowBounds']
        w, h = int(b['Width']), int(b['Height'])
        print(w - 24, h - 32, 0, 32)
        break
")
EOF

echo "recording window $wid (${DURATION}s)..."
frames_dir="$TMP/frames"
mkdir -p "$frames_dir"
interval="$(python3 -c "print(f'{1/$FPS:.4f}')")"
end=$((SECONDS + DURATION))
i=0
while [ "$SECONDS" -lt "$end" ]; do
  raw="$frames_dir/raw-$i.png"
  out="$frames_dir/frame-$(printf '%04d' "$i").png"
  /usr/sbin/screencapture -x -o -l "$wid" "$raw" 2>/dev/null
  ffmpeg -y -i "$raw" -vf "crop=${crop_w}:${crop_h}:${crop_x}:${crop_y},scale=920:-1:flags=lanczos" "$out" 2>/dev/null
  rm -f "$raw"
  i=$((i + 1))
  sleep "$interval"
done

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
