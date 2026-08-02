# Terminal Math Concept

## Summary

Terminal Math (`tmath`) is a standalone terminal renderer: it takes Markdown with `$...$` and
`$$...$$` math and renders the result as a transparent image placed into the terminal with the
Kitty graphics protocol. The image is anchored to real terminal cells, so it scrolls with the
shell's scrollback instead of floating over the viewport. Mouse wheel and keyboard both scroll a
tall rendered document.

It runs from a plain terminal in any Kitty-graphics-capable outer terminal such as Ghostty,
kitty, or WezTerm. There is no plugin runtime, no browser, and no network rendering service.

## Product Promise

> Render Markdown and LaTeX as scrollable images, in your terminal, locally.

The target experience:

1. You run `tmath render ./notes.md` (or pipe a document on stdin).
2. Terminal Math renders the allowlisted Markdown and math into a transparent PNG locally.
3. It transmits the image into the main terminal buffer at the cursor row.
4. The image is glued to real cells, so terminal scrollback carries it.
5. Mouse wheel and keyboard controls scroll the rendered document.

## Why Scrollback-Anchored Images

Rewriting terminal output in place would be fragile and would interfere with copy, scrollback,
and the shell. Placing an image over real cells gives three useful properties:

- The shell transcript stays the canonical record; the image is attached to cells.
- Scrolling the terminal naturally scrolls the image with the content around it.
- The renderer never needs to own the application screen, so there is no alternate-screen
  takeover and no focus loss.

The v1 placement is one scrollback-anchored virtual placement (`U=1,c,r`) with a placeholder
grid per document block.

## Intended Users

- Developers and researchers who want to read `$...$` formulas as typeset notation in the
  terminal.
- Technical writers and students reviewing Markdown documents containing math.
- Anyone who wants a local-only LaTeX/Markdown preview without a browser service.
- Maintainers building on the Kitty graphics protocol for plain terminals.

## Jobs to Be Done

- "When I have a Markdown file with formulas, let me read them without mentally parsing LaTeX."
- "Keep the exact LaTeX available for copying while showing me a readable version."
- "Let the rendered document live in my terminal scrollback, not in a separate window."
- "Do not send my document to an online renderer."
- "Let me scroll a long rendered document with the mouse wheel or the keyboard."

## Naming

The binary is **`tmath`** (short for terminal math). The product name is **Terminal Math**. The
repository is kept at `sodeyama/herdr-math`; the product identity no longer references the Herdr
plugin runtime. The homepage describes the tool as "Render Markdown and LaTeX as scrollable
terminal images."

## Functional Scope

### Included

- `tmath render <file | ->` over a file or stdin with bounded reads.
- `tmath diagnose` for local capability checks (renderer subprocess, node, stdout terminal,
  Kitty graphics probe).
- `tmath agent` / `tmath agent-viewer` (P1, experimental): watch a tmux pane running a coding
  agent and show each finished answer as rendered Markdown + math in a viewer pane.
- `tmath --help` / `tmath --version`.
- Inline math delimited by `$...$` and display math by `$$...$$`; `\(...\)` and `\[...\]`
  retained.
- Strict allowlisted Markdown subset: headings, emphasis, lists, quotes, pipe tables, fenced and
  inline code, inert links.
- One scrollback-anchored placement per rendered block, glued to real scrollback cells.
- Placeholder grid so images scroll with the shell scrollback.
- Mouse wheel and keyboard scroll states with a smooth easing profile.
- Local PNG generation through KaTeX and the browser pipeline.
- `--content-width <px>` and `--font-size <px>` composition options.
- Bounded, privacy-preserving logs and stable error records.

### Explicitly excluded

- Editing or replacing the shell transcript.
- General Markdown rendering beyond the allowlisted subset; no user HTML, CSS, color
  directives, images, or scripts.
- TeX document compilation.
- Shell execution or user macros that can run code.
- Remote rendering APIs or telemetry.
- Ghostty-specific APIs, plugins, or configuration.
- A Herdr plugin runtime, socket, or manifest.
- Guaranteed support for every terminal emulator.
- Windows support without a dedicated release matrix.

## Parsing Philosophy

Documents contain many dollar signs that are not math. Examples include prices, shell variables,
and code:

```text
$10 and $20
$HOME and $PATH
echo "$VALUE"
```

Terminal Math therefore uses a small stateful scanner rather than a single regular expression.
The scanner skips fenced code, inline code, escaped dollar signs, unclosed delimiters, and
obvious shell or price patterns.

