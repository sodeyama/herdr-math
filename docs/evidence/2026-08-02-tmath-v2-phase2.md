# Terminal-math V2 Phase 2 Evidence (Placement and Scrollback Anchoring)

Date: August 2, 2026

## Scope

This evidence covers Phase 2 for the V2 standalone `tmath` refactor, on branch
`feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 2 transmits one Kitty
placement per rendered document block into the main screen buffer, glued to real cells with a
virtual placement (`U=1,c,r`) and a cursor-relative placeholder grid so the image scrolls with
the shell scrollback. It adds PNG→RGBA decoding, placement tracking with image ids, scoped
replace/delete, a graphics-support probe, and the concurrent/pixel limits.

## Placement mechanics

A rendered document PNG is decoded to 8-bit RGBA (palette input is expanded via
`normalize_to_color8 | ALPHA`). The cell grid is `cols = ceil(width/cell_w)`, `rows =
ceil(height/cell_h)`, clamped to the addressable placeholder limit. For each block the binary
emits, in order:

1. `CSI <home>;1H` (home row move, in the main buffer — never the alternate screen)
2. `kitty_transmit_placed` with virtual placement keys `U=1,c=<cols>,r=<rows>` (`q=2`, chunked)
3. A cursor-relative placeholder grid encoding the image id as the `38;2;r;g;b` foreground
   color plus per-cell row/column diacritics, so the cells exist in the scrollback.

Replacement emits a scoped `a=d,d=I,i=<id>` delete before re-transmitting with the same image
id; removal emits only the scoped delete.

## Validation

```sh
cargo test          # 55 core + 4 transport tests passed
cargo clippy --all-targets   # clean
cargo fmt --check   # OK
```

Placement unit tests (all deterministic, no terminal required):

- `grid_for` ceil/clamp and zero-size handling.
- `reserve` enforces `max_concurrent_placements` and `max_total_pixels` before emission.
- `remove` returns the image id for a scoped delete; unknown ids are `None`.
- `replace` keeps the image id and re-checks the pixel budget.
- `home_row_for_next` / `home_row_of` stack blocks in source order.
- `emit_placed_block` starts with the home-row move, carries `U=1,c,r`, and closes the
  placeholder color.
- `emit_replaced_block` emits `a=d,d=I,i=<id>` first; `emit_remove_block` is scoped per id.
- `decode_png` expands a real palette PNG to RGBA8 and rejects malformed input.
- fail-closed: a rejected `reserve` does not mutate the tracker.

Graphics probe tests: `parse_graphics_probe` accepts `Gi=4294967295;OK`, rejects partial/wrong
ids; the terminal forwards a bounded `a=q,f=32,s=1,v=1` query and reads the reply.

Renderer-integration test (`engine/crates/tmath/tests/render_transport.rs`): renders
`$E=mc^2$` through the real TS subprocess, decodes the returned PNG, and asserts the emitted
placement bytes contain the transmit header, virtual placement keys, and the placeholder color
closure.

CLI (non-tty) smoke (stdout not a terminal):

```sh
echo 'The relation is $E=mc^2$.' | ./target/debug/tmath render -
# ok width=480 height=24 bytes=1735 renderer=katex-playwright-sharp
```

CLI terminal path fails closed without partial output when a real tty is absent:

```sh
printf 'x $E=mc^2$' | script -q /tmp/tmath.out ./target/debug/tmath render -
# tmath: initialize terminal: Inappropriate ioctl for device (os error 25)
```

## Runtime evidence (T-302)

T-302 requires a real Kitty-graphics terminal. Ghostty 1.3.1 is installed. The macOS GUI is
launched with `open -na Ghostty --args -e <command>`, which needs interactive confirmation, so
the automated run was not performed by the harness in this session.

Manual procedure to complete T-302:

```sh
open -na Ghostty --args -e bash -lc '
  cd /Users/sodeyama/git/herdr-math-v2-phase0
  export TMATH_RENDER_WORKER=$PWD/dist/renderer/subprocess.js
  printf "The relation is $E=mc^2$.\n\nThen: $x = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}" \
    | ./target/debug/tmath render -
  sleep 2
'
```

Expected: one image appears in the terminal at the cursor row; scrolling the terminal back and
forth moves it with the surrounding cells (AT-2-301). The placeholder grid bytes and placement
order are already covered deterministically, so this step confirms only the visual glue in a
real terminal.

## Acceptance status

- AT-2-300 (one placement per block, main buffer, no alternate screen): unit + renderer
  integration evidence; runtime placement pending T-302's real-terminal run.
- AT-2-301 (images scroll with scrollback): manual real-terminal step documented above.
- AT-2-302 (replace and delete): scoped `d=I,i=<id>` delete covered by unit tests; no orphan
  image path.
- AT-2-303 (fail-closed placement): rejected blocks emit nothing and earlier valid placements
  are untouched; the tracker fail-closed test covers this.
- AT-2-304 (no Kitty support): the terminal path returns a clear error and exits non-zero
  before emitting any placement; the non-tty harness reproduced fail-closed behavior.

## Commits

- `d25cba8` `docs(spec): expand phase 2 placement tasks`
- `65a4825` `feat(placement): place scrollback-anchored image blocks`
- `8948775` `feat(placement): decode png and place blocks in the main buffer`
- `f22b841` `feat(placement): cap concurrent placements and pixels`
