# Evidence: Embedded-Font Cold Start of the Native Engine (AT-3-002)

- Date: August 4, 2026
- Environment: macOS arm64 (Darwin 25.5.0), release build, offline
- Harness: `scripts/experiments/native-engine-spike` (commit `a7ed163`),
  `tests/coldstart.rs` + `src/bin/coldstart.rs`
- Reproduce: `cargo test --offline --test coldstart` inside the spike crate;
  samples land in the crate-local `out/coldstart-summary.json`.

## Result: PASS (budget 300 ms p50; measured ~9-12 ms p50)

Protocol: build `coldstart` once in release mode, one unmeasured warmup spawn,
then 10 measured spawns. Wall-clock spans process spawn to child exit (includes
OS process startup); the in-process timer starts at the first statement of
`main`. The rendered "first block" is a bold paragraph with one RaTeX inline
formula plus a display formula, composed through Typst with embedded fonts only
and encoded as a transparent PNG at dpr 2 (19,177 bytes each run).

| Metric | codex run p50 / p95 | supervisor re-run p50 / p95 |
|---|---|---|
| Wall-clock (spawn → exit) | 8.8 / 10.2 ms | 12.4 / 17.1 ms |
| In-process total | 4.6 / 5.7 ms | 5.0 / 8.0 ms |
| Engine build (incl. RaTeX assets) | ~2.4 ms | ~2.5 ms |
| First render (Typst compile + raster + PNG) | ~2.2 ms | ~2.3 ms |

Both runs sit more than 24x inside the 300 ms budget. The first-ever spawn
(cold executable/filesystem caches) measured ~640-700 ms wall-clock with the
same ~10 ms in-process time, which is why AT-3-002 specifies warm OS caches.

## System-font-scan exclusion

Verification is structural, not syscall-traced (no fs tracing in the build
sandbox): both spike binaries construct font options through one shared
`embedded_font_options()` site with `include_system_fonts(false)` and
`include_embedded_fonts(true)`, guarded by debug assertions; the engine exposes
17 embedded font faces (asserted > 0) and the compiled document is checked to
use at least one. AT-3-002's fs-trace variant remains for the Phase 1
implementation where the real `tmath` binary exists.

## Notes

- The ~4-8 ms spawn-to-main gap is executable loading and runtime startup of
  the 40 MB-class spike binary; it bounds what a resident engine avoids
  entirely on subsequent renders.
- Numbers are host- and load-dependent; the supervisor re-run happened under
  background load and still passed with 24x margin.
