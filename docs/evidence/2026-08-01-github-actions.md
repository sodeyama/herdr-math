# GitHub Actions Evidence

Date: 2026-08-01

## Workflow

Commit `eb8453d` added one macOS arm64 CI job for pushes to `main`, pull requests, and manual dispatch. The job has
read-only repository permissions, branch/ref concurrency cancellation, and a 30-minute timeout.

The job uses the standard `macos-14` arm64 runner and fails if `uname -m` is not `arm64`. GitHub's official
[hosted-runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) identifies
that label as an arm64 runner.

Official actions are pinned to immutable commits with their audited release versions in comments:

- `actions/checkout` v6.0.2;
- `actions/setup-node` v6.4.0; and
- Node.js 22.21.1 with npm lockfile caching.

`pinact run --check` passed for the workflow.

## Gates

The job runs:

1. architecture verification;
2. `npm ci`;
3. every build command declared by `herdr-plugin.toml`;
4. manifest, type, lint, format, runtime dependency, and security checks;
5. unit tests;
6. Herdr contract tests;
7. integration tests;
8. the complete test suite;
9. real renderer smoke tests;
10. a clean rebuild; and
11. `npm pack --dry-run --json` release-tree validation.

The workflow does not upload state, logs, screenshots, pane content, or package artifacts.

## First real run

[GitHub Actions run 30712318126](https://github.com/sodeyama/herdr-math/actions/runs/30712318126) completed in 57
seconds on commit `eb8453d`. Every setup, install, build, static, security, unit, contract, integration, complete-suite,
renderer, rebuild, and package-content step passed.

## Acceptance result

- AT-009 passed in CI with locked installation, build, rebuild, and package validation.
- AT-011 version and manifest checks passed.
- AT-700 passed on the declared macOS arm64 CI architecture for automated gates.
- AT-708 and AT-709 dependency, notice, security, and release-tree gates passed.
- Real Herdr, coding-agent, Ghostty, and restart evidence remains runtime evidence rather than a CI simulation.

