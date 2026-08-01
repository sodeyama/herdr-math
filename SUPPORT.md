# Support

Herdr Math has not published v0.1.0. Support for the development build is best effort.

## Where to ask

- Use [GitHub Issues](https://github.com/sodeyama/herdr-math/issues) for reproducible bugs and focused feature
  requests.
- Use the private process in [SECURITY.md](SECURITY.md) for security or privacy vulnerabilities.
- Use the official [Herdr documentation](https://herdr.dev/docs/) for host installation, configuration, and general
  Herdr behavior.

Before opening an issue, run:

```sh
herdr plugin action invoke diagnose --plugin io.github.sodeyama.herdr-math
```

Include only the allowlisted diagnostic result, Herdr Math commit or version, Herdr version, coding agent and
integration version, macOS architecture, outer terminal version, stable error code, and a synthetic reproduction.

Do not post pane output, private prompts or answers, equations from private work, agent session values, credentials,
state files, full logs, home paths, or unredacted screenshots.

## Compatibility boundary

The verified v0.1 release candidate is Herdr 0.7.5 on macOS arm64 with Ghostty 1.3.1. Other platforms,
architectures, terminals, and remote attach graphics are outside the current support claim. See
[docs/compatibility.md](docs/compatibility.md).

There is no response-time or resolution-time service-level agreement.

