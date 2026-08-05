#!/usr/bin/env python3
"""Scroll-lab feeder: append synthetic assistant messages to the lab
transcript so the watcher streams them into the viewer. No real
transcript is read or modified."""
import json, sys, time
from pathlib import Path

def _lab_path():
    import os
    slug = os.getcwd().replace("/", "-")
    return Path.home() / ".claude/projects" / slug / "zz-scrolllab.jsonl"

LAB = _lab_path()

def emit(text):
    line = json.dumps({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}, ensure_ascii=False)
    with LAB.open("a") as f:
        f.write(line + "\n")

def block(i):
    kinds = [
        f"Block {i}: paragraph with inline math $a_{{{i}}}^2 + b_{{{i}}}^2 = c_{{{i}}}^2$ and Japanese 段落テキスト確認用。",
        f"$$\\int_0^{{{i}}} x^2\\,dx = \\frac{{{i}^3}}{{3}}$$",
        f"Block {i}: **bold intro** — 続く本文でリズムを確認する。 `inline_code_{i}` も含む。",
        f"$$\\sum_{{k=1}}^{{{i}}} k = \\frac{{{i}({i}+1)}}{{2}}$$",
    ]
    return kinds[i % 4]

if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "init":
        LAB.write_text("")
        print(f"lab transcript: {LAB}")
    elif cmd == "blocks":
        n = int(sys.argv[2]); delay = float(sys.argv[3]) if len(sys.argv) > 3 else 0.3
        for i in range(1, n + 1):
            emit(block(i))
            time.sleep(delay)
        print(f"fed {n} blocks")
    elif cmd == "clean":
        LAB.unlink(missing_ok=True)
        print("removed")
