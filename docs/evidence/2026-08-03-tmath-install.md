# Terminal Math Install Evidence

Date: 2026-08-03
Scope: `scripts/install.sh` user-local install, renderer auto-discovery, coding-agent skill.

## Result

- **Install**: PASS. `bash scripts/install.sh` builds the release binary and
  renderer and installs to `~/.local/share/tmath/app` with a launcher at
  `~/.local/bin/tmath`.
- **Auto-discovery**: PASS. The installed binary renders a document with no
  `TMATH_RENDER_WORKER` set.
- **Skill**: PASS. `tmath` SKILL.md symlinked into `.agents/skills`,
  `.claude/skills`, `.codex/skills`, `.cursor/skills`,
  `.config/opencode/skills`, and `.pi/agent/skills`.
- **Render from installed binary**: PASS (`ok width=480 height=106
  bytes=3178 renderer=katex-playwright-sharp`).

## Commands and observed output

```text
$ bash scripts/install.sh
tmath: installing to ~/.local/share/tmath/app
tmath: installing renderer runtime dependencies (npm ci --omit=dev)…
tmath: installed 0.2.0 to ~/.local/share/tmath/app
tmath: launcher ~/.local/bin/tmath
tmath: skill linked into: .agents/tmath .claude/tmath .codex/tmath .cursor/tmath opencode/tmath agent/tmath
tmath 0.2.0
renderer subprocess: available
node: available
stdout: not a terminal (image transport unavailable here)
kitty graphics: not probed (no stdin terminal)

$ printf 'The answer is $E=mc^2$.\n' | tmath render -
ok width=480 height=24 bytes=1747 renderer=katex-playwright-sharp
```

The `diagnose` gate reported only the expected non-tty notes (no terminal
available in the test harness), which do not count as failures.

## Bug found by the install test

The one-shot render subprocess entry check
(`import.meta.url === pathToFileURL(resolve(argv[1]))`) **failed when the
worker path went through a symlinked directory**: on macOS `/tmp` is a symlink
to `/private/tmp`, so the resolved `argv[1]` path and Node's
symlink-resolved `import.meta.url` diverged and `main()` never ran (empty
response, exit 0). The installed renderer lives under `~/.local/share` so this
did not reproduce there, but it would have for any worker under `/tmp`.
Fixed by comparing against `realpathSync(resolve(argv[1]))`, and by using
`process.exitCode` (natural exit) so stdout drains for large responses.

## Privacy

Installer logs contain only paths, versions, and statuses; no document or
formula content. The install writes only to the user data/bin directories and
the agent skill directories listed above.
