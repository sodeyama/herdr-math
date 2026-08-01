# Licensing and Notices

## Project License

Herdr Math source code and project documentation are released under the [MIT License](../LICENSE).

The MIT license was selected for a public community plugin because it permits use, modification, and redistribution with a short attribution requirement. It applies only to material owned by this project.

## Herdr Relationship

Herdr Math is an independent community plugin for Herdr. It is not part of Herdr and does not imply endorsement by the Herdr maintainers.

The project may use the Herdr name to describe compatibility. It must not copy Herdr source code, artwork, branding assets, or documentation into a release unless their license and required notices have been verified separately.

## Prototype Boundary

The prototype described in [the experiment report](experiment-report.md) is evidence, not a source-code dependency. No prototype file is imported into the public package by default.

Before porting prototype code or assets, a contributor must verify ownership and license compatibility. Material with unclear provenance must be reimplemented from the public specification and synthetic fixtures.

## Third-Party Dependencies

Each dependency remains under its own license. Adding a production dependency requires all of the following before release:

- record its package name, version, source, and license;
- preserve its license or notice text when required;
- verify that its license permits binary and source redistribution;
- include transitive production dependencies in the audit;
- avoid dependencies whose terms conflict with the MIT-licensed distribution.

The v0.1 renderer uses KaTeX 0.18.1, Playwright 1.62.1 with Chromium headless shell 151.0.7922.34, and Sharp 0.35.3 with libvips 8.18.3. [ADR 0001](decisions/0001-v1-renderer.md) records the backend decision.

The T-405 exact-version audit is recorded in [Third-Party Notices](../THIRD_PARTY_NOTICES.md). It verifies:

- exact direct versions and lockfile integrity;
- npm-registry-only package artifacts with no Git, file, URL, or external repository dependency specifier;
- license metadata for every non-development lock entry;
- retained package license and Playwright notice files;
- both macOS arm64 and x64 Sharp and libvips lock entries;
- the installed macOS arm64 Sharp/libvips runtime versions;
- the plugin-local Chromium executable and complete `LICENSE.headless_shell` inventory;
- the companion FFmpeg executable and LGPL-2.1 license installed by Playwright;
- all 60 KaTeX font files referenced by the locked CSS.

The production license set includes MIT, Apache-2.0, ISC, 0BSD, LGPL, MPL-2.0, BSD-style, and other permissive component licenses recorded by the installed libvips and Chromium inventories. The installation retains those upstream files rather than replacing them with a summary.

## Fonts and Other Assets

KaTeX packages its CSS and fonts together under its root MIT license; the audit confirms that the CSS references exactly the 60 installed font files. The browser and FFmpeg artifacts retain their upstream license files next to their executables. The Sharp native package retains its Apache license, while the libvips package retains a component-level licensing inventory and source-project link.

System fonts and assets downloaded at runtime are not an acceptable undocumented fallback. The renderer must remain offline and reproducible from the declared package and platform requirements.

## Release Notice Gate

`THIRD_PARTY_NOTICES.md` is required and included in the package file list. The release gate must rerun `npm run audit:runtime` and `npm run audit:browser` from a clean install. Any dependency, browser revision, font inventory, native artifact, or license change must update the notice and audit expectations in the same change.