The parser is intentionally conservative. A false negative leaves readable source text in
place. A false positive can create a misleading equation or cause the renderer to fail. V2
prefers false negatives when syntax is ambiguous.

## Placement and Input Model

- The Rust CLI owns the terminal: raw mode, Kitty negotiation, mouse/keyboard input, the scroll
  state machine, and transmission of placements into the main buffer.
- The TypeScript renderer subprocess is one-shot: stdin request in, stdout response out, then
  exit. It never stays alive.
- Placements are tracked with image ids; replacement issues a scoped delete before re-transmit,
  and removal deletes only that image.
- `q` and `Ctrl-C` always reset the terminal; any exit path restores it.

## Dependency Boundary

### Direct product dependencies

- A Rust toolchain for the terminal frontend.
- A Node.js runtime and the declared npm packages for the one-shot renderer.
- A Kitty-graphics-capable terminal (Ghostty is the verified primary).

### Ghostty boundary

Terminal Math is not a Ghostty plugin and does not call Ghostty APIs. Ghostty is one verified
outer terminal because it supports the Kitty graphics protocol; kitty and WezTerm are P1 until
the same matrix passes. The tool works with any compatible terminal.

## Privacy Model

All processing is local by design.

- Document text is held in memory only for the duration of the render, then discarded.
- Logs and diagnostics never include document or formula text.
- Rendering does not fetch remote fonts, CSS, scripts, images, or APIs.
- The tool has no telemetry.

The tool cannot make the host terminal private by itself. Users must still trust the outer
terminal and any logging the terminal enables.

## Security Model

LaTeX-like input is untrusted text.

The renderer must:

- Use a non-executable math parser (KaTeX) rather than a TeX engine, with a trust policy
  equivalent to `trust: false`.
- Disable remote resources and inert links.
- Enforce formula-count, per-formula, aggregate, scan-byte, and placement limits before
  rendering.
- Enforce a wall-clock timeout.
- Enforce image dimension and byte limits.
- Avoid shell interpolation, dynamic code evaluation, and user-driven subprocess execution.
- Return stable error codes without exposing input text.

The placement is fail-closed: invalid input, timeouts, and payload rejection leave earlier
valid placements intact and never emit an uncertain image.

## Error Philosophy

Errors fall into three categories:

1. **User-input rejection**: invalid LaTeX or configured limits. Keep earlier placements and
   emit a bounded diagnostic.
2. **Capability failure**: no Kitty graphics support, no terminal for stdout, or a missing
   renderer dependency. `tmath diagnose` explains the corrective action.
3. **Transient runtime failure**: a render timeout or subprocess failure. Fail closed and do not
   retry in a tight loop.

## Compatibility Policy

Compatibility statements are evidence-based.

- macOS is the primary target. Linux and Windows are post-V2 (P1/P2) and are not claimed until
  a release matrix passes.
- Terminal documentation distinguishes `verified`, `expected`, and `unsupported`.
- An outer terminal that supports Kitty graphics in general is not automatically considered
  compatible; each claim requires a real smoke test.

## Product Success Criteria

V2 is successful when a new user can:

1. Install `tmath` from a tagged revision.
2. Run `tmath render ./notes.md` in Ghostty.
3. See the rendered document anchored in the scrollback with equations typeset.
4. Scroll the terminal and watch the image move with the content.
5. Scroll the rendered document with the mouse wheel and the keyboard.
6. Inspect clear local diagnostics when the terminal lacks Kitty graphics.

Engineering success also requires zero raw document content in logs, and recovery after invalid
input, timeout, and missing capabilities.

## Future Directions

Future versions may evaluate:

- `tmath agent` watching a coding agent's pane and showing its answers in a viewer pane
  (P1, in progress as Phase 8).
- `tmath watch <file>` re-rendering on change.
- `tmath ls` listing active placements.
- Linux and Windows support.
- Additional verified terminals.
- Shared-memory/file media to avoid pushing large payloads through a pipe.

These are not V2 commitments. Each requires a specification update and compatibility evidence.

## Terminology

- **Placement**: an image transmitted through the Kitty graphics protocol and anchored to real
  terminal cells.
- **Placeholder grid**: the per-cell combining-character text that glues a virtual placement to
  scrollback cells.
- **Scroll driver**: the module mapping wheel deltas and fallback keys through a smoothing state
  machine.
- **Render subprocess**: the one-shot TypeScript process that turns a bounded JSON request into
  a transparent PNG.

## Primary References

- [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [xterm controls](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Ghostty](https://ghostty.org/docs/features)
