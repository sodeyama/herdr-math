# Evidence: Many-Placement Viewer Behavior in Ghostty + tmux (AT-3-501, AT-3-503)

- Date: August 5, 2026
- Environment: macOS arm64 (Darwin 25.6.0), release build
- Outer terminal: Ghostty, route: tmux (`TMATH_TMUX_TRANSPORT=passthrough`,
  `allow-passthrough on`, `mouse on` — both pre-existing tmux settings, unchanged)
- `tmath` commit observed: `97c435cf5f4f1193f61fe0c24ee1cdb9d25aa245` (`97c435c`)
- Harness: `.claude/skills/viewer-scroll-lab` (`feed.py`, `drive.sh`) plus
  `tmux pipe-pane` to capture the viewer pane's raw output stream (Kitty
  graphics escape sequences) to a scratch file for offline analysis
- Reproduce: build `cargo build --release -p tmath`, run
  `tmath agent --source-pane <pane>` with
  `TMATH_VIEWER_LOG=1 TMATH_TMUX_TRANSPORT=passthrough TMATH_RENDER_WORKER=<path to dist/renderer/subprocess.js>`,
  feed content with `feed.py blocks N <delay>`, drive scroll with
  `drive.sh <viewer-pane> up N <interval_ms>`

## Method note: no screenshots this run

The host running this session has many concurrent Ghostty spaces/tabs across
unrelated private projects. Three attempts at whole-screen or
whole-window `screencapture` each captured a different unrelated project's
private session content (a customer cost dashboard, a git operation log, an
unrelated agent's chat transcript). None of those images were kept — each
was deleted from the scratch directory within the same turn it was taken,
before any further use, and never staged or committed. No reliable
non-interactive way to bring only the `terminal-math` Ghostty space to the
front was found, so further screenshot attempts were abandoned to avoid
repeating the exposure. This evidence run instead validates the viewer
through the raw Kitty graphics protocol byte stream captured via
`tmux pipe-pane`, which never leaves the terminal-math pane's own output and
contains no other project's data. See "Limitations" below for what this
does and does not establish relative to a direct pixel/visual check.

## Setup

The lab transcript (`feed.py`) generates synthetic paragraphs, inline/display
math, and bold/code text — no real document or transcript content. 45 blocks
total were fed across the run (40 initial + 5 after a scroll/follow-mode
cycle). The viewer auto-split into its own tmux pane next to the source pane;
`tmux pipe-pane -o` mirrored that pane's raw byte stream (including
`TMATH_VIEWER_LOG=1` diagnostic lines and the Kitty APC escape sequences) to
a scratch file for offline parsing. No repository or durable state was
touched by the harness.

## Result: PASS for both AT-3-501 and AT-3-503

### AT-3-501 — one placement per block, no composite buffer, O(new block) append bytes

- `TMATH_VIEWER_LOG=1` diagnostic lines showed `placed blocks=N` incrementing
  by exactly 1 for each of the 40 fed blocks (`placed blocks=1` through
  `placed blocks=40`, `formula_errors=0` throughout), confirming one
  placement operation per block with no batching or coalescing.
- Per-append bytes transmitted (measured as the byte span between
  consecutive `placed blocks=N` log lines, counting Kitty APC chunk
  boundaries): blocks 2 through 40 ranged from 18,197 to 21,121 bytes per
  append (mean ~19,820 bytes), with no upward trend as history grew from 1
  to 39 prior blocks. A second batch of 5 blocks fed later in the session
  (blocks 41-45, after history had grown further and a scroll/follow cycle
  had occurred) measured 18,202-19,817 bytes per append — the same range,
  confirming append cost stays flat as history length increases.
- Parsing the reassembled Kitty APC control data across the full session
  (6,630 complete image transmissions) found only `a=T` (transmit-and-
  display) and `a=d,d=I,i=<id>,q=2` (delete a single image ID, quiet) action
  types. No frame ever used a full-buffer-clear delete (e.g. `d=a`), which
  is the direct evidence against a composite RGBA buffer — deletions always
  target one placement's image ID individually.

### AT-3-503 — scroll re-emission bounded by visible window, cache reuse, no re-render

