#!/bin/zsh
# Inject scroll-wheel events into a tmux pane (viewer). Usage:
#   demo-drive-scroll.sh <pane> down [count] [interval_ms]
#   demo-drive-scroll.sh <pane> up   [count] [interval_ms]

set -eu
pane=$1
action=$2
count=${3:-1}
interval_ms=${4:-50}

case $action in
  up)   seq_hex="1b 5b 3c 36 34 3b 31 30 3b 32 30 4d" ;;
  down) seq_hex="1b 5b 3c 36 35 3b 31 30 3b 32 30 4d" ;;
  *)
    echo "usage: $0 <pane> up|down [count] [interval_ms]" >&2
    exit 2
    ;;
esac

for _ in $(seq 1 "$count"); do
  tmux send-keys -t "$pane" -H ${=seq_hex}
  if [ "$interval_ms" -gt 0 ]; then
    sleep "$(printf '0.%03d' "$interval_ms")"
  fi
done
