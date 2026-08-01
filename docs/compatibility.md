# Compatibility

This document separates the verified v0.1 release-candidate scope from combinations that remain unverified. It
does not claim that the plugin has been released.

## Verified release candidate

| Dimension | Verified value |
|---|---|
| Herdr | 0.7.5, protocol 17 |
| Operating system | macOS 26.5.2 |
| Architecture | arm64 |
| Outer terminal | Ghostty 1.3.1 stable |
| Node.js | 22.21.1 |
| Claude Code | 2.1.220, Herdr integration v7 |
| Codex CLI | 0.146.0, Herdr integration v6 |
| Pi | 0.83.0, Herdr integration v6 |
| OpenCode | 1.18.10, Herdr integration v9 |

This combination passed fresh dependency installation, native artifact inspection, build, local rendering,
agent lifecycle, boundary detection, viewer creation and replacement, focus preservation, resize, failure
preservation, graphics-disabled diagnostics, named-session isolation, stale-lock cleanup, and server restart.

Herdr Math uses Herdr's plugin and pane graphics APIs. It does not call Ghostty APIs. Ghostty is the first verified
outer terminal, not a direct package dependency.

## Required runtime capability

- `herdr-plugin.toml` requires Herdr 0.7.5 or later and declares the `macos` platform.
- Herdr's experimental Kitty graphics setting must be enabled.
- The attached client must provide non-zero cell dimensions and a working graphics path.
- Node.js 22 or later is required by the package metadata.
- The four coding agents require the Herdr integration versions listed above or a later compatible version.

If graphics are disabled, diagnostics report `graphics_disabled` and show the configuration action. If the
attached client does not provide usable cell dimensions, diagnostics report `cell_size_unavailable` and suggest
reattaching a compatible client. Neither result means that Ghostty must be installed.

See the official [Herdr plugin documentation](https://herdr.dev/docs/plugins/) and
[socket API documentation](https://herdr.dev/docs/socket-api/) for the host lifecycle and graphics contract.

## Unverified and unsupported claims

The following combinations are not verified for v0.1 and must not be described as supported:

- macOS x64;
- Linux and Windows, which are absent from the manifest;
- Kitty, WezTerm, and other outer terminals;
- remote Herdr attach graphics;
- Herdr versions newer than 0.7.5 that change protocol 17 or the required plugin methods; and
- coding-agent versions or integrations that change the recorded lifecycle or alternate-screen behavior.

Newer compatible versions may work, but they remain expected rather than verified until the release matrix is
repeated. A future compatibility claim must include clean installation, rendering, agent lifecycle, graphics,
resize, focus, error preservation, viewer recreation, and restart evidence.

## Evidence

- [Ghostty and coding-agent runtime](evidence/2026-08-01-ghostty-runtime.md)
- [Named-session restart](evidence/2026-08-01-session-restart.md)
- [macOS arm64 fresh installation](evidence/2026-08-01-platform-macos-arm64.md)

