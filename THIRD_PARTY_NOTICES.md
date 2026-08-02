# Third-Party Notices

Herdr Math is MIT licensed. The renderer also installs the third-party components below. Each component remains under its own license.

`npm ci` installs JavaScript packages and native Sharp artifacts from the npm registry. Its postinstall step runs `npm run install:browser`, which installs the Playwright-managed Chromium headless shell and companion FFmpeg artifact under `node_modules/playwright-core/.local-browsers`. Runtime rendering performs no downloads.

The installed packages retain their complete license and notice files under `node_modules`. The Chromium artifact retains `LICENSE.headless_shell` next to the executable. This inventory does not replace those license texts.

## Renderer Packages

| Component                                            | Locked version | License                                          | Retained license or notice                              |
| ---------------------------------------------------- | -------------: | ------------------------------------------------ | ------------------------------------------------------- |
| KaTeX and its packaged fonts                         |         0.18.1 | MIT                                              | `node_modules/katex/LICENSE`                            |
| commander                                            |          8.3.0 | MIT                                              | `node_modules/commander/LICENSE`                        |
| Playwright                                           |         1.62.1 | Apache-2.0                                       | `node_modules/playwright/LICENSE` and `NOTICE`          |
| Playwright Core                                      |         1.62.1 | Apache-2.0                                       | `node_modules/playwright-core/LICENSE` and `NOTICE`     |
| fsevents                                             |          2.3.2 | MIT                                              | `node_modules/playwright/node_modules/fsevents/LICENSE` |
| Sharp                                                |         0.35.3 | Apache-2.0                                       | `node_modules/sharp/LICENSE`                            |
| @img/colour                                          |          1.1.0 | MIT                                              | `node_modules/@img/colour/LICENSE.md`                   |
| detect-libc                                          |          2.1.2 | Apache-2.0                                       | `node_modules/detect-libc/LICENSE`                      |
| semver                                               |          7.8.5 | ISC                                              | `node_modules/semver/LICENSE`                           |
| Sharp macOS native addon, arm64 and x64 lock entries |         0.35.3 | Apache-2.0                                       | Native package `LICENSE`                                |
| Sharp libvips bundle, arm64 and x64 lock entries     |          1.3.2 | LGPL-3.0-or-later and bundled component licenses | Native package `README.md` licensing inventory          |
| markdown-it                                         |         14.3.0 | MIT                                              | `node_modules/markdown-it/LICENSE`                      |
| highlight.js                                        |         11.11.1 | BSD-3-Clause                                     | `node_modules/highlight.js/LICENSE`                     |

markdown-it is MIT licensed and parses Markdown text into safe HTML with raw HTML disabled. highlight.js is BSD-3-Clause licensed and provides the code-block syntax highlighting theme used by the renderer.

The npm 10 optional-dependency resolver may also install the locked `@img/sharp-wasm32` 0.35.3 package and its `@emnapi/runtime` and `tslib` dependencies on macOS. They are not selected at runtime when the native macOS addon loads. Their package metadata declares `Apache-2.0 AND LGPL-3.0-or-later AND MIT`, MIT, and 0BSD respectively, and their installed license files are retained.

The Sharp libvips bundle contains dynamically linked or bundled image libraries under permissive, MPL-2.0, and LGPL licenses. Its installed `README.md` records the exact component inventory, including libvips, glib, librsvg, pango, libexif, libheif, fontconfig, freetype, libpng, libtiff, libwebp, and their applicable terms. Corresponding source and build materials are published by the [sharp-libvips project](https://github.com/lovell/sharp-libvips).

## KaTeX Fonts

KaTeX 0.18.1 contains 60 TTF, WOFF, and WOFF2 font files referenced by its packaged CSS. The package provides one MIT `LICENSE` covering the distributed package and no separate font license. Herdr Math loads these files directly from the installed KaTeX package and does not copy or modify them.

KaTeX documents the packaged formats and the expected sibling `fonts` directory in its [font documentation](https://github.com/KaTeX/KaTeX/blob/v0.18.1/docs/font.md).

## Playwright Notice

Playwright is copyright Microsoft Corporation and contains code derived from the Puppeteer project under the Apache License 2.0. The complete upstream notice remains in both installed Playwright packages.

## Chromium Headless Shell

Playwright 1.62.1 locks Chromium headless shell revision 1234, browser version 151.0.7922.34. The browser download contains `LICENSE.headless_shell`, including the Chromium BSD terms and bundled third-party license texts. The installation audit requires this file to remain next to the executable.

Chromium is not copied into this Git repository or npm source package. If a distributor bundles the browser artifact separately, that distributor must include its complete `LICENSE.headless_shell` file without truncation.

## Playwright FFmpeg Artifact

The Playwright installation also downloads FFmpeg revision 1011 even though Herdr Math does not request video recording. The artifact retains the complete LGPL-2.1 license in `COPYING.LGPLv2.1`. The installation audit requires both the executable and this license file to be present.
