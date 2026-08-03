# tmath-agent.sh — auto-starts `tmath agent` watchers for coding-agent CLIs
# when the current directory is allowlisted (`tmath agent-enable`).
#
# Installed by scripts/install.sh under $APP/shell/tmath-agent.sh and sourced
# from ~/.zshrc / ~/.bashrc via a marker-delimited block. Safe to source
# multiple times (idempotent alias/function definitions).

__tmath_wrap_agent() {
  local cmd="$1"; shift

  command -v "$cmd" >/dev/null 2>&1 || { echo "tmath: '$cmd' not found" >&2; return 127; }
  # tmath itself missing, or the directory is not allowlisted: pure
  # passthrough, never block or alter the wrapped command.
  command -v tmath >/dev/null 2>&1 || { command "$cmd" "$@"; return $?; }
  tmath agent-allowed >/dev/null 2>&1 || { command "$cmd" "$@"; return $?; }

  if [ -n "${TMUX:-}" ]; then
    __tmath_start_watcher_for_pane "${TMUX_PANE:-}"
    command "$cmd" "$@"
    return $?
  fi

  # Outside tmux, only auto-tmux-ify interactive terminals; piped/redirected
  # invocations pass through untouched so scripted use never changes
  # behavior and tmath never runs.
  if [ -t 0 ] && [ -t 1 ]; then
    __tmath_start_in_new_tmux_session "$cmd" "$@"
    return $?
  fi

  command "$cmd" "$@"
}

# Pane-id-scoped lock with PID liveness check: `noclobber` gives atomic
# create-if-absent so concurrent invocations in the same pane only start one
# watcher; the stored PID + `kill -0` reclaims a lock left by a watcher that
# crashed or was killed instead of wedging auto-start forever.
__tmath_start_watcher_for_pane() {
  local pane="$1"
  [ -n "$pane" ] || return 0
  local lock="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/tmath-agent-pane-${pane#%}.lock"

  if [ -f "$lock" ]; then
    local old_pid
    old_pid="$(cat "$lock" 2>/dev/null || true)"
    if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
      return 0
    fi
    rm -f "$lock"
  fi
  ( set -o noclobber; : > "$lock" ) 2>/dev/null || return 0

  ( tmath agent --source-pane "$pane" >/dev/null 2>&1 & echo $! > "$lock"; wait ) &
  disown 2>/dev/null || true
}

# Outside tmux: build an explicit two-pane session (agent pane + watcher
# pane) rather than relying on the wrapper re-firing inside the new session —
# a plain `tmux new-session <cmd>` runs the command directly, never sources
# shell rc files, so the wrapper would not fire there.
__tmath_start_in_new_tmux_session() {
  local cmd="$1"; shift
  local session="tmath-$$"
  local quoted
  quoted="$(__tmath_shell_quote_args "$cmd" "$@")"
  tmux new-session -d -s "$session" "$quoted"
  local agent_pane
  agent_pane="$(tmux list-panes -t "$session" -F '#{pane_id}' | head -1)"
  tmux split-window -h -p 35 -t "$session" \
    "tmath agent --source-pane $agent_pane; exec \$SHELL"
  tmux attach -t "$session"
}

__tmath_shell_quote_args() {
  local out="" arg
  for arg in "$@"; do
    out="$out $(printf '%q' "$arg")"
  done
  printf '%s' "$out"
}

for __tmath_cmd in claude codex opencode cursor-agent pi; do
  eval "alias $__tmath_cmd='__tmath_wrap_agent $__tmath_cmd'"
done
unset __tmath_cmd
