# Privacy

Terminal Math is designed for local processing and has no telemetry.

## Data processed

The tool temporarily reads bounded document text from a file, a pipe, or stdin. It uses that text
in memory to scan supported math delimiters and render one or more images. It may also process
byte sizes, image dimensions, cell dimensions, placement counts, and stable error codes.

## Data retained

No raw document text, formula source, or rendered byte content is written to durable state or
logs. Logs and diagnostics are restricted to timestamps, outcomes, stable error codes, bounded
counts, byte sizes, timing, and other allowlisted metadata. Runtime artifacts are kept outside
the repository.

## Network behavior

Terminal Math makes no runtime network request and uploads no document content, equation, image,
log, or telemetry. KaTeX, fonts, Chromium, and rendering assets are local at runtime; remote
resource loading is disabled during rendering.

Installation uses npm to download locked packages and Playwright to install a Chromium headless
shell. Those installation tools have their own network and privacy behavior.

## User controls

- Read a document from stdin instead of disk; nothing is written back.
- Run `tmath diagnose` to inspect capability status locally.
- Report a privacy or security defect through the private process in [SECURITY.md](SECURITY.md).

Terminal Math cannot control retention performed by the outer terminal, shell history, or
operating-system logging.
