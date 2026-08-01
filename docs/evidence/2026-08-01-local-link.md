# Clean Local-Link Verification Evidence

Date: 2026-08-01

## Environment

- Herdr Math commit: `f15fc8d`
- Herdr client and server: 0.7.5, protocol 17, compatible
- Platform: macOS 26.5.2 on arm64
- Node.js: 22.21.1
- npm: 10.9.4

This verification followed the current Herdr [plugin development workflow](https://herdr.dev/docs/plugins/) and
[CLI reference](https://herdr.dev/docs/cli-reference/). Herdr documents local linking as registration of an
already-built checkout; it does not run the manifest build or one-shot startup hooks during `plugin link`.

## Clean checkout and registration

A temporary clean checkout ran every command declared by `herdr-plugin.toml` before linking:

```sh
npm ci
npm run install:browser
npm run audit:browser
npm run build
herdr plugin link <clean-checkout>
```

The worktree stayed clean. Herdr registered version 0.1.0 at the declared minimum version without an unknown
event, missing platform, incompatible version, or malformed command warning. The registered contract contained
both build commands plus the browser install and audit, the one-shot startup hook, both event hooks, the
`diagnose` action, and the `viewer` pane entrypoint.

Before linking, one unrelated local plugin was enabled. During verification, both plugins were enabled.

## Entrypoint and runtime checks

The `diagnose` action exited successfully and verified the Herdr version and protocol, plugin config and state
directories, local renderer, graphics capability, and cell dimensions. It reported `viewer_not_open` and
`terminal_unverified` as informational results before a viewer existed.

The linked checkout then completed these real socket operations:

| Operation | Result |
|---|---|
| Supported Codex `working` event | `baseline_stored`, exit 0 |
| Stable completion with one display formula | `image_published`, exit 0 |
| Viewer split creation and metadata registration | One unfocused owned viewer created |
| Viewer-generated status event with no agent | `ignored`, exit 0 |
| Viewer close | Viewer mapping removed, exit 0 |
| Source close | Fingerprint-only state removed, exit 0 |

The source pane retained focus while the viewer opened. Plugin logs contained only timestamp, level, outcome or
stable error code, and bounded cleanup counts. No pane output or LaTeX source appeared in the logs.

Real Herdr 0.7.5 checks exposed three contract details that differed from the original recorded fixture. The
implementation and fixtures were corrected before this evidence was accepted:

- split pane requests use `target_pane_id` and omit `workspace_id`;
- `pane.report_metadata` acknowledges with `ok`, followed by authoritative `pane.get`;
- socket pane reads use `recent_unwrapped`, and an absent pane may return `pane_not_found`.

Viewer panes also emit status events without an agent. Those events now produce a successful ignored outcome
instead of a failed plugin log.

## Unlink safety

The temporary viewer was closed before unlinking. `herdr plugin unlink io.github.sodeyama.herdr-math` returned
`removed: true`. The unrelated plugin remained enabled, and both unrelated panes remained present with the same
focus state.

Local unlink retained the user-owned linked checkout and the plugin config/state directories. This is the
observed retention behavior for a development link; removal of a Herdr-managed tagged checkout remains part of
the clean installation and uninstall release gate.

## Acceptance result

- AT-002: passed against Herdr 0.7.5 with no registration warning.
- AT-006: passed from an explicitly built clean checkout; every declared runtime entrypoint resolved from it.
- AT-010: the local-link safety portion passed. Unrelated plugins and panes were unchanged, and retained local
  checkout, config, and state behavior is recorded above. Managed tagged-checkout deletion remains unclaimed.

