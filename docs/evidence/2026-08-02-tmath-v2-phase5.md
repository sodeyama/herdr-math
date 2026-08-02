# Terminal-math V2 Phase 5 Evidence (Hardening and Security)

Date: August 2, 2026

## Scope

This evidence covers Phase 5 for the V2 standalone `tmath` refactor, on branch
`feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 5 closes the security and privacy
gaps in the new Rust/TS split: user input fails closed with stable errors, the repo security gate
covers the Rust workspace, privacy invariants are asserted statically and at the CLI boundary,
and the input/mouse/escape/scanner parsers are fuzzed against adversarial input.

## T-601: Fail closed on scanner and renderer limit errors

A real crash was found and fixed: a `document` request with 21 formulas (over the scanner limit)
threw an uncaught `ScannerLimitError` in the renderer subprocess, producing no JSON and a
non-zero exit. `src/renderer/subprocess.ts` now wraps the scan-and-render path so any
`HerdrMathError` (e.g. `scanner_input_limit`) serializes to a stable bounded JSON error record and
the subprocess exits cleanly.

Verified end to end:

```json
{"protocol":"tmath-render/1","ok":false,"error":{"code":"scanner_input_limit","retryable":false,
 "details":{"limit_kind":"formula_count","limit":20,"actual":21}}}
```

Regression coverage: `tests/integration/render-ipc.spec.ts` asserts the stable error record and
no source emission for an over-limit document.

## T-602: Privacy audit for the new code paths

- `scripts/security-check.mjs` now scans `.rs` and `.swift` sources with the repository rules
  (absolute home paths, credentials, artifacts). No Rust violations were introduced; only the
  pre-existing V1 `macos_home_path` findings remain.
- A CLI sentinel test (`engine/crates/tmath/tests/render_transport.rs`) feeds a unique sentinel in
  a document that fails the scanner limit and asserts the sentinel never appears on stdout or
  stderr, and that the process terminates with a stable non-usage exit code.
- Static Rust gates (`engine/crates/tmath/tests/privacy_gates.rs`) scan every `.rs` file in the
  workspace (excluding `build.rs` and the test itself) and assert:
  - no `std::net` / `TcpStream` / `UdpSocket` / `reqwest` network imports,
  - no `eval(` or `sh -c` shell evaluation of user input,
  - no `/Users/` or `/home/` absolute home paths in committed source.

The only `Command::new` uses are the documented renderer subprocess (`node`), the Swift helper
build (`swiftc`), and the native helper spawn — none driven by user input.

## T-603: Fuzz the input, mouse, escape, and scanner parsers

- `input.rs` (Phase 3) already had a 512-iteration adversarial decode fuzz.
- `mouse.rs`: deterministic 4096-iteration fuzz over SGR parameter bodies; asserts the parser
  never panics and never reports zero coordinates.
- `terminal.rs`: deterministic 4096-iteration fuzz over cell-size, DECRQM, and graphics-probe
  reply bytes; asserts no panic and positive cell sizes.
- `scanner.spec.ts`: two deterministic LCG-based fuzz tests over a token alphabet that stresses
  delimiters, fences, escapes, code, and Unicode; asserts valid in-bounds offsets or a
  `ScannerLimitError`, and never a different exception.

## Validation

```sh
cargo test          # all suites pass (76 core + gates + transport)
cargo clippy --all-targets   # clean
cargo fmt --check   # OK
npm test            # 389 passed (49 files), all green on re-run
npm run check       # typecheck, lint, format, manifest, runtime audit pass
npm run security:check      # no new violations; the 5 remaining are pre-existing V1 docs
```

## Acceptance status

- AT-2-501 (renderer limits preserved): scanner `formula_count` limits now produce a stable
  `scanner_input_limit` error record instead of crashing; the 1 MiB document/`--content-width`
  boundaries remain enforced.
- AT-2-600 (no content in logs or state): sentinel CLI test + the existing renderer privacy tests
  confirm no document content, formula source, or paths in observable output.
- AT-2-601 (no network): static Rust and TS gates find no socket/network imports.
- AT-2-602 (no execution of user input): static gates find no shell/`eval`/TeX; only the
  documented renderer and helper spawns exist.
- AT-2-603 (invalid input preserves earlier placements): placement fail-closed behavior from
  Phase 2 plus the subprocess fail-closed fix.
- AT-2-403 / AT-2-500 (bounded parsing, scanner delimiters): fuzz coverage across all parsers.

Runtime (real Ghostty placement) and install evidence remain deferred to later phases and the
release gate.

## Commits

- `1e9b118` `docs(spec): expand phase 5 hardening tasks`
- `a70a5d1` `fix(renderer): fail closed on scanner and renderer limits` (T-601)
- `a6f104c` `test(security): enforce local-only privacy invariants` (T-602)
- `b7d6741` `test(security): fuzz input, mouse, and scanner parsers` (T-603)
