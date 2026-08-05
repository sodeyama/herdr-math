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

echo "PASS: launcher install is atomic, warns on foreign files (AT-R-101, AT-R-102)"
exit 0
