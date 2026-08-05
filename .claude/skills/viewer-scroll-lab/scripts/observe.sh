#!/bin/zsh
# Scroll-lab observer: full-screen capture into a scratch dir (never the repo).
set -eu
label=$1
outdir=${2:-${TMPDIR:-/tmp}}
/usr/sbin/screencapture -x -o "$outdir/shot-$label.png"
echo "shot: $outdir/shot-$label.png"
