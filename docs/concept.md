# Herdr Math Concept

## Summary

Herdr Math is a Herdr plugin that presents the visible conclusion of a completed AI-agent response as clean prose and locally rendered math in a side pane.

The plugin is designed for people who use coding agents inside terminal multiplexers and regularly discuss mathematics, statistics, machine learning, optimization, physics, or technical documentation. Those conversations often contain `$...$` and `$$...$$` expressions that remain raw terminal text. The source is useful for copying, but it is slower to read than typeset notation.

Herdr Math keeps the original answer untouched and opens a separate visual surface for the conclusion, including its explanatory message and equations.

## Product Promise

> Stay in the terminal, keep the original answer, and read its conclusion with rendered notation.

The target experience is automatic:

1. A supported agent starts working in a Herdr pane.
2. Herdr Math records a privacy-preserving baseline for that pane.
3. The agent reaches a completed state.
4. Herdr Math isolates only the visible final response, excluding reasoning, tool output, progress, prompts, and terminal chrome.
5. It scans that response for supported math delimiters.
6. If equations exist, it renders the response prose and math locally and updates a right-side viewer.
7. The agent pane keeps focus, and later answers reuse the same viewer.

If no equation exists, nothing opens and the current viewer is not changed.

## Why a Side Pane

The original terminal transcript remains the canonical conversation record. Rewriting terminal output in place would be fragile, would interfere with copy and scrollback behavior, and would couple the plugin to each agent's terminal UI.

A separate pane provides four useful properties:

- The raw answer and the cleanly formatted conclusion remain visible together.
- The source pane does not need to understand images.
- Viewer ownership is explicit and reversible.
- The plugin can update or close its surface without modifying agent output.

The v1 placement is a split pane. Popup and overlay variants are possible experiments, but they are not part of the initial release contract.

## Intended Users

### Primary users

- Developers asking coding agents about algorithms, proofs, numerical methods, or model behavior
- Researchers and students working through equations in Claude Code or Codex
- Data scientists reviewing statistics and optimization formulas in terminal sessions
- Technical writers checking LaTeX emitted by an agent

### Secondary users

- Maintainers testing how Herdr plugins can provide visual companion panes
- Teams that want a local-only math display without a browser service

## Jobs to Be Done

- "When an agent explains a formula, let me understand it without mentally parsing LaTeX."
- "Keep the exact LaTeX available for copying while showing me a readable version."
- "Update the visual automatically when the next answer arrives."
- "Do not send proprietary terminal output to an online renderer."
- "Do not create a new split every time the agent uses math."

## Naming

The product name is **Herdr Math**.

- `Herdr` makes the host environment immediately discoverable.
- `Math` describes the user value rather than one rendering implementation.
- The name remains valid if the project later adds MathML input, accessibility output, or alternative local renderers.

The repository name is `herdr-math`. The public description should include the more specific terms `LaTeX`, `viewer`, `agent responses`, and `side pane` for searchability.

## V1 Functional Scope

### Included

- Herdr plugin installation from GitHub
- Automatic handling of supported agent lifecycle events
- Claude Code and Codex panes detected by Herdr
- Inline math delimited by `$...$`
- Display math delimited by `$$...$$`
- Multiple equations in one completed answer
- Visible final-response prose and equations in source order
- Transparent output with one base size across prose and math
- A reusable viewer split for each source pane
- Local PNG generation
- Safe replacement of the existing viewer image
- Clear diagnostics for missing graphics support
- Bounded, privacy-preserving logs
- Clean recovery after viewer closure and Herdr restart

### Explicitly excluded from v1

- Editing or replacing the source terminal transcript
- Full Markdown rendering
- TeX document compilation
- Shell execution or user macros that can run code
- Remote rendering APIs
- OCR or image-to-LaTeX conversion
- Automatic formula solving or symbolic algebra
- Browser, Ghostty, or editor extensions
- Popup or overlay as the default viewer
- Guaranteed support for every terminal emulator
- Windows support without a dedicated release matrix

## Parsing Philosophy

Terminal answers contain many dollar signs that are not math. Examples include prices, shell variables, and code:

```text
$10 and $20
$HOME and $PATH
echo "$VALUE"
```

Herdr Math therefore uses a small stateful scanner rather than a single regular expression. The scanner skips fenced code, inline code, escaped dollar signs, unclosed delimiters, and obvious shell or price patterns.

The parser is intentionally conservative. A false negative leaves readable source text in place. A false positive can create a misleading equation or cause the renderer to fail. V1 prefers false negatives when syntax is ambiguous.

## Answer-Boundary Philosophy

The plugin must render the current completed answer, not every equation in terminal history.

The target boundary model compares a baseline captured at the start of agent work with a stable read after completion. It may recover a boundary using an exact prefix, a stable prefix, a sliding-window overlap, or a context-qualified tail anchor.

Every recovery strategy must prove that the selected text follows known baseline context. If no strategy can establish that relationship, processing stops without changing the viewer.

This fail-closed rule is more important than rendering every answer.

## Viewer Behavior

The viewer belongs to one source pane.

