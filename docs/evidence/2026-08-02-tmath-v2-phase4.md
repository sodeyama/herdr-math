# Terminal-math V2 Phase 4 Evidence (CLI and Document Composition)

Date: August 2, 2026

## Scope

This evidence covers Phase 4 for the V2 standalone `tmath` refactor, on branch
`feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 4 delivers the user-facing CLI:
`tmath render <file | ->` with bounded reads and composition options, `tmath diagnose` for
capability checks, and accurate `--help`/`--version`. Document composition runs the scanner and
the allowlisted Markdown renderer through the Phase 1 transport.

## CLI surface

- `tmath render [OPTIONS] <file | ->` — reads a document (bounded at 1 MiB for both file and
  stdin), forwards it through the `tmath-render/1` transport, and either places the image in a
  real Kitty-graphics terminal (main buffer, scrollback-anchored) or prints a summary when stdout
  is not a terminal.
  - `--content-width <px>` and `--font-size <px>` are forwarded to the renderer layout.
- `tmath diagnose` — reports renderer subprocess availability, node, whether stdout is a
  terminal, and `a=q` Kitty graphics support when a real tty exists; exits non-zero when a
  required capability is missing.
- `tmath --help` / `--version` — print usage and the crate version.

## Validation

```sh
cargo test          # 83 tests passed (78 core/bin + 5 transport), all green
cargo clippy --all-targets   # clean
cargo fmt --check   # OK
npm test            # 386 passed (unchanged TypeScript)
```

CLI unit tests: argument parsing (input, `--content-width`, `--font-size`, `-`), rejection of
missing/two inputs, invalid widths, unknown options; help text mentions commands and options.

Renderer-integration tests (real TS subprocess):

- `markdown_document_composes_in_source_order_with_options_when_built`: renders
  `# Header`, prose with `$E=mc^2$`, a display `$$...$$` formula, and a list item `$a_i$` in
  source order; the default yields a bounded PNG and `--content-width 800` yields a wider canvas
  of exactly 800 px with the same renderer.
- `oversized_document_is_rejected_before_the_renderer`: a text payload over 1 MiB is rejected at
  the IPC boundary before the renderer is spawned.

CLI smoke (no tty, real renderer):

```sh
printf '# Header\n\nProse with $E=mc^2$.\n\n$$x = ...$$\n' | ./target/debug/tmath render -
# ok width=480 height=135 bytes=3529 renderer=katex-playwright-sharp
./target/debug/tmath render --content-width 800 doc.md
# ok width=800 height=135 ...
./target/debug/tmath render --font-size 18 --content-width 600 doc.md
# ok width=600 height=173 ...
```

Oversized file (1 MiB + 100 bytes) is rejected with `document exceeds 1048576 bytes` (exit 2).

`tmath diagnose` without a tty reports renderer/node availability and stdout status; with
`TMATH_RENDER_WORKER` set it exits 0.

## Acceptance status

- AT-2-501 (renderer limits preserved): formula/aggregate/PNG limits still enforced by the V1
  pipeline; the CLI adds the 1 MiB document-read bound at the boundary, covered by the
  oversized-rejection integration test.
- AT-2-502 (renderer corpus compatibility): the standalone pipeline renders the fixed corpus of
  prose + math in source order through `renderResponse(scanLatex(...))`; the Markdown
  composition integration test covers headings, prose, inline math, display math, and list items.
- AT-2-304 (no Kitty support): `tmath diagnose` reports missing graphics support and
  `place_in_terminal` fails closed before emitting any placement; help text documents terminals.
- AT-2-706 (English public surface): CLI help, version, diagnostics, and error messages are
  English.

Runtime (real Ghostty placement) and install evidence remain deferred to later phases and the
release gate.

## Commits

- `9937548` `docs(spec): expand phase 4 cli and composition tasks`
- `b3dc302` `feat(cli): add bounded render reads and composition options`
