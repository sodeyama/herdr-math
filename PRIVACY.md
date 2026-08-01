# Privacy

Herdr Math is designed for local processing and has no telemetry in v0.1.

## Data processed

The plugin temporarily reads bounded recent output from a Herdr coding-agent pane. It uses that text in memory to
prove the current response boundary, scan supported math delimiters, and render an image.

It may also process pane ids, workspace ids, canonical agent ids, lifecycle status, pane revision, event sequence,
cell dimensions, layout dimensions, and plugin-owned viewer metadata.

## Data retained

Durable state contains keyed cryptographic fingerprints, bounded structural metadata, generation counters,
processed-content digests, and owned viewer ids. It does not contain raw pane output, answer text, selected text,
or LaTeX source.

Logs are restricted to timestamps, outcomes, stable error codes, bounded counts, byte sizes, timing, and other
allowlisted diagnostics. They do not contain raw events, environment dumps, pane text, or equations.

Herdr owns the plugin state and config directories. Local unlink retains those directories in Herdr 0.7.5. The
managed tagged uninstall retention behavior will be documented after the immutable-tag release test.

## Network behavior

Herdr Math makes no runtime network request and uploads no pane content, equation, image, log, or telemetry. KaTeX,
fonts, Chromium, and rendering assets are local at runtime. Remote resource loading is disabled during rendering.

Installation uses npm and Playwright to download locked packages, Chromium headless shell, and FFmpeg artifacts.
Those installation tools have their own network and privacy behavior. Review the manifest commands and source ref
before installation.

## User controls

- Disable or uninstall the plugin through Herdr to stop event processing.
- Close an owned viewer without losing the source pane.
- Run the privacy-safe `diagnose` action to inspect capability status.
- Report a privacy or security defect through the private process in [SECURITY.md](SECURITY.md).

Herdr Math cannot control retention performed by Herdr, the coding agent, the outer terminal, shell history, or
operating-system logging.

