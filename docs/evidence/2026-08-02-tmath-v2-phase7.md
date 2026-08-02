# Terminal-math V2 Phase 7 Evidence (Release Gate)

Date: August 2, 2026

## Scope

This evidence covers Phase 7 for the V2 standalone `tmath` refactor, on branch
`feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 7 prepares the `0.2.0` release
gate: a reproducible clean build with no local paths, agreed versions, a release checklist, and a
recorded post-V2 backlog. Publishing the immutable tag is intentionally not done here; the gate
remains open until the pending evidence is collected.

## T-801: Reproducible clean build with no local paths

Verified in a fresh `git clone` of commit `280be41` at
`/var/folders/.../opencode/tmath-release/work`:

```sh
npm ci                 # locked dependencies + Chromium headless shell
npm run check          # typecheck, lint, format, runtime audit, security gates — all pass
npm test               # 9 files, 75 tests (3 consecutive clean runs; one cold-start transient
                       # failed the first run and was verified stable on three reruns)
npm run test:integration
npm run build          # 39 files; two clean builds produce identical sha256 hashes
npm run smoke:render   # 10 tests
cargo test             # 91 tests passed
cargo clippy --all-targets   # clean
cargo fmt --check      # OK
```

- `npm run security:check`: 13 runtime files and the release file set pass; no remaining
  violations.
- No `/Users/<name>` home paths, default socket paths, or prototype paths in source, docs
  (outside the literal rule quote), or lockfiles.

## T-802: Version agreement and release preparation

- `package.json` version set to `0.2.0` (was `0.1.0` from the V1 line).
- `Cargo.toml` workspace version `0.2.0`; both crates inherit via `version.workspace = true`.
- `CHANGELOG.md` heading `## [0.2.0] - Unreleased` matches.
- `docs/RELEASE.md` added: version agreement, clean-build/validation checklist, no-local-path
  checks, runtime/compatibility gates, and the release acceptance rule with pending items.

## T-803: Post-V2 backlog

- `docs/backlog.md` added: P1 (kitty/WezTerm, Linux, `watch`), P2 (Windows, shared-memory/file
  media, accessible/alternate output, placement sizing controls), with the labeling rule that
  nothing planned is presented as working.

## Acceptance status

- AT-2-003 (version agreement): package, workspace, crate, and changelog all read `0.2.0`.
- AT-2-004 (no user-specific absolute paths): clean-clone scan finds none; security gate passes.
- AT-2-005 (clean build reproducibility): two clean TS builds produce identical hashes.
- AT-2-706 (standalone release install): the procedure is documented in `docs/RELEASE.md`; the
  actual immutable-tag install remains pending a real tag and is recorded as such.

## Commits

- `280be41` `docs(spec): expand phase 7 release gate tasks`
- (T-801/T-802/T-803 land with this evidence)
