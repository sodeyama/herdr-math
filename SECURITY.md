# Security Policy

## Supported versions

Herdr Math has not published its first release. The `main` branch is a development line and does not yet receive a
published-version support guarantee. This table will be updated when v0.1.0 is released.

| Version | Supported |
|---|---|
| Unreleased development build | Best-effort security fixes |

## Report a vulnerability

Use the repository's private
[GitHub Security Advisory form](https://github.com/sodeyama/herdr-math/security/advisories/new). Do not disclose a
suspected vulnerability in a public issue before a fix is available.

Include a minimal sanitized reproduction, affected commit or version, Herdr version, platform, expected impact,
and whether the issue can expose pane content, execute input, bypass answer boundaries, cross session boundaries,
or modify an unowned pane.

Do not include:

- credentials, tokens, or private keys;
- private coding-agent prompts or responses;
- raw pane history or LaTeX source from private work;
- Herdr state, lock, secret, session, or log files; or
- local usernames, home paths, or unredacted screenshots.

Use synthetic values and stable error codes. If a private artifact is essential, first describe why in the
advisory and wait for a safe transfer method.

## Security boundaries

Herdr Math treats pane text and LaTeX as untrusted. Runtime processing is local and uses no remote service. The
plugin does not execute TeX, shell commands, user JavaScript, input-selected binaries, or remote resources. It
fails closed when it cannot prove the current answer boundary or viewer ownership.

Install-time package and browser downloads are separate from runtime. Review the immutable source ref and manifest
build commands before installing a release.

See [PRIVACY.md](PRIVACY.md), [docs/architecture.md](docs/architecture.md), and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the data and dependency boundaries.