- Two consecutive batches of 10 wheel-up scroll steps (`drive.sh <pane> up
  10 100`) were injected. Byte deltas measured from the pipe-pane capture:
  first batch 2,366,898 bytes / 10 steps (~236.7 KB/step), second batch
  (deeper into history) 2,338,480 bytes / 10 steps (~233.8 KB/step). The
  two batches are within ~1.2% of each other despite the second batch
  scrolling further back into history, supporting a per-step budget bounded
  by the visible window rather than by history depth.
- Pressing `End` (`drive.sh <pane> end`) produced a `follow=true` log line
  immediately following the `follow=false` line logged at the first manual
  scroll, confirming re-engagement on `End`.
- After follow re-engaged, feeding 5 more blocks produced `placed blocks=41`
  through `placed blocks=45` with `cache_hits` climbing from 1 to 9 (against
  a stable `cache_misses=80`) — blocks that had scrolled out and back into
  view were re-placed from the `RenderCache` rather than re-rendered, which
  is the direct evidence for "re-emits from cached PNGs only," not a fresh
  render.
- No render-pipeline invocation (no new `cache_misses` growth) was observed
  during either scroll batch — only placement/delete APC traffic — meaning
  scrolling did not trigger the renderer subprocess.

## Pixel-level validation (screenshot substitute)

Because on-screen visual inspection was not possible this run (see Method
note), the Kitty graphics payloads themselves were validated as a stronger
substitute: every placement transmits raw RGBA pixels (`f=32`, zlib-
compressed, `o=z`) rather than PNG, sized `s=<width>,v=<height>`. All 6,630
complete image transmissions captured in the session were reassembled
(concatenating multi-chunk `m=1`/`m=0` payloads), zlib-decompressed, and
checked against `width * height * 4`:

- 6,630 / 6,630 decompressed to the exact expected RGBA byte count (0
  failures — no truncated or malformed transmission).
- Dimensions observed ranged from 368x48 to 448x56 pixels per block image.
  Alpha-channel non-transparent pixel ratio (a proxy for "this image has
  visible glyph content, not a blank placeholder") ranged 2.2%-18.0% per
  image (mean 10.2%); zero transmissions had 0% non-transparent pixels.

This confirms every placed block carried real rendered content at the byte
level, though it does not confirm the glyphs are legible/correctly shaped on
screen (a genuine visual check would still catch, e.g., font substitution or
KaTeX/Typst layout bugs that produce non-blank but wrong output).

## Supervisor visual check (addendum, same day)

A separate supervisor-run lab session on the same commit (`97c435c`), in the
same Ghostty + tmux passthrough environment, did capture and review live
screenshots while the terminal-math tmux window was frontmost (6-block and
40-block sessions using the same `viewer-scroll-lab` harness). The review
confirmed: legible, correctly shaped math and mixed Japanese/English text
(no tofu, no blank or misshapen blocks), the status bar fixed on row 1
through appends and scrolling, bottom-aligned fills with no stale rows or
overlaps, and scroll-back reaching the first block's true top row. The
screenshots capture the whole screen and therefore stay local-only per the
repository's privacy rules; they are not committed. This closes the visual
portion of the check for this commit; the limitation below stands for the
byte-capture run itself.

## Limitations

- No screenshot was taken or reviewed during the byte-capture run itself;
  within this run the "no tofu, no stale rows, no overlap" visual claims
  are verified only by the supervisor addendum above, not by an artifact in
  the repository. A follow-up run should establish a reliable
  non-interactive way to bring only the `terminal-math` Ghostty space to
  the front (or use a dedicated, single-project Ghostty window) before
  attempting screenshots again.
- History eviction re-render (AT-3-504, beyond a bounded cache budget) was
  not exercised — the session only reached 45 blocks and two 10-step scroll
  batches, short of forcing eviction. `cache_hits` staying below
  `cache_misses` in this run is consistent with the cache budget not yet
  being exceeded.
- Byte accounting is derived from the pipe-pane capture of the viewer
  pane's own output stream, not an independent packet capture; tmux's own
  passthrough framing overhead is included in the totals but this framing
  is constant per Kitty APC chunk (`\x1bPtmux;\x1b...\x1b\\`) and does not
  change the scaling conclusions.
- The scratch capture file (raw Kitty stream containing only synthetic lab
  content, no real document text) was not committed and was not screenshot;
  it stays local to this session's scratch directory.
