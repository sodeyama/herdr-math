#!/bin/zsh
# Scroll-lab observer: capture into a scratch dir (never the repo).
#
# Default captures the OUTER TERMINAL WINDOW by CGWindowID (occlusion-immune:
# the window buffer is captured even when other apps overlap it — a
# full-screen `screencapture -x` silently photographs whatever app happens to
# be frontmost instead). Falls back to full-screen when no matching window is
# found. Optional x/y/w/h args crop the shot (window-relative pixels) via
# crop.py, so the agent can zoom into the viewer pane for glyph-level reads.
#
# usage: observe.sh <label> [outdir] [x y w h]
set -eu
label=$1
outdir=${2:-${TMPDIR:-/tmp}}
out="$outdir/shot-$label.png"
app=${OBSERVE_APP:-ghostty}
wid=$(/usr/bin/python3 -c "
import Quartz
wins = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID)
for win in wins:
    if '$app' in (win.get('kCGWindowOwnerName') or '').lower():
        print(win['kCGWindowNumber']); break
" 2>/dev/null || true)
if [ -n "$wid" ]; then
  /usr/sbin/screencapture -x -o -l "$wid" "$out"
else
  /usr/sbin/screencapture -x -o "$out"
fi
if [ $# -ge 6 ]; then
  /usr/bin/python3 "${0:A:h}/crop.py" "$out" "$out" "$3" "$4" "$5" "$6" >/dev/null
fi
echo "shot: $out"
