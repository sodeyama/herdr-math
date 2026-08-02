# Cursor Support, Delimiter Expansion, and Interactive Viewer Scroll — Work Status

Date: August 2, 2026

## Purpose

This note records the working-tree state and commit status for the August 2 session that added Cursor agent support,
expanded LaTeX delimiters, added directory scoping, and reworked the viewer into an interactive scrollable history.
It is a status record, not release evidence. No prompt, response text, LaTeX source, agent session value, local path,
or screenshot is included.

## Commit and push status

- Branch: `main`
- Last pushed commit: `869d802` (`fix(renderer): prevent line overlap and raise default font to 13px`)
- Unpushed commits: none (`origin/main...HEAD` is 0 ahead, 0 behind)
- **All work below is currently UNCOMMITTED in the working tree.** Nothing in this note has been committed or pushed.

Working-tree summary at the time of writing: 37 files changed, roughly 780 insertions and 190 deletions, plus new
untracked files under `src/config/`, `docs/config.example.json`, and two new unit specs.

## Logical changes in the working tree (not yet committed)

The working tree mixes several logical changes. Per the repository commit discipline they must be split into separate
focused commits before pushing. The intended split is:

1. **Cursor agent support**
   - Adds `cursor` to `SupportedAgent`, the `integration_hook` authority map, fingerprint/state/lifecycle agent lists,
     and the agent-status worker `cursor_plain` snapshot mode.
   - Adds Cursor chrome handling (tool activity, status bar, footer, answer marker) in final-response extraction.
   - Fixtures and contract/integration/unit tests updated for the new agent.

2. **LaTeX delimiter expansion**
   - Scanner recognizes `\(...\)` (inline) and `\[...\]` (display) in addition to `$...$` and `$$...$$`.
   - `Formula` gains an optional `delimiter` tag; renderer document validation accepts the new delimiters.
   - New answer-corpus cases for the paren/bracket forms.

3. **Directory scoping and pane working-directory resolution**
   - New `src/config/` module (`plugin-config.ts`, `directory-scope.ts`) with `allowed_directories` support and a
     `directory_out_of_scope` outcome.
   - `resolvePaneWorkingDirectory` prefers pane `cwd` over `foregroundCwd` (Cursor runs the agent from a temporary
     foreground directory while the pane `cwd` is the real project directory).
   - Diagnostics surface the scoping outcome; example config added.

4. **Codex/Claude leading prompt echo fix**
   - Final-response tail trimming skips a leading `›`/`❯` prompt echo before extracting multiline display math so a
     `$$` answer is not rejected as `conclusion_boundary_failed`.

5. **Interactive viewer scroll (largest change)**
   - Viewer presenter reworked from multi-frame auto-scroll animation to a single stacked image placed once with
     `viewport_row` controlling the visible window. New responses are appended below and the view resets to the bottom.
   - `ViewerPresenter.scrollBy` moves the visible window with clamping; placement math keeps natural `grid_rows` and a
     negative `viewport_row` for the overflow.
   - Socket client gains a persistent `events.subscribe` (`pane.scroll_changed`) subscription plus scroll event types.
   - Viewer runtime wires the subscription and exposes the presenter; `viewer.ts` reads raw keyboard input
     (arrow keys, `j`/`k`, `PgUp`/`PgDn`, `g`/`G`) to drive `scrollBy`.
   - Integration tests rewritten for the single-frame placement, scroll clamping, and bottom reset semantics.

## Verification so far

- `npm run build` passes.
- `npm test`: 46 files passed (358 tests) after the rework. One transient failure in
  `tests/integration/adversarial-protocol.spec.ts` appeared under full-suite parallelism but passes in isolation and
  on rerun; no source change resulted from it.

## Known limitations discovered (must be documented before release)

- Herdr's `pane.graphics.set` host path places plugin images with `scrollback_offset: 0` (see
  `src/kitty_graphics.rs` in Herdr). Plugin images therefore do **not** track terminal text scroll, and
  `pane.scroll_changed` does not fire for a graphics-only viewer pane (no scrollback buffer). Confirmed live: mouse
  wheel produces no event on the viewer pane.
- Because mouse/trackpad scroll cannot be observed for the viewer, scrolling is driven by keyboard input while the
  viewer pane is focused. Mouse-follows-scroll is not achievable with the current 0.7.5 socket API.
- Very tall stacked histories make individual formulas small when the whole image must fit; this needs a width-fit
  rendering policy decision before release.

## Required follow-ups before commit/push

1. Split the working tree into the five logical commits above (English Conventional Commit subjects).
2. Update `specs/herdr-math-v1/tasks/main.md` and relevant spec/plan docs for Cursor support, new delimiters,
   directory scoping, and the scroll interaction change, in separate documentation commits.
3. Update `docs/architecture.md` presenter section (single-frame scroll model replaces the animation description) and
   `README.md`/`docs/getting-started.md` for the keyboard scroll controls and Cursor support claims.
4. Record real runtime evidence for Cursor rendering and keyboard scroll in a new evidence file once visually verified.
5. Do not mark any release-gate task complete from this note; acceptance requires passing acceptance tests with evidence.
