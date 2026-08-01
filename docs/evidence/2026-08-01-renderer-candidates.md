# Renderer Candidate Measurements

## Status

This document records the T-402 renderer experiment. It is comparison evidence, not the final renderer decision.

## Environment

- Date: 2026-08-01
- Platform: macOS 26.5.2, arm64
- Runtime: Node.js 22.21.1, npm 10.9.4
- Corpus: `tests/fixtures/renderer/formula-corpus.json`
- Repetitions: three fresh Node.js processes per candidate
- Install measurements: fresh temporary project directories with a warm npm package cache
- Network policy during rendering: page HTTP(S) routes denied for the browser candidate; Node network entrypoints denied and external SVG references rejected for the SVG candidate

The compared fixed versions were:

- Browser: KaTeX 0.18.1, Playwright 1.62.1, Sharp 0.35.3, Chromium headless shell 151.0.7922.34
- SVG: MathJax 4.1.3, MathJax New Computer Modern font 4.1.3, resvg-js 2.6.2

## Method

Each candidate used its nested experiment package and the same release corpus. Representative PNG files were written only to temporary directories for visual review.

Browser candidate setup:

```sh
cd scripts/experiments/renderer-browser
npm install --no-audit --no-fund
PLAYWRIGHT_BROWSERS_PATH=<temporary-browser-directory> npx playwright install --only-shell chromium
PLAYWRIGHT_BROWSERS_PATH=<temporary-browser-directory> node run.mjs ../../../tests/fixtures/renderer/formula-corpus.json
```

SVG candidate setup:

```sh
cd scripts/experiments/renderer-svg
npm install --no-audit --no-fund
node run.mjs ../../../tests/fixtures/renderer/formula-corpus.json
```

The scripts use only locally installed assets after setup. The browser path embeds local KaTeX CSS and permits local font files while aborting page HTTP(S) requests. The SVG path uses MathJax's lite DOM and SVG output, rejects external SVG references before resvg, and calls `MathJax.done()`.

## Results

Runtime values are medians of three runs unless noted otherwise.

| Metric                                    | KaTeX + Playwright + Sharp | MathJax SVG + resvg |
| ----------------------------------------- | -------------------------: | ------------------: |
| Valid corpus                              |                        8/8 |                 8/8 |
| Invalid corpus rejected by raw parser     |                        3/3 |                 2/3 |
| Process start to first PNG                |                   371.0 ms |            423.5 ms |
| First render after backend initialization |                    85.4 ms |            327.7 ms |
| Warm case latency                         |                    42.7 ms |            207.2 ms |
| Full startup and corpus                   |                   870.6 ms |          2,997.3 ms |
| Node peak RSS                             |                  157.7 MiB |           431.1 MiB |
| Total PNG bytes for eight cases           |                     25,528 |              25,036 |
| Median PNG bytes                          |                      3,508 |               3,517 |
| Installed package tree                    |                   50.5 MiB |           115.3 MiB |
| Additional browser artifact               |                  198.5 MiB |                None |
| Total installed footprint                 |                  249.0 MiB |           115.3 MiB |
| Clean package install                     |                     1.70 s |              3.84 s |
| Additional browser download/install       |                     9.71 s |                None |
| Network attempts observed while rendering |                          0 |                   0 |

The browser process tree used approximately 324.3 MiB of RSS while held after the corpus. Adding the median Node RSS gives an approximate aggregate snapshot of 482 MiB. The SVG candidate used about 431 MiB in one Node process. These values are not a synchronized cross-platform peak and must not be used as a release resource guarantee.

## Correctness and Visual Review

The browser candidate produced one padded PNG per corpus case, including cases with multiple formulas. Powers, fractions, roots, sums, integrals, aligned equations, matrices, Greek letters, Unicode relations, and multiline layouts were readable and correctly arranged.

The SVG candidate produced correct individual formula shapes, but the prototype emitted one PNG per formula rather than one combined image per case. Its output also had no outer padding, so several glyphs touched image edges. The unknown-command invalid case was rendered rather than rejected. These are contract-parity gaps, even though the valid individual formulas were visually readable.

Both raw backends need the shared production input gate for URL-capable and HTML-extension commands. KaTeX with `trust: false` kept all four security cases inert and issued no page network request. The SVG harness rejected one external-reference result and rendered three cases inert; it observed no Node network attempt. Rendering alone did not satisfy the corpus requirement that all four inputs map to `invalid_latex`.

## Packaging and Cleanup

The browser package tree contained one Sharp native addon and required the Chromium headless executable. The SVG tree contained one resvg native addon. Only macOS arm64 artifacts were executed in this experiment; other platforms remain unverified.

Both harnesses exited normally. A post-run process check found no process whose executable or arguments referenced either experiment directory. The browser harness explicitly closed its page, context, and browser. The SVG harness explicitly called `MathJax.done()` to stop MathJax workers.

## Primary References

- [KaTeX options and trust policy](https://katex.org/docs/options)
- [KaTeX security guidance](https://katex.org/docs/security)
- [Playwright library and browser lifecycle](https://playwright.dev/docs/library)
- [MathJax components in Node](https://docs.mathjax.org/en/latest/server/components.html)
- [MathJax SVG output](https://docs.mathjax.org/en/latest/output/svg.html)
- [resvg-js repository and Node API](https://github.com/thx/resvg-js)

## Remaining Decision Input

T-403 must select one backend and record the packaging tradeoff. The browser candidate met the current output contract and was materially faster, but its installed footprint and process tree were larger. The SVG candidate was smaller on disk but did not reach contract parity in this prototype.
