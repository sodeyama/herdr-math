#!/usr/bin/env bash
#
# tmath — local installer (mirrors terminal-browser's install model).
#
# Builds the release binary and the TypeScript renderer, installs them under
# ~/.local/share/tmath/app (or $TMATH_INSTALL_ROOT), creates a `tmath` launcher
# in ~/.local/bin, and links a SKILL.md into each coding agent's skills dir so
# agents know how to render LaTeX/Markdown with tmath.
#
# Usage:
#   bash scripts/install.sh                 # from a repository checkout
#   curl -fsSL <raw .../scripts/install.sh> | bash   # anywhere (auto-clones)
#
# Options (environment variables):
#   TMATH_INSTALL_ROOT   install prefix (default ~/.local/share/tmath)
#   TMATH_SKIP_TESTS=1   skip the post-install `tmath diagnose` gate
#   TMATH_FORCE_REBUILD=1 always rebuild instead of reusing artifacts
#   TMATH_SKIP_SHELL_INTEGRATION=1  skip the ~/.zshrc / ~/.bashrc auto-watch snippet

set -euo pipefail

# ----------------------------------------------------------------------------
# Locate the repository (checkout, parent of this script, cwd, or a fresh
# shallow clone) so the one-liner form works without a local checkout.
# ----------------------------------------------------------------------------
find_repo() {
  local root
  for root in "${TMATH_BUILD_ROOT:-}" "$PWD" "$(dirname "${BASH_SOURCE[0]:-$0}")/.."; do
    [ -n "$root" ] || continue
    if [ -f "$root/Cargo.toml" ] && [ -d "$root/engine/crates/tmath" ] && [ -f "$root/package.json" ]; then
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
BIN_HOME="${XDG_BIN_HOME:-$HOME/.local/bin}"

# ----------------------------------------------------------------------------
# Prerequisites (macOS / Linux are the tested bases)
# ----------------------------------------------------------------------------
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64 | Darwin-x86_64 | Linux-x86_64 | Linux-aarch64) ;;
  *) echo "tmath: unsupported platform $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
command -v cargo >/dev/null || { echo "tmath: the Rust toolchain (cargo) is required" >&2; exit 1; }
command -v node >/dev/null || { echo "tmath: Node.js 22+ is required" >&2; exit 1; }
command -v npm >/dev/null || { echo "tmath: npm is required" >&2; exit 1; }

# ----------------------------------------------------------------------------
# Build: release Rust binary + TypeScript renderer (dist/)
# ----------------------------------------------------------------------------
VERSION="$(node -p "require('$REPO/package.json').version")"

if [ "${TMATH_FORCE_REBUILD:-0}" != "1" ] && [ -x "$REPO/target/release/tmath" ]; then
  echo "tmath $VERSION: using existing release binary"
else
  echo "tmath $VERSION: building the release binary (cargo build --release)…"
  (cd "$REPO" && cargo build --release)
fi

if [ ! -d "$REPO/node_modules" ]; then
  echo "tmath: installing renderer dependencies (npm ci)…"
  (cd "$REPO" && npm ci)
fi
echo "tmath: compiling the renderer (npm run build)…"
(cd "$REPO" && npm run build >/dev/null)

# ----------------------------------------------------------------------------
# Assemble the app tree (idempotent: swaps in a fresh .new, no partial state)
# ----------------------------------------------------------------------------
echo "tmath: installing to $APP"
rm -rf "$APP.new"
mkdir -p "$APP.new/bin" "$APP.new/renderer" "$APP.new/skill" "$APP.new/shell"
cp "$REPO/target/release/tmath" "$APP.new/bin/tmath"
cp -R "$REPO/dist" "$APP.new/renderer/dist"
cp "$REPO/package.json" "$REPO/package-lock.json" "$APP.new/renderer/"
cp "$REPO/scripts/audit-runtime.mjs" "$APP.new/renderer/scripts/" 2>/dev/null || {
  mkdir -p "$APP.new/renderer/scripts"
  cp "$REPO/scripts/audit-runtime.mjs" "$APP.new/renderer/scripts/"
}
cp "$REPO/skill/tmath/SKILL.md" "$APP.new/skill/SKILL.md"
cp "$REPO/scripts/shell/tmath-agent.sh" "$APP.new/shell/tmath-agent.sh"
printf '%s\n' "$VERSION" > "$APP.new/VERSION"

# Production renderer dependencies (omits devDependencies; the postinstall
# fetches the pinned local Chromium headless shell used by the renderer).
if [ ! -d "$APP.new/renderer/node_modules" ]; then
  echo "tmath: installing renderer runtime dependencies (npm ci --omit=dev)…"
  (cd "$APP.new/renderer" && npm ci --omit=dev >/dev/null)
fi

rm -rf "$APP"
mv "$APP.new" "$APP"

# ----------------------------------------------------------------------------
# Launcher on PATH
# ----------------------------------------------------------------------------
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
# wrapped coding-agent command runs. TMATH_SKIP_SHELL_INTEGRATION=1 opts out.
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
    echo "tmath: updated shell integration in $rc"
  else
    printf '\n' >> "$rc"
    cat "$block_file" >> "$rc"
    echo "tmath: added shell integration to $rc"
  fi
  rm -f "$block_file"
}

if [ "${TMATH_SKIP_SHELL_INTEGRATION:-0}" != "1" ]; then
  for TMATH_RC in "$HOME/.zshrc" "$HOME/.bashrc"; do
    install_shell_integration "$TMATH_RC"
  done
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
