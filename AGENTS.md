# Repository Instructions

## Scope

These instructions apply to the entire `terminal-math` repository. More specific `AGENTS.md`
files may add rules for a subdirectory, but they must not weaken the safety, privacy, testing,
or release requirements defined here.

## Mission

Terminal Math (`tmath`) is a standalone terminal renderer: it renders `$...$` and `$$...$$`
equations plus a strict allowlisted Markdown subset as transparent images, transmits them with
the Kitty graphics protocol, and anchors them to terminal cells so they scroll with the shell
scrollback. It needs no plugin runtime (no Herdr, no browser, no daemon).

The public identity is:

- Product name: `Terminal Math` (binary: `tmath`)
- Repository: `sodeyama/terminal-math` (renamed from `herdr-math`; product migrated to a
  standalone identity)
- One-line description: `Render Markdown and LaTeX as scrollable terminal images.`

The repository is intended for international users. Treat portability, predictable installation,
privacy, and clear English documentation as product requirements.

## Language

- Write all public documentation, specifications, UI text, log messages, code comments, commit
  subjects, release notes, and issue text in English.
- Use plain, concise English. Prefer direct technical wording over idioms or wordplay.
- Keep identifiers in English.
- Test fixtures may contain non-English text only when the test explicitly verifies Unicode or
  multilingual behavior.

## Sources of Truth

Use the following precedence when requirements disagree:

1. `AGENTS.md`
2. `specs/terminal-math-v2/tests/main.md`
3. `specs/terminal-math-v2/plans/main.md`
4. `specs/terminal-math-v2/tasks/main.md`
5. Current official Kitty graphics protocol documentation
6. `docs/architecture.md` and `docs/concept.md`
7. `docs/experiment-report.md`
8. Existing implementation and tests

The experiment report is evidence, not a permanent API contract. When the implementation
conflicts with current Kitty graphics semantics, follow the official protocol and update the
specifications before implementing.

The V1 Herdr plugin spec (`specs/herdr-math-v1/`) is superseded and kept only as historical
reference. Do not treat it as current product guidance.

## Product Boundaries

- Build a standalone terminal renderer, not a plugin, browser extension, shell integration, or
  standalone terminal emulator.
- Do not call Ghostty-specific CLI, AppleScript, configuration, or application APIs. Ghostty is
  one verified outer terminal, not a required application dependency.
- Image display uses the Kitty graphics protocol. Ghostty is the verified primary terminal;
  kitty and WezTerm are P1 until recorded evidence passes the same matrix.
- Parse `$...$` and `$$...$$` math delimiters first; `\(...\)` and `\[...\]` are retained from
  V1. Render a strict allowlisted Markdown subset (headings, emphasis, lists, quotes, tables,
  code blocks, inert links) through a local renderer that never executes raw HTML, links, or
  scripts. Do not expand into arbitrary HTML or a fully general Markdown engine, and do not allow
  user-provided CSS or color directives.
- Do not execute LaTeX, shell commands, user-provided JavaScript, remote resources, or TeX
  binaries.
- Do not upload document contents, equations, images, logs, or telemetry to a network service.

## Required Architecture

- Use `Cargo.toml` and `package.json` as the build and runtime contract.
- The Rust `tmath` binary owns the terminal: raw mode, Kitty negotiation, mouse/keyboard input,
  scroll state machine, and scrollback-anchored placement in the main screen buffer (never the
  alternate screen).
- Two render engines coexist during the V3 migration
  (`specs/terminal-math-v3/plans/main.md`):
  - **Native engine (default)**: the Rust `engine/crates/tmath-render` crate renders
    in-process (RaTeX for math, Typst as a library for the Markdown subset) with no
    subprocess and no IPC. It must use only fonts embedded in the binary (no system font
    scan), must disable Typst package resolution and all network and filesystem
    capabilities, must pass user text into Typst only through escaped string literals
    (never as markup), and must enforce the per-block limit, deadline, and fail-closed
    error contracts inside the crate.
  - **Node engine (deprecated, `tmath render --engine node`)**: the TypeScript
    `tmath-render` subprocess is one-shot: it reads one bounded JSON request on stdin,
    renders with KaTeX/Chromium/sharp, writes one bounded JSON response on stdout, and
    exits. It uses the versioned JSON IPC (`tmath-render/1`) between Rust and the
    renderer; enforce size, timeout, and trust limits at that boundary. Scheduled for
    removal in Phase 5 per the V3 plan once single-binary packaging lands.
- Keep the parser, renderer transport, placement tracker, input decoder, scroll driver, and CLI
  as separate modules with narrow interfaces.
- Never store durable state in the repository; runtime artifacts live in a platform state
  directory. Never add user-specific absolute paths.

## Privacy and Security Invariants

- Never persist raw document text, formula source, rendered bytes, or local paths in logs or
  durable state.
- Logs may contain event/status names, bounded counts, byte sizes, timing, non-reversible
  hashes, and stable error codes.
- Use a cryptographic hash when content identity is needed. Do not use a hash as a substitute
  for an authorization or boundary check.
- Render with remote resource loading disabled and a trust policy equivalent to KaTeX
  `trust: false`.
