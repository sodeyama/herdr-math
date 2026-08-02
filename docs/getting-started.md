# Getting Started

Herdr Math 0.1.0 is currently a development build. The local-link procedure below is verified. The tagged GitHub
installation and managed reinstall/uninstall procedures are documented in advance but remain release gates until
an immutable v0.1.0 tag exists.

## Requirements

- Herdr 0.7.5 or later with protocol 17 compatibility
- macOS arm64 for the verified v0.1 release candidate
- Node.js 22 or later and npm
- Ghostty 1.3.1 for the verified outer-terminal path
- Herdr's experimental Kitty graphics setting
- A supported coding agent: Claude Code, Codex CLI, Cursor, Pi, or OpenCode

Herdr Math does not call Ghostty APIs. Other terminals may work, but they are not verified for v0.1.

## Tagged installation after release

This command becomes valid only after the repository publishes the immutable `v0.1.0` tag:

```sh
herdr plugin install sodeyama/herdr-math --ref v0.1.0
```

Herdr previews the manifest build commands before installation. Review the commands and source ref before
confirming. Do not install an untagged branch as a substitute for the release gate.

## Verified local development link

Clone the repository, enter the checkout, and run:

```sh
npm ci
npm run audit:browser
npm run build
herdr plugin link /path/to/herdr-math --enabled
herdr plugin list --plugin io.github.sodeyama.herdr-math
```

`npm ci` installs the locked dependencies and the plugin-local Chromium headless shell. Local linking registers
the built checkout; Herdr does not run the manifest build commands for `plugin link`.

## Configure graphics

Add or update this section in the Herdr configuration:

```toml
[experimental]
kitty_graphics = true
```

Validate and reload it:

```sh
herdr config check
herdr server reload-config
```

## Limit to specific directories

By default Herdr Math runs in every supported agent pane. To restrict it to one or more project roots, create
`config.json` in the plugin config directory:

```sh
herdr plugin config-dir io.github.sodeyama.herdr-math
```

Example `config.json`:

```json
{
  "allowed_directories": [
    "/Users/you/docs/obsidian"
  ]
}
```

Copy [docs/config.example.json](config.example.json) as a starting point. Each entry must be an absolute path.
Herdr Math compares the pane working directory against those roots and ignores panes outside them. Omit the file,
or use an empty `allowed_directories` array, to keep the default unrestricted behavior.

## Install coding-agent integrations

Install only the agents you use:

```sh
herdr integration install claude
herdr integration install codex
herdr integration install cursor
herdr integration install pi
herdr integration install opencode
herdr integration status
```

The verified minimum integration versions are Claude Code v7, Codex v6, Cursor v1, Pi v6, and OpenCode v9. Herdr reports
whether each installed integration is current.

## First use

1. Start Herdr in Ghostty.
2. Start or focus a supported coding agent in a Herdr pane.
3. Ask the agent for an answer containing `$...$` or `$$...$$` LaTeX.
4. Wait for the agent response to complete.
5. Herdr Math opens one `Math` split to the right with the final message and rendered equations, while keeping focus on the source agent. A long response scrolls automatically and stops at the bottom.
6. Complete another formula response to replace the response image in the same viewer.

Herdr Math processes completed responses only. It does not render partial streaming output. Closing the viewer is
safe; the next valid formula response recreates one owned viewer.

## Diagnose

Run the action from a Herdr pane:

```sh
herdr plugin action invoke diagnose --plugin io.github.sodeyama.herdr-math
```

Diagnostics report only allowlisted versions, capabilities, statuses, counts, and stable error codes. They do not
print pane text, equations, environment contents, or local paths.

Common results:

- `graphics_disabled`: set `kitty_graphics = true`, run `herdr config check`, and reload the server configuration.
- `cell_size_unavailable`: reattach Herdr from a graphics-capable client and run diagnostics again.
- `terminal_unverified`: the client may work, but it is outside the verified terminal matrix.
- `viewer_not_open`: informational before the first successful formula response.
- `baseline_missing`: the completion had no proven preceding working baseline; complete a new agent turn.
- `boundary_failed`: the current answer could not be separated safely from pane history; no historical content was rendered.
- `invalid_latex`: the answer contained rejected syntax; the previous valid image remains.
- `scanner_input_limit` or `image_too_large`: reduce the number or size of formulas.
- `renderer_timeout`: the render exceeded the bounded deadline; a later valid response may retry normally.

If the browser executable is missing in a development checkout, run `npm ci` again. Do not use
`npm ci --ignore-scripts`; it skips the plugin-local browser installation. `npm run install:browser` repairs only
the locked browser artifacts, and `npm run audit:browser` verifies them.

## Update and reinstall

For a local development link, update the checkout explicitly and rebuild it before relinking:

```sh
git pull --ff-only
npm ci
npm run audit:browser
npm run build
herdr plugin link /path/to/herdr-math --enabled
```

After release, reinstall the same immutable tag with the same `herdr plugin install` command. Same-tag managed
reinstallation remains unverified until the release-candidate install test is recorded.

## Uninstall or unlink

Remove a local development registration without deleting the user-owned checkout:

```sh
herdr plugin unlink io.github.sodeyama.herdr-math
```

For a future managed tagged installation, use:

```sh
herdr plugin uninstall io.github.sodeyama.herdr-math
```

Herdr 0.7.5 local unlink retains the user-owned checkout and plugin config/state directories. Managed-checkout
removal and config/state retention will be finalized by the immutable-tag uninstall test before release.

## Known limits

- Only `$...$` and `$$...$$` math delimiters are parsed, and only the allowlisted Markdown subset (headings, emphasis, lists, quotes, tables, code blocks, inert links) is rendered by a local parser. Raw HTML, images, scripts, custom CSS, and color directives are not supported.
- Only the visible final response is presented; reasoning, tool output, progress, prompts, and terminal chrome are excluded or cause a fail-closed result when their boundary is uncertain.
- The response background is transparent, while custom foreground colors are not configurable in v0.1.
- Formulas in code spans, fenced code, prices, shell variables, and ambiguous delimiter runs are rejected.
- Strict formula count, source length, image dimension, byte, and time limits apply.
- Previous images remain on invalid input, limits, timeout, or graphics failure.
- Raw answer text and LaTeX source are never written to durable plugin state or logs.
- macOS arm64 with Ghostty is the only verified v0.1 terminal combination.
- macOS x64, Linux, Windows, other terminals, and remote attach graphics are unverified.
- The v0.1 viewer is an image pane; accessible math text, copying, popup placement, and overlays are future work.

See [Compatibility](compatibility.md), [Architecture](architecture.md), and the official
[Herdr plugin documentation](https://herdr.dev/docs/plugins/) for more detail.
