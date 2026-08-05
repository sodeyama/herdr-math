#!/usr/bin/env bash
#
# smoke-install-launcher: verifies the launcher-install block in
# scripts/install.sh warns on foreign files, installs atomically via
# temp file + mv, and is idempotent over an existing launcher script.
# Runs against a temporary sandbox only; never touches $HOME.
# Covers AT-R-101, AT-R-102.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

TMP="$(mktemp -d)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

fail() {
  echo "FAIL - $1"
  exit 1
}

# Extract the launcher-install block from install.sh so this test exercises
# the real logic, not a reimplementation.
SNIPPET="$TMP/launcher-snippet.sh"
awk '/# Launcher on PATH/,/mv -f "\$LAUNCHER_TMP" "\$BIN_HOME\/tmath"/' \
  "$ROOT/scripts/install.sh" > "$SNIPPET"
if [ ! -s "$SNIPPET" ]; then
  echo "FAIL - could not extract launcher-install block from install.sh"
  exit 1
fi

BIN_HOME="$TMP/bin"
APP="$TMP/app"
mkdir -p "$BIN_HOME" "$APP/bin"

# Seed a foreign executable (real compiled binary, not a script).
cp /bin/ls "$BIN_HOME/tmath"
chmod +x "$BIN_HOME/tmath"
OLD_INODE="$(ls -i "$BIN_HOME/tmath" | awk '{print $1}')"

# Stub app binary invoked by the launcher.
cat > "$APP/bin/tmath" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  echo "tmath 0.0.0"
  exit 0
fi
exit 1
EOF
chmod +x "$APP/bin/tmath"

run_launcher_install() {
  ( BIN_HOME="$BIN_HOME" APP="$APP" source "$SNIPPET" )
}

# --- first run: foreign file is replaced with a working launcher ------------
STDERR="$TMP/first.stderr"
if ! run_launcher_install 2>"$STDERR"; then
  fail "first launcher install exited non-zero"
fi

grep -q 'replacing non-launcher file at' "$STDERR" \
  || fail "stderr missing foreign-file warning"

NEW_INODE="$(ls -i "$BIN_HOME/tmath" | awk '{print $1}')"
[ "$OLD_INODE" != "$NEW_INODE" ] || fail "inode did not change after install"

FIRST_LINE="$(head -1 "$BIN_HOME/tmath")"
[ "$FIRST_LINE" = "#!/bin/sh" ] || fail "first line is not #!/bin/sh (got: $FIRST_LINE)"

VERSION_OUT="$("$BIN_HOME/tmath" --version 2>&1)" || fail "launcher --version exited non-zero"
[ "$VERSION_OUT" = "tmath 0.0.0" ] \
  || fail "launcher --version output wrong (got: $VERSION_OUT)"

# --- second run: idempotent over an existing launcher script ----------------
STDERR2="$TMP/second.stderr"
if ! run_launcher_install 2>"$STDERR2"; then
  fail "second launcher install exited non-zero"
fi

grep -q 'replacing non-launcher file at' "$STDERR2" \
  && fail "second run emitted foreign-file warning"

VERSION_OUT2="$("$BIN_HOME/tmath" --version 2>&1)" \
  || fail "launcher --version after second install exited non-zero"
[ "$VERSION_OUT2" = "tmath 0.0.0" ] \
  || fail "launcher --version after second install wrong (got: $VERSION_OUT2)"

# --- launcher-directory chooser ---------------------------------------------
# Extract choose_bin_home (plus its BIN_HOME assignment) so these cases
# exercise the real selection ladder, not a reimplementation.
CHOOSER="$TMP/chooser-snippet.sh"
awk '/# Launcher location/,/^BIN_HOME=/' "$ROOT/scripts/install.sh" > "$CHOOSER"
if ! grep -q 'choose_bin_home()' "$CHOOSER"; then
  fail "could not extract choose_bin_home from install.sh"
fi

# Each case runs the chooser in a subshell with a synthetic HOME and PATH.
chosen() {
  ( HOME="$1" PATH="$2" XDG_BIN_HOME="${3:-}" source "$CHOOSER"; echo "$BIN_HOME" )
}

FAKE_HOME="$TMP/home"
mkdir -p "$FAKE_HOME"

# 1. Explicit XDG_BIN_HOME always wins.
GOT="$(chosen "$FAKE_HOME" "/usr/bin:/bin" "$FAKE_HOME/custom-xdg")"
[ "$GOT" = "$FAKE_HOME/custom-xdg" ] \
  || fail "explicit XDG_BIN_HOME not honored (got: $GOT)"

# 2. An existing launcher (a #! script) on PATH under \$HOME is updated in
#    place: its directory is chosen over every default candidate.
EXISTING_DIR="$FAKE_HOME/tools/bin"
mkdir -p "$EXISTING_DIR" "$FAKE_HOME/.local/bin"
printf '#!/bin/sh\nexit 0\n' > "$EXISTING_DIR/tmath"
chmod +x "$EXISTING_DIR/tmath"
GOT="$(chosen "$FAKE_HOME" "$EXISTING_DIR:$FAKE_HOME/.local/bin:/usr/bin:/bin")"
[ "$GOT" = "$EXISTING_DIR" ] \
  || fail "existing launcher directory not respected (got: $GOT)"

# 3. A foreign (non-launcher) tmath on PATH is NOT adopted; the allowlisted
#    candidate that is on PATH wins instead.
FOREIGN_DIR="$FAKE_HOME/foreign/bin"
mkdir -p "$FOREIGN_DIR"
cp /bin/ls "$FOREIGN_DIR/tmath"
chmod +x "$FOREIGN_DIR/tmath"
GOT="$(chosen "$FAKE_HOME" "$FOREIGN_DIR:$FAKE_HOME/.local/bin:/usr/bin:/bin")"
[ "$GOT" = "$FAKE_HOME/.local/bin" ] \
  || fail "foreign binary adopted or candidate skipped (got: $GOT)"

# 4. A writable directory on PATH that is not an allowlisted candidate and
#    holds no launcher is never chosen; with no candidate on PATH the chooser
#    falls back to ~/.local/bin.
RANDOM_DIR="$FAKE_HOME/random/bin"
mkdir -p "$RANDOM_DIR"
GOT="$(chosen "$FAKE_HOME" "$RANDOM_DIR:/usr/bin:/bin")"
[ "$GOT" = "$FAKE_HOME/.local/bin" ] \
  || fail "arbitrary PATH directory chosen (got: $GOT)"

# 5. A launcher outside \$HOME is never adopted.
OUTSIDE_DIR="$TMP/outside-home/bin"
mkdir -p "$OUTSIDE_DIR"
printf '#!/bin/sh\nexit 0\n' > "$OUTSIDE_DIR/tmath"
chmod +x "$OUTSIDE_DIR/tmath"
GOT="$(chosen "$FAKE_HOME" "$OUTSIDE_DIR:/usr/bin:/bin")"
[ "$GOT" = "$FAKE_HOME/.local/bin" ] \
  || fail "launcher outside \$HOME adopted (got: $GOT)"

echo "PASS: launcher install is atomic, warns on foreign files, and the directory chooser respects user intent (AT-R-101, AT-R-102)"
exit 0
