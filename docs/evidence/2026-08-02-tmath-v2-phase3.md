# Terminal-math V2 Phase 3 Evidence (Input Loop)

Date: August 2, 2026

## Scope

This evidence covers Phase 3 for the V2 standalone `tmath` refactor, on branch
`feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 3 adds the interactive input
loop: a bounded incremental decoder (`input.rs`) that turns raw stdin bytes into mouse, key,
paste, and focus events; a scroll driver (`scroll_driver.rs`) that maps wheel deltas and fallback
keys through the existing `ScrollState`/`Smooth`; and a clean `q`/`Ctrl-C` reset wired into
`tmath render`.

## Input decoder

`InputDecoder` buffers capped bytes (64 KiB) from stdin and replays them one event at a time:

- SGR mouse reports (`ESC [ < b ; x ; y M|m`) → `Event::Mouse`, including wheel and motion.
- Cursor/page keys (`ESC [ A|B|C|D`, `ESC [ 5~|6~`, `ESC O A|B`, `Home`/`End`) → `Event::Key`.
- Plain characters, `Enter`/`Tab`/`Backspace`/`Escape`, and `Ctrl-C` (as `Char('c')`, ctrl).
- Bracketed paste (`ESC [ 200~ … ESC [ 201~`) → a single `Event::Paste` with CRLF/CR normalized
  to LF; an unclosed paste waits instead of growing unbounded.
- Focus in/out (`CSI I`/`CSI O`) → `Event::Focus`.
- String sequences (OSC/DSC/PM/APC) are skipped bounded; garbage prefixes are dropped to the
  next valid event boundary.

## Scroll driver and loop

`ScrollDriver` turns wheel notches into ±3 rows, arrows/`j`/`k` into ±1 row, `PgUp`/`PgDn`/
`g`/`G` into ±page, and `Home`/`End` into the extrema, all through `ScrollState`/`Smooth`
(exponential easing then braking). `is_exit_signal` recognizes `q` and `Ctrl-C`. In `tmath
render` the loop reads capped chunks, decodes events, feeds the driver, and returns on `q`,
`Ctrl-C`, EOF, or a 5 s bound so a non-interactive run cannot hang; the terminal is always reset
afterwards.

## Validation

```sh
cargo test          # 79 tests passed (74 core + 5 transport)
cargo clippy --all-targets   # clean
cargo fmt --check   # OK
npm test            # 386 passed (unchanged TypeScript)
```

Decoder unit tests: named/plain/control keys, scroll wheel and motion mouse, bracketed paste
(with normalization and unclosed-paste deferral), focus events, truncated-CSI deferral, garbage
skip, buffer-cap eviction, oversize parameter bounds, adversarial 512-iteration deterministic
fuzz (never panics, never unbounded, never emits a raw ESC as a character), and byte-by-byte vs
whole-buffer agreement.

Scroll driver tests: wheel/fallback/page/Home/End deltas, `q`/`Ctrl-C` detection, positive and
negative clamping with settle.

Renderer-integration test consumes a real wheel-up + wheel-down + page-key + `q` stream through
the decoder and driver: all wheel events are consumed and `q` is a clean exit signal.

CLI (non-tty) smoke unchanged:

```sh
echo 'The relation is $E=mc^2$.' | ./target/debug/tmath render -
# ok width=480 height=24 bytes=1735 renderer=katex-playwright-sharp
```

## Acceptance status

- AT-2-400 (mouse wheel → scroll state machine): decoder + driver unit/integration evidence;
  real-terminal wheel verification deferred to the Phase 3 runtime step (same manual procedure as
  Phase 2 T-302).
- AT-2-401 (keyboard fallback): arrows, `PgUp`/`PgDn`, `j`/`k`, `g`/`G`, `Home`/`End` mapped and
  tested; `q`/`Ctrl-C` reset covered by driver tests.
- AT-2-403 (bounded parsing, paste/focus hygiene): bracket paste as a single event with LF
  normalization, focus events, unclosed-paste deferral, oversize-parameter and 64 KiB buffer caps.
- AT-2-404 (clean reset on any path): `run_scroll_loop` returns on exit signal, EOF, and timeout,
  and `place_in_terminal` always resets the terminal before returning.

## Commits

- `d25cba8` `docs(spec): expand phase 3 input loop tasks` (superseded placement order note)
- `5133ddf` `docs(spec): expand phase 3 input loop tasks`
- `d5bcca3` `feat(input): decode bounded terminal input events` (T-401)
- `3627579` `feat(input): scroll with keyboard fallback and reset cleanly` (T-402)
- `dbba4a9` `feat(input): handle bracketed paste and focus events` (T-403)
- `ba8ff71` `test(input): fuzz and bound the input decoders` (T-404)
