# Repository Instructions

## Scope

These instructions apply to the entire `herdr-math` repository. More specific `AGENTS.md` files may add rules for a subdirectory, but they must not weaken the safety, privacy, testing, or release requirements defined here.

## Mission

Herdr Math is a public Herdr plugin that detects LaTeX equations in completed AI-agent responses and renders them as images in a reusable side pane.

The public identity is:

- Product name: `Herdr Math`
- Repository: `sodeyama/herdr-math`
- Planned plugin id: `io.github.sodeyama.herdr-math`
- One-line description: `Render LaTeX from AI agent responses in a side pane.`

The repository is intended for international users. Treat portability, predictable installation, privacy, and clear English documentation as product requirements.

## Language

- Write all public documentation, specifications, UI text, log messages, code comments, commit subjects, release notes, and issue text in English.
- Use plain, concise English. Prefer direct technical wording over idioms or wordplay.
- Keep identifiers in English.
- Test fixtures may contain non-English text only when the test explicitly verifies Unicode or multilingual behavior.

## Sources of Truth

Use the following precedence when requirements disagree:

1. `AGENTS.md`
2. `specs/herdr-math-v1/tests/main.md`
3. `specs/herdr-math-v1/plans/main.md`
4. `specs/herdr-math-v1/tasks/main.md`
5. Current official Herdr documentation and the schema emitted by the minimum supported Herdr version
6. `docs/architecture.md` and `docs/concept.md`
7. `docs/experiment-report.md`
8. Existing implementation and tests

The experiment report is evidence, not a permanent API contract. When the prototype conflicts with current Herdr plugin semantics, follow the official API and update the specifications before implementing.

## Product Boundaries

- Build a Herdr plugin, not a Ghostty plugin, shell integration, browser extension, or standalone terminal emulator.
- Do not call Ghostty-specific CLI, AppleScript, configuration, or application APIs.
- Image display may depend on Herdr's experimental Kitty graphics support. Ghostty is one verified outer terminal, not a required application dependency.
- The first public release targets the platforms explicitly declared in `herdr-plugin.toml`. Do not claim support for an untested platform or terminal.
- Support detected Claude Code and Codex panes first. Add another agent only with recorded lifecycle evidence and tests.
- Parse `$...$` and `$$...$$` math delimiters for v1. Do not expand into a general Markdown renderer.
- Do not execute LaTeX, shell commands, user-provided JavaScript, remote resources, or TeX binaries.
- Do not upload pane contents, equations, images, logs, or telemetry to a network service.

## Required Architecture

- Use `herdr-plugin.toml` as the installation and runtime contract.
- Use Herdr-provided `HERDR_*` environment variables. Never add user-specific absolute paths or assume a default socket location.
- Treat `[[startup]]` as a one-shot hook. Do not launch an unsupervised long-running controller from a startup hook.
- Use manifest event hooks and short-lived workers for agent lifecycle events unless an updated, documented Herdr API provides a supervised service primitive.
- Use `HERDR_PLUGIN_STATE_DIR` for runtime state and `HERDR_PLUGIN_CONFIG_DIR` for user-editable configuration. Never store durable state in `HERDR_PLUGIN_ROOT`.
- Namespace state by Herdr session or socket identity and source pane. Use atomic writes and per-pane locks so concurrent `done` and `idle` events cannot create duplicate work.
- Fail closed when the current answer boundary cannot be established. Never render an uncertain slice of historical pane content.
- Reuse one viewer pane per source pane. Preserve source focus, replace the existing graphics layer, and recreate the viewer only after it has been closed.
- Keep the parser, boundary detector, renderer, Herdr client, event handler, state store, and viewer lifecycle as separate modules with narrow interfaces.

## Privacy and Security Invariants

- Never persist raw pane output, answer text, selected text, or LaTeX source in logs or durable state.
- Logs may contain event names, pane ids, status values, bounded counts, byte sizes, timing, non-reversible hashes, and stable error codes.
- Use a cryptographic hash when content identity is needed. Do not use a hash as a substitute for an authorization or boundary check.
- Render with remote resource loading disabled and an explicit trust policy equivalent to KaTeX `trust: false`.
- Keep strict limits for formula count, per-formula length, aggregate length, render duration, image dimensions, raw PNG bytes, and base64 payload size.
- Invalid input, timeouts, and payload rejection must leave the previous valid image intact.
- Never introduce `child_process`, shell evaluation, `eval`, dynamic imports from user input, or executable TeX engines without first updating the threat model and obtaining explicit approval.
- Do not commit secrets, tokens, private pane output, local usernames, home-directory paths, or unsanitized screenshots.

