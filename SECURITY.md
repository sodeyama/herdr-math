# Security Policy

## Supported versions

Terminal Math has not published its first standalone release. The `main` branch is a development
line and does not yet receive a published-version support guarantee. This table will be updated
when `0.2.0` is released.

| Version | Supported |
|---|---|
| Unreleased development build | Best-effort security fixes |

## Report a vulnerability

Use the repository's private
[GitHub Security Advisory form](https://github.com/sodeyama/terminal-math/security/advisories/new).
Do not disclose a suspected vulnerability in a public issue before a fix is available.

Include a minimal sanitized reproduction, affected commit or version, platform, terminal,
expected impact, and whether the issue can expose document content, execute input, break
placements, or cross the fail-closed boundary.

Do not include:

- credentials, tokens, or private keys;
- private documents or LaTeX source;
- local usernames, home paths, or unredacted screenshots.

Use synthetic values and stable error codes. If a private artifact is essential, first describe
why in the advisory and wait for a safe transfer method.

## Security boundaries

Terminal Math treats document text and LaTeX as untrusted. Runtime processing is local and uses
no remote service. It does not execute TeX, shell commands, user JavaScript, input-selected
binaries, or remote resources. It fails closed when input is invalid, exceeds a limit, or the
terminal lacks Kitty graphics, leaving earlier placements intact.

Install-time npm and Playwright package and browser downloads are separate from runtime. Review
the locked dependencies and the built render subprocess path before running.

See [PRIVACY.md](PRIVACY.md), [docs/architecture.md](docs/architecture.md), and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the data and dependency boundaries.
