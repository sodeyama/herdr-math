# Herdr Math

Render LaTeX from AI agent responses in a side pane.

Herdr Math is a planned public [Herdr](https://herdr.dev/) plugin. When a supported coding agent finishes a response, the plugin extracts inline and display equations, renders them locally, and updates a reusable viewer pane without taking focus away from the agent.

## Status

This repository currently contains the product design, experiment evidence, target architecture, and v1 implementation plan. It is **not yet an installable release**.

A local prototype proved the core path on August 1, 2026:

```text
agent completion
  -> current-answer boundary detection
  -> LaTeX scanning
  -> local PNG rendering
  -> Herdr pane.graphics.set
  -> reusable side pane
```

The production implementation will be built in this repository from the specifications below. Prototype code will be ported selectively rather than copied as-is.

## Design Principles

- Local-only processing: no pane content or equations leave the machine.
- Fail-closed answer detection: uncertain historical content is not rendered.
- One viewer per source pane: repeated answers update the same split.
- Safe rendering: no TeX executable, shell evaluation, or trusted remote content.
- Herdr-native lifecycle: event hooks and Herdr-managed plugin state.
- Honest compatibility: Ghostty is a verified terminal, not a direct dependency.

## Documentation

- [Concept and product boundaries](docs/concept.md)
- [Target architecture](docs/architecture.md)
- [August 2026 experiment report](docs/experiment-report.md)
- [Documentation index](docs/README.md)

## Implementation Specification

The canonical v1 specification is split into three synchronized documents:

- [Acceptance tests](specs/herdr-math-v1/tests/main.md)
- [Implementation plan](specs/herdr-math-v1/plans/main.md)
- [Executable task list](specs/herdr-math-v1/tasks/main.md)

## Compatibility Direction

The current 0.1.0 development build is verified with Herdr 0.7.5 on macOS arm64 using Ghostty 1.3.1 and Herdr's experimental Kitty graphics support. The first release will declare only combinations that pass the release matrix. See [Compatibility](docs/compatibility.md) for verified and unverified combinations.

Herdr Math does not call Ghostty APIs. It uses Herdr's plugin and pane graphics APIs; the attached outer terminal must support the image path used by Herdr.

## Contributing

Read [AGENTS.md](AGENTS.md) before making changes. Public documentation, code comments, logs, commits, and release material are written in English.

## Official Herdr References

- [Plugins](https://herdr.dev/docs/plugins/)
- [Socket API](https://herdr.dev/docs/socket-api/)
- [Marketplace](https://herdr.dev/docs/marketplace/)
