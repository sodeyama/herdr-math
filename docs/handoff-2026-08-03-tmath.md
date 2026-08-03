# Terminal Math — Session Handoff (2026-08-03)

Passing this repo's current state and delivery history to the next working
session. Repo: `sodeyama/terminal-math` (public identity: `tmath`, product
"Terminal Math").

**Update (later on 2026-08-03):** the "CURRENT OPEN PROBLEM" below (tmux
image pixels not displaying) has been root-caused and fixed — see
[Resolution](#resolution-2026-08-03-later) at the end of this document
before reading the investigation notes as still-open. A further session
also added opt-in shell auto-watch (`tmath agent-enable`); see
[docs/coding-agents.md](coding-agents.md#auto-watch-opt-in-per-directory).

## Current state (as of the original handoff)

- `main` at `b21294f` (`fix(placement): place renders below the command line...`), pushed to
  `origin/main` (`git@github.com:sodeyama/terminal-math.git`). Working tree clean.
- Installed on this machine: `~/.local/bin/tmath` (launcher) → `~/.local/share/tmath/app/bin/tmath`
  (release binary), renderer at `~/.local/share/tmath/app/renderer/`. `tmath --version` = `0.2.0`.
  Auto-discovery of the renderer (no `TMATH_RENDER_WORKER` needed) is active.
- Full validation green at that commit: `cargo test` (5 suites), `cargo clippy --all-targets`,
  `cargo fmt --check`, `npm run check`, `npm test` (75), `npm run test:integration` (17),
  `npm run smoke:render` (10), `scripts/smoke-agent-tmux.sh`.

## What was built (this feature arc)

1. **Agent integration (Phase 8, P1)**: `tmath agent` (watcher) + `tmath agent-viewer`.
   - Watcher: tmux pane capture → answer-boundary detection (`tmath-core::agent::boundary`,
     based on `tests/fixtures/agents/answer-corpus.json`) → bounded Unix-socket JSON channel
     (`agent::codec`, doc/quit) → viewer.
   - Viewer: renders each answer via the one-shot TS renderer and places it in the pane
     (replace-by-image-id), scrolls (wheel/arrows), `q`/Ctrl-C.
   - tmux commands in `tmath-core::agent::tmux`; `\ePtmux;` passthrough wrapper in
     `tmath-core::kitty::dcs_wrap` / `wrapped_for_tty`.
2. **Install (`scripts/install.sh`, `npm run install:local`)**: builds release binary + renderer,
   installs to `~/.local/share/tmath/app`, launcher in `~/.local/bin`, links `skill/tmath/SKILL.md`
   into agent skill dirs (`.agents`, `.claude`, `.codex`, `.cursor`, `.config/opencode`,
   `.pi/agent`), runs `tmath diagnose` as a post-install gate.
3. **Per-agent usage**: `docs/coding-agents.md` (table: claude=❯/ok, codex=›/ok, opencode=┃ prompt:/ok,
   cursor=>/partial, pi=inline prompt/not yet) + the agent-facing skill.

## User-reported bug saga (all fixed, committed/pushed)

The user ran `printf '積分は $...$ です。' | tmath render -` and reported one issue at a time.

1. `tmath: initialize terminal: Inappropriate ioctl for device (os error 25)`
   → `tmath render -` read the doc from stdin (a pipe) but also used stdin as the terminal
   control device. Fix: when stdin is not a tty, open a control device
   (`/dev/tty` initially, see #2) for raw mode/probes/input; document read stays on the pipe.
   Commit `1a5743b`.
2. Probe success in direct terminal but **`/dev/tty` probe reads failed** (reply leaked/echoed
   as `^[_Gi=4294967295;OK^\`).
   → **Root cause: macOS `poll(2)` on a freshly opened `/dev/tty` fd reports readiness as
   `POLLPRI` (0x20) instead of `POLLIN`**, so `PollFlags::IN` checks missed it. Fix: use the
   **original stdout fd (fd1)** as the control device when stdin is piped (stdout is the real
   terminal and polls with POLLIN). Confirmed with a C probe: `poll(fd1)→POLLIN`, `poll(/dev/tty)→POLLPRI`.
   Commit `9c3c7d3`.
3. Inside **tmux**, no image (only a warning). `tmath` now auto-runs
   `tmux set-option -w allow-passthrough on` (best-effort) for both `render` and `agent`.
   Commit `4815023`.
4. In direct terminal, **the command hung / got `killed`** (zsh job report `[N] done printf ... |
   killed tmath`). → `tmath render -` entered the interactive scroll loop and held the terminal
   forever even though stdin was a pipe (user cannot send keys). Fix: when stdin is not a
   terminal, **place the image and return immediately** (image stays scrollback-anchored);
   interactive scroll (`q`/Ctrl-C) only when stdin is a tty (file argument). Commit `c399b00`.
5. Image appeared **at the top of the terminal** instead of below the command line.
   → `emit_placed_block` used absolute row 1 (`\x1b[{home_row};1H`). Fix: new
   `emit_placed_block_cursor` advances one line (`\r\n`) from the cursor and places there.
   Commit `b21294f`.

## CURRENT OPEN PROBLEM (the live thread)

**Inside tmux, the image pixels are not displayed — only the placeholder-grid glyph wall
shows.** Running in tmux:

```
➜  obsidian git:(main) ✗ printf '積分は $...$ です。' | tmath render -
tmath: tmux passthrough enabled (allow-passthrough on)
<wall of placeholder combining-glyph cells>
placed width=480 height=24 image_id=1
➜  ...
```

- The placeholder grid (combining chars) is rendered (it is normal text), but the **Kitty image
  transmit that should overlay it is not drawn** when forwarded through tmux passthrough.
- **Direct terminal (no tmux): image displays fine** (user confirmed). Probe works.
- **User answered "outer terminal is NOT Ghostty"** (Ghostty以外) — the actual outer terminal is
  **currently unidentified; must ask/inspect** (`tty`, Ghostty vs iTerm vs …; iTerm2 does not
  support tmux DCS-passthrough Kitty images the same way; kitty/WezTerm do).
- User direction: "もっとちゃんと調べて。実験して" (investigate thoroughly and experiment).

### Hypotheses / next experiments (unfinished)

1. **Identify the real outer terminal** (`echo $TERM`, `tmux display -p '#{client_termname}'`,
   ask the user). Everything below depends on this. Earlier tests assumed Ghostty because I opened
   Ghostty windows for verification, but the user's actual terminal is something else.
2. **Does the outer terminal answer the `a=q` probe through tmux passthrough at all?**
   Earlier (agent-viewer investigation) we only confirmed the Ghostty* I spawned answered when
   direct; inside tmux we got no probe reply (queries don't round-trip). Test on the real terminal:
   C probe that sends `\ePtmux;...a=q...` from a tmux pane and reads the reply from fd1/`/dev/tty`.
3. **Feature/advertising check**: `tmux display -p -t <pane> '#{client_termname} #{client_termfeatures}'`
   and whether `terminal-features` needs `passthrough`/`kitty-graphics` set for that terminal before
   attaching (earlier finding: Ghostty's xterm-ghostty advertised only `bpaste,focus,title` in tmux).
4. **Placement type**: try **cursor placement `p=1,C=1`** (floating image at the cursor, no virtual
   `U=1` placement / no placeholder grid) for the tmux path — if Ghostty/other renders passthrough
   transmits, the image may show at the cursor even if the scrollback-anchored `U=1` form does not.
5. **Wrap scope**: currently `wrapped_for_tty` wraps the *whole* emit (transmit + grid + cursor
   moves) in one `\ePtmux;...`. Experiment with wrapping **only the `\e_G...\e\` APC** and letting
   the grid/cursor text go through normal tmux output.
6. **tmux `allow-passthrough` detection**: verify `tmux show-options -w allow-passthrough` in the
   user's tmux actually flips to `on` (tmath does it best-effort and prints a message).
7. **iTerm2 specific**: if outer terminal is iTerm2, check "imgcat" support / tmux integration
   flag; iTerm2's tmux mode and Kitty graphics support are limited.

### Decision points for the next session

- If the outer terminal cannot render Kitty images through tmux passthrough, the pragmatic
  product stance is: **direct terminal = supported for images; tmux = do not spam the glyph
  wall** (print a readable fallback e.g. the formula text or a clear note) unless/until a
  terminal that supports passthrough images (kitty, WezTerm) is used. Confirm with the user.
- The `install.sh` install is on this machine and gets updated by copying `target/release/tmath`
  into `~/.local/share/tmath/app/bin/` after `cargo build --release`; the renderer `dist` lives in
  the app's `renderer/` dir.

## Environment

- macOS arm64, tmux 3.5a, Node 22 (renderer), Rust. Ghostty 1.3.1 installed (used only for my
  verification windows; the user's real outer terminal is NOT Ghostty).
- Dev binary: `target/debug/tmath`, renderer worker repo `dist/renderer/subprocess.js`.
- Installed binary: `~/.local/share/tmath/app/bin/tmath`; launcher `~/.local/bin/tmath`.
- Verification harnesses (reusable): `scripts/smoke-agent-tmux.sh`
  (headless pipeline test), and the C probe idea above for poll/feature checks.
- No leftover tmath/tmux demo processes or sockets (cleaned); no demo sessions remain.

## Key source files

- `engine/crates/tmath-core/src/terminal.rs` — control-device selection (`open_control_terminal`),
  `StdioTty` (fd1 control), `read_report`/probe reading, `set_raw`.
- `engine/crates/tmath/src/main.rs` — `place_in_terminal`, `run_scroll_loop`, `enable_tmux_passthrough`,
  auto-discovery in `engine/crates/tmath/src/render.rs`.
- `engine/crates/tmath-core/src/placement.rs` — `emit_placed_block_cursor` (+ absolute variant),
  placement tracker.
- `engine/crates/tmath-core/src/kitty.rs` — `wrapped_for_tty` / `dcs_wrap` (`\ePtmux;`), `inside_tmux`.
- `engine/crates/tmath/src/agent_watcher.rs`, `agent_viewer.rs`, `engine/crates/tmath-core/src/agent/*`
  (boundary/codec/tmux), `src/renderer/subprocess.ts` (entry realpath + drain fix).
- Docs: `docs/coding-agents.md`, `docs/getting-started.md`, `docs/architecture.md`,
  `specs/terminal-math-v2/{tests,plans,tasks}/main.md` (Phase 8), `docs/evidence/*`.

## Resolution (2026-08-03, later)

The root cause matched hypothesis 5 above (wrap scope): `wrapped_for_tty` DCS-wrapped the
*entire* emit (transmit + placeholder grid + cursor moves) in one `\ePtmux;...` envelope and did
not double embedded `ESC` bytes inside it, so tmux either swallowed pane-local output alongside
the graphics command or forwarded a malformed passthrough string.

Fix, landed as structured pane-local/graphics operations
(`engine/crates/tmath-core/src/placement.rs::TerminalOp`, `engine/crates/tmath/src/
terminal_output.rs`):

- Kitty APC upload chunks are framed as independent commands
  (`kitty_transmit_placed_commands`) instead of one concatenated blob.
- Each Kitty APC is wrapped in its own `\ePtmux;...\e\\` envelope with every embedded `ESC`
  doubled (`kitty::dcs_wrap`); pane-local bytes (cursor movement, terminal modes, placeholder
  cells, color CSI, line breaks) are written as normal tmux pane output, never wrapped.
- A graphics route is selected per environment
  (`terminal_output::selected_route`/`Route::{Direct,TmuxPassthrough,TmuxClientTty}`): the
  default inside tmux writes only Kitty commands directly to the attached client's tty (bypassing
  tmux's DCS relay entirely for graphics), with `TMATH_TMUX_TRANSPORT=passthrough` selecting the
  corrected DCS route. Outside tmux, direct writes are used as before.
- `known_outer_terminal()` fails closed (refuses placeholder output) unless the outer terminal is
  advertised or otherwise verified as Kitty-capable, instead of assuming any tmux client can
  display graphics.

Verified with controlled pixel display on Ghostty 1.3.1 + tmux 3.5a (both routes) and cmux
0.64.12 + tmux 3.5a (client-tty route) — see
[docs/evidence/2026-08-03-tmath-tmux-graphics.md](evidence/2026-08-03-tmath-tmux-graphics.md).
Resize, detach/attach, multiple clients, and the full live-agent boundary matrix remain the
open P1 items tracked as `AT-2-806`/`AT-2-810`/`AT-2-811` in
`specs/terminal-math-v2/tests/main.md` — narrower claims than the earlier "supported" framing in
this document's investigation notes above, which predate the fix and should be read as history,
not current status.
