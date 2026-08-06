#!/usr/bin/env python3
"""Stream a synthetic agent answer to stdout for tmath agent capture demos."""

from __future__ import annotations

import sys
import time

# (text, pause-after-seconds). Pauses must exceed tmath agent --wait-ms so the
# viewer receives incremental ReplaceTail updates between chunks.
ANSWERS: dict[str, list[tuple[str, float]]] = {
    "long": [
        ("# Quadratic equations — reference sheet", 0.45),
        ("", 0.2),
        ("The standard form is $ax^2+bx+c=0$ with $a \\neq 0$.", 0.45),
        ("", 0.2),
        ("## Quadratic formula", 0.35),
        ("$$x = ", 0.35),
        ("\\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}", 0.65),
        ("$$", 0.7),
        ("Use it when factoring is awkward.", 0.4),
        ("", 0.2),
        ("## Discriminant", 0.35),
        ("Define $\\Delta = b^2 - 4ac$.", 0.45),
        ("$$\\Delta > 0 \\Rightarrow \\text{two real roots},\\quad", 0.4),
        ("\\Delta = 0 \\Rightarrow \\text{one repeated root},\\quad", 0.4),
        ("\\Delta < 0 \\Rightarrow \\text{no real roots}", 0.55),
        ("$$", 0.65),
        ("", 0.2),
        ("## Vertex form", 0.35),
        ("Completing the square gives", 0.4),
        ("$$y = a(x-h)^2 + k", 0.45),
        ("$$", 0.65),
        ("The vertex is $(h,k)$ and the axis of symmetry is $x=h$.", 0.45),
        ("", 0.2),
        ("## Completing the square", 0.35),
        ("Starting from $ax^2+bx+c=0$:", 0.4),
        ("$$x^2 + \\frac{b}{a}x = -\\frac{c}{a}", 0.45),
        ("$$\\left(x+\\frac{b}{2a}\\right)^2 = \\frac{b^2-4ac}{4a^2}", 0.45),
        ("$$x = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}", 0.55),
        ("$$", 0.65),
        ("", 0.2),
        ("## Vieta's formulas", 0.35),
        ("For roots $r_1$ and $r_2$:", 0.4),
        ("$$r_1 + r_2 = -\\frac{b}{a}, \\qquad r_1 r_2 = \\frac{c}{a}", 0.45),
        ("$$", 0.65),
        ("", 0.2),
        ("## Example", 0.35),
        ("Solve $2x^2 - 4x - 6 = 0$.", 0.4),
        ("Here $a=2$, $b=-4$, $c=-6$, so $\\Delta = 16+48 = 64$.", 0.45),
        ("$$x = \\frac{4 \\pm 8}{4} = \\frac{1}{2} \\pm 2", 0.45),
        ("$$", 0.65),
        ("Therefore $x = \\tfrac{5}{2}$ or $x = -\\tfrac{3}{2}$.", 0.4),
    ],
    "answer1": [
        ("The quadratic formula solves ", 0.35),
        ("$ax^2+bx+c=0$:", 0.55),
        ("", 0.25),
        ("$$x = ", 0.45),
        ("\\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}", 0.65),
        ("$$", 0.75),
        ("", 0.25),
        ("Use it when factoring is awkward.", 0.45),
        ("The discriminant $b^2-4ac$ tells you how many real roots exist.", 0.4),
    ],
    "answer2": [
        ("Vertex form:", 0.4),
        ("", 0.25),
        ("$$y = ", 0.4),
        ("a(x-h)^2 + k", 0.55),
        ("$$", 0.7),
        ("", 0.25),
        ("The vertex is at $(h,k)$.", 0.45),
        ("For example, $y = 2(x-3)^2 + 1$ has vertex $(3,1)$.", 0.4),
    ],
}


def emit(text: str) -> None:
    if text:
        sys.stdout.write(text)
    sys.stdout.write("\n")
    sys.stdout.flush()


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ANSWERS:
        names = "|".join(sorted(ANSWERS))
        print(f"usage: {sys.argv[0]} {names}", file=sys.stderr)
        return 2

    for chunk, pause in ANSWERS[sys.argv[1]]:
        emit(chunk)
        time.sleep(pause)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