- Keep bounded, non-infinite limits for formula count, per-formula length, aggregate length, scan
  input bytes, render duration, image dimensions, raw PNG bytes, base64 payload size, and
  placement concurrency/pixel totals. Limits may be sized generously for real-world document and
  chapter-summary inputs, but every limit must remain finite and enforced so pathological input
  cannot exhaust memory or render time.
- Invalid input, timeouts, and payload rejection must leave earlier valid placements intact and
  fail closed.
- Never introduce `spawn` of user-controlled commands, shell evaluation, `eval`, dynamic imports
  from user input, or executable TeX engines without first updating the threat model and
  obtaining explicit approval.
- Do not commit secrets, tokens, private output, local usernames, home-directory paths, or
  unsanitized screenshots.

## Target Repository Layout

```text
Cargo.toml                     # Rust workspace
engine/crates/tmath-core/      # terminal surface: kitty, terminal, mouse, input, scroll, native
engine/crates/tmath/           # tmath CLI binary
src/
  renderer/                    # TS renderer pipeline (one-shot subprocess)
  scanner/
  core/                        # contracts, errors, limits
tests/
  unit/
  integration/
  fixtures/
scripts/
docs/
specs/herdr-math-v1/           # superseded V1, kept for reference
specs/terminal-math-v2/        # current specification triad
```

Keep generated output under `dist/` and `target/`, coverage under `coverage/`, and local runtime
artifacts outside the repository. Add generated and local files to `.gitignore` before producing
them.

## Development Workflow

1. Read the full specification triad before changing code.
2. Identify the task id and acceptance-test ids covered by the change.
3. Update a false or incomplete specification before implementing against it.
4. Implement one logical change at a time.
5. Run the narrowest relevant tests during development, then the full required validation before
   declaring the task complete.
6. Update the task checklist and documentation in a separate documentation commit immediately
   after the implementation commit when progress tracking changes.
7. Inspect `git status` and `git diff` before every commit. Preserve unrelated user changes.

Do not mark a task complete because code exists. Mark it complete only when its stated
acceptance tests pass with the required evidence.

## Testing Requirements

- Pure parser, boundary, state-machine, and limit logic must have deterministic unit tests.
- Kitty escape construction and probes must have contract tests against recorded or generated
  schema-compatible fixtures, run through a fake-tty harness with no real terminal required.
- Integration tests must cover duplicate and out-of-order input, placement replacement/removal,
  stale placement state, invalid LaTeX, timeout recovery, truncation, and fail-closed behavior.
- Rendering tests must compare dimensions, byte limits, and meaningful image snapshots or pixel
  hashes. Avoid fragile full-byte equality when encoder metadata can vary.
- Installation tests must use a clean checkout and the same build commands declared in
  `Cargo.toml`/`package.json`.
- Runtime release evidence must include a real Kitty-graphics terminal (Ghostty primary).
  CI-only mocks are not sufficient for the release gate.
- A failed, skipped, retried, or unimplemented acceptance case is not a pass.

Planned command names belong in `package.json`/Cargo manifests until they exist. The expected
top-level validation surface is:

```sh
npm ci
npm run check
npm test
npm run test:integration
npm run build
npm run smoke:render
cargo test
cargo clippy --all-targets
```

## Documentation Rules

- Keep `README.md` concise and user-oriented.
- Put product intent in `docs/concept.md`, target technical design in `docs/architecture.md`,
  and historical evidence in `docs/experiment-report.md`.
- Clearly label prototype behavior, target behavior, verified behavior, and planned behavior. Do
  not present planned functionality as released.
- Link to primary Kitty graphics protocol documentation for placement, probe, and input
  sequences.
- Update documentation in the same change that modifies a public command, configuration key,
  compatibility claim, limit, error code, or lifecycle contract.

## Commit and Pull Request Discipline

- One commit must contain one logical change. Aim for no more than 300 changed source lines and
  2-6 files, excluding lockfiles, generated output, and test fixtures.
- Separate feature or fix work from refactoring and from specification or documentation progress
  updates.
- Use English Conventional Commit subjects such as `feat(placement): anchor image blocks`.
- Do not use vague subjects such as `WIP`, `fix`, or `updates`.
- Keep one pull request focused on one issue or one cohesive release phase. Split unrelated bugs
  into separate branches or pull requests.
- Include acceptance-test ids, commands run, runtime evidence, compatibility scope, and remaining
  limitations in the pull request description.

## Release Gate

Do not publish a release until all v2 `0.2.0` release-gate tasks are complete.

At minimum, verify:

- Clean build and install of the standalone artifact with no Herdr runtime present
- No absolute local paths or undeclared runtime dependencies
- Fresh dependency installation and build for both Rust and TypeScript
- Unit, integration, rendering, security, and real Ghostty smoke tests
- Correct behavior when Kitty graphics is disabled or unavailable
- Accurate platform and terminal compatibility statements
- English README, setup, troubleshooting, security, license, and release notes
- No secrets, document content, private paths, or unredacted local screenshots

Version tags, `Cargo.toml` versions, and `package.json` versions must agree.
