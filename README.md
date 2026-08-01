# Herdr Math

Render LaTeX from AI agent responses in a side pane.

Herdr Math is a [Herdr](https://herdr.dev/) plugin. It isolates the visible conclusion of a completed coding-agent response, excludes reasoning and tool output, and presents its prose and `$...$` or `$$...$$` equations in one reusable viewer pane without taking focus from the agent.

## Status

Version 0.1.0 is a development build, not a published release. The implementation, automated suite, real Herdr runtime, compatibility gates, public maintenance files, and CI are complete. Sanitized screenshots and immutable-tag installation remain release work.

Verified release-candidate environment:

- Herdr 0.7.5, protocol 17
- macOS arm64
- Ghostty 1.3.1
- Claude Code, Codex CLI, Pi, and OpenCode

See [Compatibility](docs/compatibility.md) for exact versions and unverified combinations.

## Development installation

Herdr local linking expects an already-built checkout:

```sh
npm ci
npm run audit:browser
npm run build
herdr plugin link /path/to/herdr-math --enabled
```

Enable Herdr's experimental graphics support, then reload the server configuration:

```toml
[experimental]
kitty_graphics = true
```

```sh
herdr server reload-config
```

The future tagged installation command is documented in [Getting started](docs/getting-started.md), but it is not valid until the v0.1.0 tag is published.

## Use

Install the Herdr integration for each coding agent you use:

```sh
herdr integration install claude
herdr integration install codex
herdr integration install pi
herdr integration install opencode
herdr integration status
```

Run a supported agent in Herdr and complete a response containing inline or display LaTeX. Herdr Math opens one `Math` pane to the right with the final message and rendered equations on a transparent background. Long responses scroll automatically to the bottom. Later valid answers replace the image in that pane. Answers without math and rejected updates leave the previous image unchanged.

Run privacy-safe diagnostics from a Herdr pane:

```sh
herdr plugin action invoke diagnose --plugin io.github.sodeyama.herdr-math
```

## Safety

- Pane text and equations stay local.
- Durable state contains keyed fingerprints, not transcripts or LaTeX source.
- Rendering uses local KaTeX and Chromium assets with remote loading and trusted links disabled.
- The plugin does not execute TeX, shell input, user JavaScript, or remote resources.
- Uncertain answer boundaries fail closed instead of rendering historical pane content.

## Documentation

- [Getting started and troubleshooting](docs/getting-started.md)
- [Compatibility](docs/compatibility.md)
- [Concept and product boundaries](docs/concept.md)
- [Architecture](docs/architecture.md)
- [Documentation index](docs/README.md)
- [Acceptance tests](specs/herdr-math-v1/tests/main.md)
- [Privacy](PRIVACY.md)
- [Security](SECURITY.md)
- [Support](SUPPORT.md)
- [Contributing](CONTRIBUTING.md)
- [Changelog](CHANGELOG.md)

## Development

```sh
npm ci
npm run check
npm test
npm run test:integration
npm run build
npm run smoke:render
```

Read [AGENTS.md](AGENTS.md) before contributing. Public documentation, code comments, logs, commits, and release material are written in English.

Herdr Math is licensed under the [MIT License](LICENSE). Third-party runtime notices are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