## Target Repository Layout

```text
herdr-plugin.toml
src/
  boundary/
  events/
  herdr/
  renderer/
  scanner/
  state/
  viewer/
tests/
  unit/
  integration/
  fixtures/
scripts/
docs/
specs/herdr-math-v1/
  tests/main.md
  plans/main.md
  tasks/main.md
```

Keep generated output under `dist/`, coverage output under `coverage/`, and local runtime artifacts outside the repository. Add generated and local files to `.gitignore` before producing them.

## Development Workflow

1. Read the full specification triad before changing code.
2. Identify the task id and acceptance-test ids covered by the change.
3. Update a false or incomplete specification before implementing against it.
4. Implement one logical change at a time.
5. Run the narrowest relevant tests during development, then the full required validation before declaring the task complete.
6. Update the task checklist and documentation in a separate documentation commit immediately after the implementation commit when progress tracking changes.
7. Inspect `git status` and `git diff` before every commit. Preserve unrelated user changes.

Do not mark a task complete because code exists. Mark it complete only when its stated acceptance tests pass with the required evidence.

## Testing Requirements

- Pure parser, boundary, state-machine, and limit logic must have deterministic unit tests.
- Herdr protocol code must have contract tests against recorded or generated schema-compatible fixtures.
- Integration tests must cover duplicate delivery, out-of-order lifecycle events, viewer reuse, viewer closure, stale locks, invalid LaTeX, timeout recovery, truncation, and focus preservation.
- Rendering tests must compare dimensions, byte limits, and meaningful image snapshots or pixel hashes. Avoid fragile full-byte equality when encoder metadata can vary.
- Installation tests must use a clean checkout and the same build commands declared in `herdr-plugin.toml`.
- Runtime release evidence must include a real Herdr session and a graphics-capable outer terminal. CI-only mocks are not sufficient for the release gate.
- A failed, skipped, retried, or unimplemented acceptance case is not a pass.

Planned command names belong in `package.json`. Until they exist, do not document them as working commands. Once implemented, the expected top-level validation surface is:

```sh
npm ci
npm run check
npm test
npm run test:integration
npm run build
npm run smoke:render
```

## Documentation Rules

- Keep `README.md` concise and user-oriented.
- Put product intent in `docs/concept.md`, target technical design in `docs/architecture.md`, and historical evidence in `docs/experiment-report.md`.
- Clearly label prototype behavior, target behavior, verified behavior, and planned behavior. Do not present planned functionality as released.
- Link to primary Herdr documentation for plugin lifecycle and socket behavior.
- Update documentation in the same change that modifies a public command, configuration key, compatibility claim, limit, error code, or lifecycle contract.

## Commit and Pull Request Discipline

- One commit must contain one logical change. Aim for no more than 300 changed source lines and 2-6 files, excluding lockfiles, generated output, and test fixtures.
- Separate feature or fix work from refactoring and from specification or documentation progress updates.
- Use English Conventional Commit subjects such as `feat(events): handle agent completion hooks`.
- Do not use vague subjects such as `WIP`, `fix`, or `updates`.
- Keep one pull request focused on one issue or one cohesive release phase. Split unrelated bugs into separate branches or pull requests.
- Include acceptance-test ids, commands run, runtime evidence, compatibility scope, and remaining limitations in the pull request description.

## Release Gate

Do not publish a release or add the `herdr-plugin` GitHub topic until all v1 release-gate tasks are complete.

At minimum, verify:

- Clean GitHub installation through `herdr plugin install sodeyama/herdr-math --ref <tag>`
- Manifest validation against the minimum supported Herdr version
- No absolute local paths or undeclared runtime dependencies
- Fresh dependency installation and build
- Unit, integration, rendering, security, and real Herdr smoke tests
- Correct behavior when Kitty graphics is disabled or unavailable
- Accurate platform and terminal compatibility statements
- English README, setup, troubleshooting, security, license, and release notes
- No secrets, pane content, private paths, or unredacted local screenshots

Version tags and `herdr-plugin.toml` versions must agree.
