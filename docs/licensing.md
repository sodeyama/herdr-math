# Licensing and Notices

## Project License

Terminal Math source code and project documentation are released under the
[MIT License](../LICENSE).

The MIT license was selected for a public community tool because it permits use, modification,
and redistribution with a short attribution requirement. It applies only to material owned by
this project.

## Prototype Boundary

The prototype described in [the experiment report](experiment-report.md) is evidence, not a
source-code dependency. The terminal-facing Rust port derives from the MIT-licensed
`terminal-browser` `pixel-core` crate; ownership and license compatibility are verified before
use.

Before porting other prototype code or assets, a contributor must verify ownership and license
compatibility. Material with unclear provenance must be reimplemented from the public
specification and synthetic fixtures.

## Third-Party Dependencies

Each dependency remains under its own license. Adding a production dependency requires all of
the following before release:

- record its package name, version, source, and license;
- preserve its license or notice text when required;
- verify that its license permits binary and source redistribution;
- include transitive production dependencies in the audit;
- avoid dependencies whose terms conflict with the MIT-licensed distribution.

The renderer uses KaTeX 0.18.1, Playwright 1.62.1 with a Chromium headless shell, and Sharp
0.35.3 with libvips. The Rust workspace uses rustix, serde, and the other crates recorded in
`Cargo.lock`/`package-lock.json`.

## Fonts and Other Assets

KaTeX packages its CSS and fonts together under its root MIT license; the audit confirms that the
CSS references exactly the installed font files. The browser and FFmpeg artifacts retain their
upstream license files next to their executables.

System fonts and assets downloaded at runtime are not an acceptable undocumented fallback. The
renderer must remain offline and reproducible from the declared package and platform
requirements.

## Release Notice Gate

`THIRD_PARTY_NOTICES.md` is required and included in the package file list. The release gate must
rerun `npm run audit:runtime` and `npm run audit:browser` from a clean install. Any dependency,
browser revision, font inventory, native artifact, or license change must update the notice and
audit expectations in the same change.
