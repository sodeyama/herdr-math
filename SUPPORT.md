# Support

Terminal Math has not published `0.2.0`. Support for the development build is best effort.

## Where to ask

- Use [GitHub Issues](https://github.com/sodeyama/herdr-math/issues) for reproducible bugs and
  focused feature requests.
- Use the private process in [SECURITY.md](SECURITY.md) for security or privacy vulnerabilities.
- Use the [Kitty graphics protocol documentation](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
  for terminal behavior.

Before opening an issue, run:

```sh
./target/debug/tmath diagnose
```

Include only the allowlisted diagnostic result, Terminal Math commit or version, terminal
version, macOS architecture, stable error code, and a synthetic reproduction.

Do not post private documents, equations from private work, credentials, state files, full logs,
home paths, or unredacted screenshots.

## Compatibility boundary

The verified target is macOS arm64 with Ghostty 1.3.1. Other platforms, architectures, and
terminals (kitty, WezTerm) are outside the current support claim until evidence is recorded. See
[docs/compatibility.md](docs/compatibility.md).

There is no response-time or resolution-time service-level agreement.
