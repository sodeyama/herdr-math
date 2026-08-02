# Terminal-math V2 Phase 6 Evidence (Compatibility and Documentation)

Date: August 2, 2026

## Scope

This evidence covers Phase 6 for the V2 standalone `tmath` refactor, on branch
`feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 6 removes every Herdr-plugin
artifact and `HERDR_*` read, rewrites the repository documentation to the standalone product
identity, marks the V1 spec superseded, and begins the real-terminal compatibility record.

## T-701: Herdr contract removal

Deleted (98 files, mechanical):

- `herdr-plugin.toml` and all `HERDR_*` environment reads.
- `src/herdr/`, `src/viewer/` (+ `src/viewer.ts`), `src/graphics/`, `src/manifest/`,
  `src/on-agent-status.ts`, `src/on-pane-closed.ts`, `src/startup.ts`, `src/diagnose.ts`,
  `src/index.ts`.
- Herdr-coupled `src/boundary/`, `src/state/`, `src/config/`, `src/events/`,
  `src/presentation/`, `src/diagnostics/`, and their tests, support rigs, reference oracles, and
  Herdr/presentation fixtures.

Kept: `src/renderer/*` (9 files), `src/scanner/scan-latex.ts`, `src/core/{contracts,errors,
limits}.ts` (Herdr-specific error codes, limit keys, and boundary types pruned), and the V2
tests (`answer-corpus.json` kept for the scanner spec).

Tooling rewrites:

- `package.json`: dropped `validate:manifest`, `test:contract`, the `herdr-plugin.toml` files
  entry, and the `herdr-plugin` keyword; description updated to the standalone identity.
- `scripts/security-check.mjs`: removed the `HERDR_*` env allowlist and the herdr/viewer socket
  special cases; `.rs`/`.swift` were already added to scanned extensions in Phase 5.

Result: `npm run typecheck`, `npm run lint`, `npm run check`, `npm test` (75 tests), and the
security gate (264 release files) all pass with zero dangling imports and zero `HERDR_*`
references in code or tooling.

## T-702: Standalone documentation

Rewritten to the standalone `tmath` identity:

- `AGENTS.md` — mission, product boundaries, required architecture (Rust CLI + one-shot TS
  render subprocess), privacy/security invariants, layout, workflow, testing, commit, and
  release gate; V1 spec marked superseded.
- `docs/concept.md`, `docs/architecture.md` — standalone two-process design, placement/input
  model, limits, error model, compatibility policy.
- `docs/getting-started.md`, `docs/compatibility.md`, `docs/README.md`, `docs/licensing.md`.
- `README.md` — status updated to "in development" through Phase 5.
- `SECURITY.md`, `PRIVACY.md`, `SUPPORT.md`, `CONTRIBUTING.md`, `CHANGELOG.md`.
- `docs/config.example.json` removed (it carried a real local username and referenced the
  deleted V1 `config` module). This also fixed the last security-gate `macos_home_path` finding.
- `docs/experiment-report.md` labeled as historical V1 evidence.
- Each `specs/herdr-math-v1/{tests,plans,tasks}/main.md` got a SUPERSEDED banner; the V1 tag
  remains for rollback.

## Validation

```sh
npm run check        # typecheck, lint, format, runtime audit, security gates — all pass
npm test             # 9 files, 75 tests passed
npm run build        # passes
cargo test           # 87 tests passed
cargo clippy --all-targets   # clean
cargo fmt --check    # OK
```

- `npm run security:check`: 13 runtime files and 264 release files pass (no remaining
  violations; the V1 `macos_home_path` findings are gone with `docs/config.example.json`).
- CLI smoke: `tmath render` over stdin and `tmath diagnose` both work with the renderer
  subprocess built.
- Static scan of tracked code/tooling finds no `HERDR_*`, `herdr-plugin`, or deleted
  `src/herdr`/`src/viewer`/`src/graphics`/`src/manifest` references.

## Acceptance status

- AT-2-705 (no Herdr contract remains): passed — `herdr-plugin.toml`, `src/herdr`,
  `src/viewer`, `src/graphics`, `src/manifest`, `src/on-*.ts`, `src/startup.ts`, and all
  `HERDR_*` reads are absent with no dangling imports; `npm run check` and `cargo test` pass.
- AT-2-702 (English public surface): public docs, CLI help, and error text are English.
- AT-2-703 (required public documentation): README, setup, troubleshooting, privacy, security,
  compatibility, contribution, license, changelog, and known-limit docs describe the standalone
  product; V1 is explicitly superseded.

## Commits

- `1e9b118` `docs(spec): expand phase 5 hardening tasks`
- `3e279e5` `chore(v2): remove the herdr plugin contract` (T-701)
- (this evidence; T-702 doc rewrite lands with the removal and this record)
