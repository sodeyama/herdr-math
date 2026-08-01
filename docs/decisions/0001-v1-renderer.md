# ADR 0001: V1 Renderer Backend

- Status: Accepted and implemented
- Date: August 1, 2026
- Decision owners: Herdr Math maintainers
- Acceptance tests: AT-410, AT-708

## Context

Herdr Math needs to turn a bounded final-response document containing untrusted prose and LaTeX into one local PNG. The renderer must preserve source order and formula coverage, remain offline after installation, reject unsafe input, clean up deterministically, and fit the public plugin installation model.

T-402 compared two fixed-version candidates against the same release corpus:

1. KaTeX 0.18.1, Playwright 1.62.1, Chromium headless shell 151.0.7922.34, and Sharp 0.35.3.
2. MathJax 4.1.3, MathJax New Computer Modern 4.1.3, and resvg-js 2.6.2.

The complete measurements and reproduction commands are in [Renderer candidate measurements](../evidence/2026-08-01-renderer-candidates.md).

## Decision

V1 will use **KaTeX, Playwright Chromium headless shell, and Sharp** behind the backend-neutral renderer interface.

The renderer will:

- escape prose as text and parse each formula with KaTeX using `throwOnError: true`, `trust: false`, strict command handling, bounded expansion, and fresh macros per render;
- reject URL-capable and HTML-extension commands in a shared input gate before browser startup;
- load only local KaTeX CSS and font assets;
- abort browser HTTP and HTTPS requests;
- compose prose, inline math, and display math in source order with fixed padding;
- use one inherited base font size for prose and KaTeX;
- capture a transparent background so the attached terminal remains visible behind the response;
- capture a PNG, validate dimensions and byte limits, and optimize it with Sharp;
- close the page, browser context, and browser on every success or failure path;
- map failures to the stable renderer error codes without logging formula source or rendered markup.

Transparent output is the fixed v0.1 background behavior. Custom foreground colors remain deferred until Herdr provides a tested theme contract.

## Evidence

All values below are medians from three fresh Node.js processes on macOS 26.5.2 arm64 with Node.js 22.21.1. Install measurements used fresh temporary projects and a warm npm cache.

| Metric | Selected browser backend | Rejected SVG backend |
|---|---:|---:|
| Valid corpus | 8/8 | 8/8 |
| Invalid corpus rejected by raw parser | 3/3 | 2/3 |
| Process start to first PNG | 371.0 ms | 423.5 ms |
| First render after initialization | 85.4 ms | 327.7 ms |
| Warm case latency | 42.7 ms | 207.2 ms |
| Full startup and corpus | 870.6 ms | 2,997.3 ms |
| Node peak RSS | 157.7 MiB | 431.1 MiB |
| Total PNG bytes for eight cases | 25,528 | 25,036 |
| Installed package tree | 50.5 MiB | 115.3 MiB |
| Additional browser artifact | 198.5 MiB | None |
| Total installed footprint | 249.0 MiB | 115.3 MiB |
| Total clean install time | 11.41 s | 3.84 s |
| Native runtime artifact | Sharp addon and Chromium executable | resvg addon |
| Rendering network attempts observed | 0 | 0 |

The original candidate comparison produced one padded image for every formula case, including cases with multiple formulas. The implemented response-document contract additionally verifies escaped prose, source order, transparent pixels, and a shared prose-and-math base size. The SVG prototype produced correct individual formula shapes but did not implement case-level composition or padding, and it accepted one unsupported command.

## Security Boundary

The selected backend has a larger process and executable surface, so the following controls are part of the decision rather than optional hardening:

- No formula, event, or environment value is used as a process argument, executable path, file path, module name, or shell input.
- Production code does not use `child_process`, shell evaluation, `eval`, user-directed dynamic imports, or a TeX executable.
- The browser executable and local asset roots are resolved only from locked dependencies and approved plugin directories.
- KaTeX `trust: false` is necessary but not sufficient. A separate command policy rejects link, URL, image, and HTML-extension commands before rendering.
- The browser context denies remote requests. Local asset reads are restricted to the locked KaTeX distribution.
- Formula count, per-formula length, aggregate length, render time, dimensions, pixels, raw PNG bytes, and encoded payload size are checked at their earliest enforcement point.
- Timeout handling closes the complete browser process tree. A later valid render must succeed after a timeout.
- Errors expose only stable codes and bounded metadata. Formula source, generated HTML, page content, and arbitrary backend exceptions are not logged or persisted.
- The previous viewer image is retained until the new PNG passes every renderer and graphics check.

The T-404 contract tests prove these controls against the fixed corpus and a forced timeout. Failure of any control reopens this decision.

## Packaging and License Consequences

The selected backend adds approximately 249.0 MiB in the measured macOS arm64 installation and requires a Playwright-managed Chromium headless shell download during plugin installation. The manifest build must install the exact browser revision from the lockfile-compatible Playwright package; runtime rendering must not download anything.

The locked dependency and asset audit found:

- KaTeX is MIT licensed and distributes its fonts in the same package;
- Playwright and Playwright Core are Apache-2.0 licensed and include `NOTICE` files;
- Sharp is Apache-2.0 licensed and has transitive production dependencies;
- the Sharp libvips artifact is LGPL-3.0-or-later and records its bundled component licenses;
- the Chromium headless shell includes a BSD license and bundled third-party license text.
- Playwright also installs an FFmpeg artifact with its LGPL-2.1 license text.

The exact lock has no Git, file, URL, or external repository dependency specifier. `THIRD_PARTY_NOTICES.md` records the package and asset inventory, while complete upstream license files remain in installed packages and browser artifacts. The browser is installed under the plugin's `node_modules` and launched through a fixed revision path; user input and arbitrary environment paths do not select the executable.

T-405 passed on macOS arm64. The release gate must repeat the clean install and native smoke test on every declared architecture; this decision alone is not a release-readiness claim.

## Architecture Support

This decision does not widen the public compatibility claim.

| Platform and architecture | Decision status |
|---|---|
| macOS arm64 | Candidate executed and measured; release smoke test still required |
| macOS x64 | Unverified; must pass clean install and render smoke before declaration |
| Linux | Outside the current manifest; no v0.1 claim without a plan and runtime evidence |
| Windows | Outside the current manifest; no v0.1 claim without a plan and runtime evidence |

Every declared release architecture must install the locked Sharp artifact and Playwright browser, render the fixed corpus offline, and leave no owned process behind. A failing architecture must be removed from the manifest or cause the backend decision to be reconsidered.

## Rejected Alternative

The MathJax SVG plus resvg backend is rejected for v0.1, not permanently. Its measured disk footprint was 133.7 MiB smaller and it avoided a browser executable, but those benefits did not compensate for the current contract gaps, slower warm rendering, and higher measured Node RSS.

It may be reconsidered after v0.1 if it passes the same corpus, composition, invalid-input, cleanup, packaging, and security contracts without weakening behavior.

## References

- [KaTeX options](https://katex.org/docs/options)
- [KaTeX security guidance](https://katex.org/docs/security)
- [Playwright library lifecycle](https://playwright.dev/docs/library)
- [Sharp installation requirements](https://sharp.pixelplumbing.com/install/)
