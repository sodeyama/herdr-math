# Documentation

This directory separates product intent, target design, and historical evidence so that planned
behavior is not confused with verified behavior.

## Documents

- [Concept](concept.md) explains the user problem, product promise, scope, naming, compatibility
  boundary, privacy model, and success criteria.
- [Architecture](architecture.md) defines the target two-process design: the Rust terminal
  frontend, the one-shot TypeScript render subprocess, the `tmath-render/1` IPC, placement,
  input decoding, scroll, and the fail-closed error model.
- [Getting started](getting-started.md) documents build, first use, diagnostics, and known
  limits.
- [Compatibility](compatibility.md) records the verified macOS, terminal, and platform scope.
- [Experiment report](experiment-report.md) records what was tested and which prototype decisions
  must change.
- [Phase evidence](evidence/) records per-phase results for the terminal surface, render
  transport, placement, input loop, CLI and composition, and hardening.

## Specification

The implementation contract lives outside this directory:

- [Acceptance tests](../specs/terminal-math-v2/tests/main.md)
- [Implementation plan](../specs/terminal-math-v2/plans/main.md)
- [Task list](../specs/terminal-math-v2/tasks/main.md)

The superseded V1 Herdr plugin contract remains under
[`../specs/herdr-math-v1/`](../specs/herdr-math-v1/) as historical reference.

When these documents disagree, follow the precedence in [AGENTS.md](../AGENTS.md).

## Evidence Labels

The documents use four labels:

- **Verified**: observed in the recorded implementation run.
- **Target**: required behavior for the public implementation.
- **Planned**: work accepted into the V2 plan but not yet implemented.
- **Open**: a decision or compatibility claim that still requires evidence.

Do not convert a planned or open claim into a release statement without updating the acceptance
tests and attaching evidence.
