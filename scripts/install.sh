#!/usr/bin/env bash
#
# tmath — local installer (mirrors terminal-browser's install model).
#
# Builds the release binary and installs it under
# ~/.local/share/tmath/app (or $TMATH_INSTALL_ROOT), creates a `tmath` launcher
# in ~/.local/bin, and links a SKILL.md into each coding agent's skills dir so
# agents know how to render LaTeX/Markdown with tmath.
#
# Usage:
#   bash scripts/install.sh                 # from a repository checkout
#   curl -fsSL <raw .../scripts/install.sh> | bash   # anywhere (auto-clones)
#   bash scripts/install.sh --with-shell-integration   # also add the rc snippet
#
# Options:
#   --with-shell-integration        opt in to adding the auto-watch snippet to
#                                   ~/.zshrc / ~/.bashrc (never added silently;
#                                   an existing marker block keeps being updated)
#   TMATH_WITH_SHELL_INTEGRATION=1  same as the flag (for `curl | bash`)
#   TMATH_INSTALL_ROOT   install prefix (default ~/.local/share/tmath)
#   TMATH_SKIP_TESTS=1   skip the post-install `tmath diagnose` gate
#   TMATH_FORCE_REBUILD=1 always rebuild instead of reusing artifacts
#   TMATH_SKIP_SHELL_INTEGRATION=1  never touch rc files, not even an existing block

set -euo pipefail

WITH_SHELL_INTEGRATION="${TMATH_WITH_SHELL_INTEGRATION:-0}"
for arg in "$@"; do
  case "$arg" in
    --with-shell-integration) WITH_SHELL_INTEGRATION=1 ;;
    *) echo "tmath: unknown option $arg" >&2; exit 1 ;;
  esac
done

# ----------------------------------------------------------------------------
# Locate the repository (checkout, parent of this script, cwd, or a fresh
# shallow clone) so the one-liner form works without a local checkout.
# ----------------------------------------------------------------------------
find_repo() {
  local root
  for root in "${TMATH_BUILD_ROOT:-}" "$PWD" "$(dirname "${BASH_SOURCE[0]:-$0}")/.."; do
    [ -n "$root" ] || continue
    if [ -f "$root/Cargo.toml" ] && [ -d "$root/engine/crates/tmath" ]; then
      echo "$(cd "$root" && pwd)"
      return 0
    fi
  done
  local tmp
  tmp="$(mktemp -d)"
  echo "tmath: cloning the repository into $tmp" >&2
  git clone --depth 1 https://github.com/sodeyama/terminal-math.git "$tmp" >/dev/null
  echo "$tmp"
}

REPO="$(find_repo)"

# ----------------------------------------------------------------------------
# Targets
# ----------------------------------------------------------------------------
APP="${TMATH_INSTALL_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/tmath}/app"

