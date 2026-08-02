# Contributing to Terminal Math

Thank you for helping improve Terminal Math. The project accepts focused bug fixes, tests,
documentation, and design proposals that preserve its local-only and fail-closed behavior.

## Before starting

- Read [AGENTS.md](AGENTS.md), the [acceptance tests](specs/terminal-math-v2/tests/main.md), and
  the [architecture](docs/architecture.md).
- Search existing issues and pull requests before opening duplicate work.
- Open an issue before changing a public CLI contract, platform claim, renderer backend, security
  boundary, or protocol version.
- Never attach private documents, LaTeX from private work, credentials, or local state files.

## Development setup

The verified development environment uses Node.js 22 or later and a recent Rust toolchain on
macOS arm64:

```sh
npm ci
npm run audit:browser
npm run build
cargo build
```

Run the complete local validation surface before requesting review:

```sh
npm run check
npm test
npm run test:integration
npm run build
npm run smoke:render
cargo test
cargo clippy --all-targets
```

Use synthetic fixtures. A fixture may contain non-English text only when the test explicitly
checks Unicode or multilingual behavior.

## Change design

- Keep the Kitty escapes, terminal init, mouse/input decoders, scroll driver, placement tracker,
  scanner, renderer, and CLI as separate modules with narrow interfaces.
- Do not add shell execution, TeX binaries, dynamic imports from input, trusted KaTeX links,
  remote resources, or telemetry without an approved threat-model and specification change.
- Keep runtime artifacts out of the repository.
- Do not log document text, pane output, LaTeX source, full events, environment dumps, or
  arbitrary exceptions.
- Update documentation with any public command, config key, compatibility claim, limit, error
  code, or protocol change.

## Commits and pull requests

Use English Conventional Commit subjects, one logical change per commit, and reviewable commit
history. Keep implementation, refactoring, and specification progress updates separate. Pull
requests should include:

- acceptance-test ids;
- commands run and their results;
- runtime evidence when behavior depends on terminal graphics;
- compatibility scope;
- privacy and security impact; and
- remaining limitations.

A failed, skipped, retried, or unimplemented acceptance case is not a pass.

## Reporting security issues

Do not open a public issue for a suspected vulnerability. Follow [SECURITY.md](SECURITY.md).