- First valid math answer: open one split to the right without changing focus.
- Later valid math answer: replace the image in that same viewer.
- Answer without math: leave the viewer unchanged.
- Invalid or oversized math: leave the previous valid image unchanged.
- Viewer closed by the user: recreate one viewer on the next valid math answer.
- Source pane closed: stop retaining ownership state for that source.

The plugin must not take keyboard focus merely to display an image.

## Dependency Boundary

### Direct product dependencies

- A compatible Herdr release with plugin event hooks and pane graphics APIs
- A local JavaScript runtime for the planned v1 implementation
- Locally installed rendering dependencies declared by this repository
- Herdr configuration with experimental Kitty graphics enabled
- An attached terminal path on which Herdr can display those graphics

### Ghostty boundary

Herdr Math is not a Ghostty plugin and does not call Ghostty APIs.

The prototype was tested in Ghostty because Ghostty supports the Kitty graphics protocol and was the available outer terminal. Herdr itself owns the plugin surface and the `pane.graphics.*` calls. Other terminals may work, but each public compatibility claim requires a real smoke test.

The fact that Herdr internally uses a Ghostty-backed terminal engine is an implementation detail of Herdr, not a package dependency owned by Herdr Math.

## Privacy Model

All processing is local by design.

Herdr Math needs temporary access to pane text to identify the latest answer and extract formulas. That data is treated as sensitive:

- Raw pane text is held in memory only for the shortest practical time.
- Baseline state stores content hashes and structural metadata, not transcripts.
- Logs never include answer or equation text.
- Rendering does not fetch remote fonts, CSS, scripts, images, or APIs.
- The plugin does not include telemetry in v1.

The plugin cannot make the host terminal or Herdr private by itself. Users must still trust Herdr, the installed agent, local dependencies, and any terminal logging they enable.

## Security Model

LaTeX-like input is untrusted text.

The renderer must:

- Use a non-executable math parser rather than a TeX engine
- Disable trusted links and remote resources
- Enforce formula-count and length limits before rendering
- Enforce a wall-clock timeout
- Enforce image dimension and byte limits
- Avoid shell interpolation, dynamic code evaluation, and subprocess execution
- Return stable error codes without exposing input text

The graphics update is transactional from the user's perspective: the new image is validated before `pane.graphics.set` replaces the existing image.

## Error Philosophy

Errors fall into three categories:

1. **User-input rejection**: invalid LaTeX, ambiguous boundary, or configured limits. Keep the previous viewer and emit a bounded diagnostic.
2. **Capability failure**: Kitty graphics disabled, no attached client cell size, incompatible Herdr version, or missing runtime dependency. Explain the corrective action and do not retry in a tight loop.
3. **Transient runtime failure**: event duplication, a viewer closed during update, socket interruption, or renderer timeout. Recover idempotently on the next event.

The plugin should never terminate the agent, write into the agent pane, or close a user-owned pane in response to its own error.

## Compatibility Policy

Compatibility statements are evidence-based.

- `min_herdr_version` is set to the oldest version validated against the exact manifest fields and socket methods used by the release.
- Platform declarations are limited to platforms with a completed release matrix.
- Terminal documentation distinguishes `verified`, `expected`, and `unsupported`.
- An outer terminal that supports Kitty graphics in general is not automatically considered compatible through Herdr.
- Remote and direct-attach behavior is not claimed until graphics have been tested in those paths.

The first public version may remain macOS-only if that is the only fully verified runtime. International availability does not require overstating platform support.

## Product Success Criteria

V1 is successful when a new user can:

1. Install the plugin from a tagged GitHub revision using one Herdr command.
2. Enable the documented Herdr graphics setting.
3. Start a supported agent inside Herdr.
4. Receive a math answer and see its clean final message and rendered equations in one companion pane.
5. Continue the conversation and see that pane update without focus loss or split duplication.
6. Inspect clear local diagnostics when the environment is unsupported.
7. Uninstall the plugin without losing unrelated Herdr configuration or panes.

Engineering success also requires zero raw transcript content in plugin logs and successful recovery after invalid input, timeout, viewer closure, and server restart.

## Future Directions

Future versions may evaluate:

- MathML input
- SVG-native rendering
- Copy actions for equation source
- Accessible text descriptions
- User-configurable foreground colors
- User-controlled placement and sizing
- Per-agent or per-workspace configuration
- Linux and Windows support
- Remote Herdr sessions
- Popup and overlay placement

These are not v1 commitments. Each requires a specification update and compatibility evidence.

## Terminology

- **Source pane**: the Herdr pane containing the AI agent.
- **Viewer pane**: the Herdr-managed plugin pane that displays the rendered image.
- **Baseline**: the known pane state captured when an agent begins working.
- **Completion**: a transition to `done` or `idle` that may contain a finished answer.
- **Answer delta**: text proven to have appeared after the baseline.
- **Graphics layer**: the image owned by Herdr on a viewer pane through `pane.graphics.set`.

## Primary References

- [Herdr plugins](https://herdr.dev/docs/plugins/)
- [Herdr Socket API](https://herdr.dev/docs/socket-api/)
- [Herdr marketplace](https://herdr.dev/docs/marketplace/)
- [Ghostty features](https://ghostty.org/docs/features)
