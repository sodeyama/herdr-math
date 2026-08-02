# Release Checklist

Target release: `0.2.0` — the first standalone release of Terminal Math (no Herdr runtime).

## Status

The repository is at the release-gate preparation stage. Everything in this checklist is a gate;
do not publish a tag until the pending items are recorded as evidence.

## Version agreement

- [x] `package.json` version = `0.2.0`
- [x] `Cargo.toml` workspace version = `0.2.0`
- [x] crate versions inherit the workspace (`version.workspace = true`)
- [x] `CHANGELOG.md` heading = `## [0.2.0] - Unreleased`
- [ ] release tag `v0.2.0` matches the above (created only when the gate passes)

## Clean build and validation (T-801)

- [x] `npm ci` from a clean clone
- [x] `npm run check` — typecheck, lint, format, runtime audit, security gates
- [x] `npm test` — 9 files, 75 tests
- [x] `npm run test:integration`
- [x] `npm run build` — reproducible: two clean builds produce identical hashes
- [x] `npm run smoke:render` — 10 tests
- [x] `cargo test` — 91 tests
- [x] `cargo clippy --all-targets` clean
- [x] `cargo fmt --check` clean

## No local paths

- [x] `npm run security:check` passes (13 runtime files; release files clean)
- [x] no `/Users/<name>` home paths in source, docs (except the literal rule quote), or lockfiles
- [x] no default socket paths or prototype paths remain
- [ ] clean-tag install in a fresh environment (blocked on the immutable tag)

## Runtime and compatibility

- [ ] **T-703**: real Ghostty matrix — placement, scrollback scroll, mouse wheel, keyboard
      fallback, replace, invalid preservation, clean exit (`AT-2-700`)
- [ ] kitty and WezTerm remain P1 until the same matrix passes (`AT-2-701`)
- [ ] `tmath diagnose` reports capabilities in a real terminal

## Release acceptance rule

`0.2.0` may be released only when:

1. Every P0 acceptance case applicable to the declared platforms and terminals is passed with
   current evidence.
2. P1/P2 gaps (kitty, WezTerm, Linux, Windows, `watch`, shared-memory/file media) are described
   as planned/unsupported, not implied as working.
3. The task checklist contains no incomplete release-gate task.
4. The final clean-tag install test passes after all release files are committed.
5. The Rust workspace, CLI, IPC, placement, and input-loop implementation all pass without a
   Herdr runtime present.

## Pending

- T-801 clean-tag install (needs the immutable `v0.2.0` tag)
- T-802 tag creation and release notes
- T-803 post-V2 backlog record (see the Phase 7 task list)
- T-703 real Ghostty runtime evidence
