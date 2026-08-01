# Contributing to Herdr Math

Thank you for helping improve Herdr Math. The project accepts focused bug fixes, tests, documentation, and design
proposals that preserve its local-only and fail-closed behavior.

## Before starting

- Read [AGENTS.md](AGENTS.md), the [acceptance tests](specs/herdr-math-v1/tests/main.md), and the
  [architecture](docs/architecture.md).
- Search existing issues and pull requests before opening duplicate work.
- Open an issue before changing a public lifecycle contract, platform claim, renderer backend, security boundary,
  or persistent state schema.
- Never attach private pane output, agent transcripts, LaTeX from private work, credentials, or local state files.

## Development setup

The verified development environment uses Node.js 22 or later on macOS arm64:

```sh
npm ci
npm run audit:browser
npm run build
```

Run the complete local validation surface before requesting review:

```sh
npm run check
npm test
npm run test:integration
npm run build
npm run smoke:render
```

Use synthetic fixtures. A fixture may contain non-English text only when the test explicitly checks Unicode or
multilingual behavior.

## Change design

- Keep the scanner, boundary detector, renderer, Herdr client, event worker, state store, and viewer lifecycle as
  separate modules with narrow interfaces.
- Do not add shell execution, TeX binaries, dynamic imports from input, trusted KaTeX links, remote resources, or
  telemetry without an approved threat-model and specification change.
- Store durable runtime data only under Herdr-provided config and state directories.
- Do not log answer text, pane output, LaTeX source, full events, environment dumps, or arbitrary exceptions.
- Update documentation with any public command, config key, compatibility claim, limit, error code, or lifecycle
  change.

## Commits and pull requests

Use English Conventional Commit subjects, one logical change per commit, and reviewable commit history. Keep
implementation, refactoring, and specification progress updates separate. Pull requests should include:

- acceptance-test ids;
- commands run and their results;
- runtime evidence when behavior depends on Herdr or terminal graphics;
- compatibility scope;
- privacy and security impact; and
- remaining limitations.

The pull request template contains the required checklist. A failed, skipped, retried, or unimplemented acceptance
case is not a pass.

## Reporting security issues

Do not open a public issue for a suspected vulnerability. Follow [SECURITY.md](SECURITY.md).