# ----------------------------------------------------------------------------
# Launcher location
# ----------------------------------------------------------------------------
# choose_bin_home: pick where the launcher goes, most specific intent first.
#   1. $XDG_BIN_HOME — explicit user configuration always wins.
#   2. The directory of an existing tmath LAUNCHER on PATH (a `#!` script
#      under $HOME, user-owned, in a writable directory): updating in place
#      prevents this install leaving a second copy that an earlier PATH entry
#      then shadows — the version-skew failure `tmath diagnose` warns about.
#   3. The first known user-bin candidate (~/.local/bin, then ~/bin) that
#      already exists and is on PATH.
#   4. ~/.local/bin, created if needed (PATH guidance is printed at the end).
# Deliberately narrow: never scans PATH for arbitrary writable directories
# (another toolchain's bin directory is not ours to write into) and never
# chooses a location outside $HOME.
choose_bin_home() {
  if [ -n "${XDG_BIN_HOME:-}" ]; then
    echo "$XDG_BIN_HOME"
    return
  fi
  local existing dir cand
  existing="$(command -v tmath 2>/dev/null || true)"
  if [ -n "$existing" ] && [ "$(head -c 2 "$existing" 2>/dev/null || true)" = "#!" ]; then
    dir="$(cd "$(dirname "$existing")" && pwd)"
    case "$dir" in
      "$HOME"/*)
        if [ -w "$dir" ] && [ -O "$existing" ]; then
          echo "$dir"
          return
        fi
        ;;
    esac
  fi
  for cand in "$HOME/.local/bin" "$HOME/bin"; do
    [ -d "$cand" ] || continue
    case ":$PATH:" in
      *":$cand:"*)
        echo "$cand"
        return
        ;;
    esac
  done
  echo "$HOME/.local/bin"
}
BIN_HOME="$(choose_bin_home)"

# ----------------------------------------------------------------------------
# Prerequisites (macOS / Linux are the tested bases)
# ----------------------------------------------------------------------------
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64 | Darwin-x86_64 | Linux-x86_64 | Linux-aarch64) ;;
  *) echo "tmath: unsupported platform $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
command -v cargo >/dev/null || { echo "tmath: the Rust toolchain (cargo) is required" >&2; exit 1; }

# ----------------------------------------------------------------------------
# Build: release Rust binary
# ----------------------------------------------------------------------------
VERSION="$(grep '^version' "$REPO/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"

if [ "${TMATH_FORCE_REBUILD:-0}" != "1" ] && [ -x "$REPO/target/release/tmath" ]; then
  echo "tmath $VERSION: using existing release binary"
else
  echo "tmath $VERSION: building the release binary (cargo build --release)…"
  (cd "$REPO" && cargo build --release)
fi

# ----------------------------------------------------------------------------
# Assemble the app tree (idempotent: swaps in a fresh .new, no partial state)
# ----------------------------------------------------------------------------
echo "tmath: installing to $APP"
rm -rf "$APP.new"
mkdir -p "$APP.new/bin" "$APP.new/skill" "$APP.new/shell"
cp "$REPO/target/release/tmath" "$APP.new/bin/tmath"
cp "$REPO/skill/tmath/SKILL.md" "$APP.new/skill/SKILL.md"
cp "$REPO/scripts/shell/tmath-agent.sh" "$APP.new/shell/tmath-agent.sh"
printf '%s\n' "$VERSION" > "$APP.new/VERSION"

rm -rf "$APP"
mv "$APP.new" "$APP"

# ----------------------------------------------------------------------------
# Launcher on PATH
# ----------------------------------------------------------------------------
echo "tmath: launcher directory $BIN_HOME"
mkdir -p "$BIN_HOME"
if [ -f "$BIN_HOME/tmath" ] && [ "$(head -c 2 "$BIN_HOME/tmath")" != "#!" ]; then
  echo "tmath: replacing non-launcher file at $BIN_HOME/tmath" >&2
fi
# Atomic install: overwriting an already-executed file in place poisons the
# macOS kernel code-signature cache for that inode and later executions die
# with SIGKILL.
LAUNCHER_TMP="$BIN_HOME/.tmath.launcher.$$"
cat > "$LAUNCHER_TMP" <<EOF
#!/bin/sh
# tmath launcher (install: $APP)
exec "$APP/bin/tmath" "\$@"
EOF
chmod +x "$LAUNCHER_TMP"
mv -f "$LAUNCHER_TMP" "$BIN_HOME/tmath"

# ----------------------------------------------------------------------------
# Skill for coding agents (Claude Code, Codex, Cursor, opencode, pi, ...)
# ----------------------------------------------------------------------------
SKILL_DIR="$APP/skill"
LINKED=""
for SKILLS in "$HOME/.agents/skills" "$HOME/.claude/skills" "$HOME/.codex/skills" \
              "$HOME/.cursor/skills" "$HOME/.config/opencode/skills" "$HOME/.pi/agent/skills"; do
  [ -n "$SKILLS" ] || continue
  mkdir -p "$SKILLS"
  LINK="$SKILLS/tmath"
  if [ -e "$LINK" ] && [ ! -L "$LINK" ]; then
    echo "tmath: $LINK exists and is not a symlink; leaving it" >&2
    continue
  fi
  ln -sfn "$SKILL_DIR" "$LINK"
  LINKED="$LINKED $(basename "$(dirname "$SKILLS")")/tmath"
done

# ----------------------------------------------------------------------------
# Shell integration (opt-in auto-watch): source $APP/shell/tmath-agent.sh from
# ~/.zshrc and ~/.bashrc via a marker-delimited block so `tmath agent-enable`d
# directories get a background `tmath agent` watcher automatically when a
# wrapped coding-agent command runs.
#
# Editing rc files is the one thing an installer must not do silently: a NEW
# block is only added with --with-shell-integration (or
# TMATH_WITH_SHELL_INTEGRATION=1). An EXISTING marker block is refreshed on
# every install — its presence is the user's earlier consent, and a stale
# block pointing at an old install path would break the wrapper.
# TMATH_SKIP_SHELL_INTEGRATION=1 skips rc files entirely, even existing blocks.
# ----------------------------------------------------------------------------
TMATH_SHELL_SNIPPET_PATH="$APP/shell/tmath-agent.sh"
TMATH_MARK_BEGIN='# >>> tmath shell integration >>>'
TMATH_MARK_END='# <<< tmath shell integration <<<'

install_shell_integration() {
  local rc="$1"
  [ -f "$rc" ] || touch "$rc"

  local block_file="$rc.tmath.block"
  printf '%s\n[ -f "%s" ] && source "%s"\n%s\n' \
    "$TMATH_MARK_BEGIN" "$TMATH_SHELL_SNIPPET_PATH" "$TMATH_SHELL_SNIPPET_PATH" "$TMATH_MARK_END" \
    > "$block_file"

  if grep -qF "$TMATH_MARK_BEGIN" "$rc" 2>/dev/null; then
    # awk -v mangles multi-line strings on some platforms (e.g. macOS's
    # onetrueawk), so the replacement block is read from a file instead.
    awk -v begin="$TMATH_MARK_BEGIN" -v end="$TMATH_MARK_END" -v blockfile="$block_file" '
      $0 == begin {
        while ((getline line < blockfile) > 0) print line
        close(blockfile)
        skip=1; next
      }
      $0 == end   { skip=0; next }
      skip        { next }
      { print }
    ' "$rc" > "$rc.tmath.tmp" && mv "$rc.tmath.tmp" "$rc"
    echo "tmath: updated shell integration in $rc (remove: delete the '$TMATH_MARK_BEGIN' block)"
  elif [ "$WITH_SHELL_INTEGRATION" = "1" ]; then
    printf '\n' >> "$rc"
    cat "$block_file" >> "$rc"
    echo "tmath: added shell integration to $rc (remove: delete the '$TMATH_MARK_BEGIN' block)"
  fi
  rm -f "$block_file"
}

if [ "${TMATH_SKIP_SHELL_INTEGRATION:-0}" != "1" ]; then
  TMATH_RC_TOUCHED=0
  for TMATH_RC in "$HOME/.zshrc" "$HOME/.bashrc"; do
    if grep -qF "$TMATH_MARK_BEGIN" "$TMATH_RC" 2>/dev/null || [ "$WITH_SHELL_INTEGRATION" = "1" ]; then
      install_shell_integration "$TMATH_RC"
      TMATH_RC_TOUCHED=1
    fi
  done
  if [ "$TMATH_RC_TOUCHED" = "0" ]; then
    echo "tmath: shell auto-watch integration not installed (rc files untouched);"
    echo "tmath: re-run with --with-shell-integration to enable it"
  fi
else
  echo "tmath: skipping shell integration (TMATH_SKIP_SHELL_INTEGRATION=1)"
fi

# ----------------------------------------------------------------------------
# Verify
# ----------------------------------------------------------------------------
echo "tmath: installed $VERSION to $APP"
echo "tmath: launcher $BIN_HOME/tmath (on PATH? see below)"
echo "tmath: skill linked into:$LINKED"

if [ "${TMATH_SKIP_TESTS:-0}" != "1" ]; then
  "$APP/bin/tmath" --version
  "$APP/bin/tmath" diagnose || {
    echo "tmath: diagnose reported a missing capability; fix it and re-run install. (TMATH_SKIP_TESTS=1 bypasses this.)" >&2
    exit 1
  }
fi

case ":$PATH:" in
  *":$BIN_HOME:"*) ;;
  *)
    echo
    echo "add $BIN_HOME to your PATH first:"
    echo "  echo 'export PATH=\"$BIN_HOME:\$PATH\"' >> ~/.zshrc && exec zsh"
    ;;
esac
echo
echo "  tmath render notes.md"
echo "  # inside tmux, watch a coding agent pane:"
echo "  tmath agent --source-pane %0"
