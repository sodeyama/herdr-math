#!/bin/zsh
# Scroll-lab driver: inject wheel/key events into the viewer pane and
# timestamp each injection. Usage:
#   drive.sh <pane> up N [interval_ms]     - N wheel-up notches
#   drive.sh <pane> down N [interval_ms]   - N wheel-down notches
#   drive.sh <pane> end                    - End key (re-engage follow)
# SGR wheel: up = \e[<64;10;20M  down = \e[<65;10;20M
set -eu
pane=$1; action=$2
case $action in
  up)   seq_hex="1b 5b 3c 36 34 3b 31 30 3b 32 30 4d";;
  down) seq_hex="1b 5b 3c 36 35 3b 31 30 3b 32 30 4d";;
  end)  tmux send-keys -t "$pane" -H 1b 5b 34 7e; echo "$(date +%s.%N) end"; exit 0;;
esac
n=${3:-1}; interval_ms=${4:-50}
for i in $(seq 1 "$n"); do
  echo "$(date +%s.%N) $action#$i"
  tmux send-keys -t "$pane" -H ${=seq_hex}
  [ "$interval_ms" -gt 0 ] && sleep "$(printf '0.%03d' "$interval_ms")"
done
