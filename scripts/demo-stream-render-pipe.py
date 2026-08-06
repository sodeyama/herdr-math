#!/usr/bin/env python3
"""Stream demo-render-pipe.md to stdout for `tmath render -` GIF demos."""

from __future__ import annotations

import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DOC = ROOT / "demo-render-pipe.md"

# (line, pause-after-seconds). Short pauses let native_stream render tail growth
# incrementally while the pipe is still open.
PAUSE_AFTER_HEADING = 0.35
PAUSE_AFTER_DISPLAY = 0.55
PAUSE_AFTER_INLINE = 0.3
PAUSE_BLANK = 0.15


def pause_for(line: str) -> float:
    stripped = line.strip()
    if not stripped:
        return PAUSE_BLANK
    if stripped.startswith("#"):
        return PAUSE_AFTER_HEADING
    if stripped.startswith("$$"):
        return PAUSE_AFTER_DISPLAY
    if "$" in stripped:
        return PAUSE_AFTER_INLINE
    return PAUSE_AFTER_HEADING


def main() -> int:
    text = DOC.read_text(encoding="utf-8")
    for line in text.splitlines():
        sys.stdout.write(line)
        sys.stdout.write("\n")
        sys.stdout.flush()
        time.sleep(pause_for(line))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
