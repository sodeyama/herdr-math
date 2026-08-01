# Changelog

All notable changes to Herdr Math will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Herdr plugin manifest with one-shot startup and lifecycle event hooks.
- Claude Code, Codex CLI, Pi, and OpenCode completion support.
- Fail-closed answer-boundary fingerprints for appended and alternate-screen responses.
- Conservative `$...$` and `$$...$$` scanning.
- Local KaTeX, Chromium, and Sharp PNG rendering with network denial and strict limits.
- One reusable Herdr-owned viewer per source pane with focus preservation.
- Session-scoped atomic state, stale-lock cleanup, diagnostics, and pane-close recovery.
- Automated unit, contract, integration, rendering, performance, privacy, security, and manifest gates.
- Real Herdr 0.7.5, Ghostty 1.3.1, macOS arm64, four-agent, and named-session restart evidence.

### Security

- Raw pane output, answers, and LaTeX source are excluded from durable state and logs.
- Remote resources, trusted links, TeX execution, shell evaluation, and input-selected executable paths are denied.

No version has been released. The `0.1.0` heading will be added only when the release commit is prepared.

