# Documentation

This directory separates product intent, target design, and historical evidence so that planned behavior is not confused with verified behavior.

## Documents

- [Concept](concept.md) explains the user problem, product promise, scope, naming, compatibility boundary, privacy model, and success criteria.
- [Architecture](architecture.md) defines the target v1 components, event lifecycle, state model, viewer ownership, rendering boundary, failure behavior, and packaging model.
- [Getting started](getting-started.md) documents verified local setup, first use, diagnostics, update, unlink, and release-pending tagged commands.
- [Compatibility](compatibility.md) records the verified Herdr, macOS architecture, terminal, and coding-agent release-candidate scope.
- [Experiment report](experiment-report.md) records what was tested on August 1, 2026, what passed, what failed, and which prototype decisions must change before release.
- [Coding-agent lifecycle evidence](evidence/2026-08-01-agent-lifecycle.md) records redacted real-session results for Claude Code, Codex, Pi, and OpenCode.
- [Renderer decision](decisions/0001-v1-renderer.md) selects the v0.1 local rendering backend from measured candidates and records its security, packaging, and compatibility constraints.
- [Renderer candidate measurements](evidence/2026-08-01-renderer-candidates.md) records the fixed-corpus comparison used by the renderer decision.
- [Performance evidence](evidence/2026-08-01-performance.md) records worker, boundary, renderer, memory, image, and cleanup regression budgets.
- [Automated release evidence](evidence/2026-08-01-automated-release.md) records the clean-checkout automated P0 suite and reproducible build comparison.
- [Ghostty runtime evidence](evidence/2026-08-01-ghostty-runtime.md) records the four-agent, viewer, resize, failure, and graphics-capability matrix.
- [Named-session restart evidence](evidence/2026-08-01-session-restart.md) records state isolation, stale-lock cleanup, and restart recovery.
- [macOS arm64 evidence](evidence/2026-08-01-platform-macos-arm64.md) records fresh installation and native artifact verification.
- [Licensing and notices](licensing.md) defines the project license, prototype boundary, dependency policy, and release notice gate.

## Specification

The implementation contract lives outside this directory:

- [Acceptance tests](../specs/herdr-math-v1/tests/main.md)
- [Implementation plan](../specs/herdr-math-v1/plans/main.md)
- [Task list](../specs/herdr-math-v1/tasks/main.md)

When these documents disagree, follow the precedence in [AGENTS.md](../AGENTS.md).

## Evidence Labels

The documents use four labels:

- **Verified**: observed in the August 1, 2026 prototype or in a later recorded test.
- **Target**: required behavior for the public implementation.
- **Planned**: work accepted into the v1 plan but not yet implemented.
- **Open**: a decision or compatibility claim that still requires evidence.

Do not convert a planned or open claim into a release statement without updating the acceptance tests and attaching evidence.
