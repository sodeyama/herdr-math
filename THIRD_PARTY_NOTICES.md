# Third-Party Notices

Terminal Math (`tmath`) is MIT licensed. The release binary embeds or links the
third-party components below. Each component remains under its own license.

This inventory summarizes the native renderer stack shipped in `0.3.0` after the
Node/Chromium renderer was removed (V3 Phase 5, T3-502). It does not replace the
full license texts in each upstream repository or crate.

## Embedded fonts (OFL)

| Font | Files | License |
| --- | --- | --- |
| M PLUS 2 | `engine/crates/tmath-render/assets/fonts/MPlus2-Regular.ttf`, `MPlus2-Bold.ttf` | [SIL Open Font License 1.1](engine/crates/tmath-render/assets/fonts/OFL.txt) |

M PLUS 2 is bundled with `include_bytes!` and selected through the `cjk_font`
configuration key (`m-plus-2`). The complete OFL text ships in
`engine/crates/tmath-render/assets/fonts/OFL.txt`.

## Typst embedded font assets

The `typst-as-lib` crate (with `typst-kit-embed-fonts`) embeds Typst's default font
collection into the binary for prose layout. Typst and its font bundle are
Apache-2.0 licensed. See the [Typst repository](https://github.com/typst/typst) and
the `typst-assets` crate for the authoritative font inventory and notices.

## Rust renderer dependencies (pinned in `Cargo.lock`)

| Component | Locked version (workspace) | License |
| --- | --- | --- |
| RaTeX parser/layout/SVG/types | 0.1.14 | MIT |
| typst | 0.13.x (via typst-as-lib 0.14.4) | Apache-2.0 |
| typst-as-lib | 0.14.4 | MIT |
| typst-render | 0.13.x | Apache-2.0 |
| pulldown-cmark | 0.13.x | MIT |
| png | 0.18.x | MIT OR Apache-2.0 |
| serde / serde_json | 1.x | MIT OR Apache-2.0 |
| rustix | 1.x | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| base64 | 0.22.x | MIT OR Apache-2.0 |

RaTeX embeds KaTeX-compatible math fonts through its `embed-fonts` feature. Those
font files and notices are carried in the `ratex-katex-fonts` and related RaTeX
crates under MIT terms.

## Terminal frontend dependencies

The `tmath-core` and `tmath` crates add only standard Rust ecosystem dependencies
(serde, png, rustix, toml, etc.) for Kitty escape construction, terminal I/O, and
CLI parsing. No Node.js, Chromium, Playwright, or Sharp artifacts are installed or
loaded at runtime.

## Repository tooling (optional, not required to run `tmath`)

| Component | Purpose | License |
| --- | --- | --- |
| Node.js (optional) | runs `scripts/security-check.mjs` in CI | Node.js license |

Running `tmath` after `scripts/install.sh` does not require Node.js.

## Historical Node renderer (removed)

Prior to `0.3.0`, Terminal Math shipped a deprecated TypeScript renderer using KaTeX,
Playwright/Chromium, Sharp, markdown-it, and highlight.js. That stack was removed
in commit `feat(renderer): remove Node/Chromium browser renderer stack`. Distributors
must not rely on the pre-0.3.0 notices for current binaries.
