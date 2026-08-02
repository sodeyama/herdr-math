# Terminal-math V2 Phase 1 Evidence

Date: August 2, 2026

## Scope

This evidence covers Phase 1 (render transport) for the V2 standalone `tmath` refactor, on
branch `feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 1 defines the versioned
JSON IPC between the Rust CLI and the one-shot TypeScript renderer subprocess, reusing the
existing `src/renderer/*` pipeline. No terminal placement, input loop, or release claim is made.

## Transport contract

- Protocol: `tmath-render/1` (shared constants in `src/renderer/ipc-contract.ts` and
  `engine/crates/tmath-core/src/ipc.rs`).
- Request: protocol, `kind` (`document` or `formulas`), optional `text` / `formulas`, optional
  render options. Bounded at 1 MiB.
- Success response: `ok:true` plus width, height, byte size, renderer name, base64 PNG.
- Failure response: `ok:false` plus a safe error record (stable code + retryable flag).
- The subprocess reads exactly one request on stdin, writes exactly one response on stdout, and
  exits; it never stays alive waiting for a second request.
- Request/response byte bounds and a 15 s render timeout are enforced at the Rust boundary; the
  renderer keeps the existing 8 s render timeout and `trust:false` KaTeX policy.

## Deliverables

- `src/renderer/ipc-contract.ts` — wire types, encode/decode, validation, bounds.
- `src/renderer/subprocess.ts` — one-shot renderer entrypoint.
- `engine/crates/tmath-core/src/ipc.rs` — Rust contract structs and parse/encode.
- `engine/crates/tmath` — `tmath render <file | ->` placeholder binary + `render_transport.rs`
  integration tests.
- `tests/fixtures/render-ipc/requests.json`, `tests/unit/ipc-contract.spec.ts`,
  `tests/integration/render-ipc.spec.ts`.
- `scripts/security-check.mjs` — ignore Rust `target/` output and the committed `Cargo.lock`.

## Validation

TypeScript:

```sh
npm run check       # typecheck, lint, format, manifest, runtime audit, security gates
npm test            # 49 files, 386 tests passed
```

- New files introduce no security-gate violations. The five remaining `macos_home_path`
  violations are pre-existing on `origin/main` (added in `620cfbb`) and are not touched here;
  they are rewritten in Phase 6 documentation work.

Rust:

```sh
cargo test          # 45 tests passed (42 core + 3 transport)
cargo clippy --all-targets   # clean
cargo fmt --check   # OK
```

End-to-end placeholder with the built renderer set via `TMATH_RENDER_WORKER`:

```sh
echo 'The relation is $E=mc^2$.' | ./target/debug/tmath render -
# ok width=480 height=24 bytes=1735 renderer=katex-playwright-sharp
```

- Invalid LaTeX returns `invalid_latex` (non-retryable) and does not leak source.
- Empty source returns `formula_not_found`.
- Missing `TMATH_RENDER_WORKER` fails with clear usage exit 2.

## Acceptance status

- AT-2-200 (versioned JSON IPC): covered by shared contract fixtures, TS unit tests, Rust
  round-trip/parse tests, and the empty/bad-protocol integration cases.
- AT-2-201 (one-shot lifecycle): the subprocess emits exactly one response and exits; verified by
  integration tests and the Rust transport tests.
- AT-2-202 (size, timeout, trust limits): request/response byte bounds, render timeout, and
  invalid LaTeX/trust rejection verified at both boundaries.
- AT-2-203 (render trust policy): raw HTML is escaped, `\href`/`\includegraphics` denied via the
  existing renderer; no remote resource or script executed.

Runtime (real Ghostty placement) and install evidence remain deferred to Phase 2+ and the release
gate.

## Commits

- `d554d17` `feat(ipc): define versioned render contract` (T-201)
- `131f761` `docs(spec): expand phase 1 render transport tasks`
- `c44874b` `feat(renderer): add one-shot render subprocess` (T-202)
- `53e8c3c` `fix(ipc): make formula validation type-safe`
- `716ef7a` `fix(renderer): exit zero after writing a handled error response`
- `f8f65e3` `test(ipc): cover the render transport contract` (T-203)
- `24ec138` `fix(security): ignore rust artifacts and committed lockfile`
- `3f667f7` `feat(cli): add tmath render transport placeholder` (T-204)
- `d73e9fa` `style(test): format render ipc specs`
- `65892cd` `fix(ipc): keep wire serialization out of the privacy gate`
