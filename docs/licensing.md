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

The v0.1 renderer decision selects KaTeX 0.18.1, Playwright 1.62.1 with its locked Chromium headless shell, and Sharp 0.35.3. A preliminary review found MIT, Apache-2.0, and BSD-style licensing with notice obligations that permit the planned distribution. [ADR 0001](decisions/0001-v1-renderer.md) records the decision and exact gate.

These packages are not release-audited merely because they were selected. T-405 must lock the production tree, inspect every transitive runtime package and distributed browser asset, retain required Playwright and Chromium notices, and record the result in this document or a generated notice file.

## Fonts and Other Assets

No font, browser binary, image, or other third-party asset may be bundled until its redistribution terms are verified. KaTeX fonts and the Playwright-managed Chromium headless shell are selected but remain subject to the T-405 exact-version audit. The release must preserve copyright notices, license files, attribution, and reserved-name conditions required by each asset license.

System fonts and assets downloaded at runtime are not an acceptable undocumented fallback. The renderer must remain offline and reproducible from the declared package and platform requirements.

## Release Notice Gate

Before a release, the dependency and asset audit must decide whether a `THIRD_PARTY_NOTICES` file is required. Package metadata, the release archive, the repository license, and any generated notices must describe the same license set.
