#!/usr/bin/env python3
"""Stream a synthetic agent answer to stdout for tmath agent capture demos."""

from __future__ import annotations

import sys
import time

# (text, pause-after-seconds). Pauses must exceed tmath agent --wait-ms so the
# viewer receives incremental ReplaceTail updates between chunks.
ANSWERS: dict[str, list[tuple[str, float]]] = {
    "bayes-en": [
        ("# Bayesian inference — quick reference", 0.45),
        ("", 0.2),
        (
            "Bayes' theorem relates the **posterior** to the **prior** and "
            "**likelihood**:",
            0.45,
        ),
        ("", 0.2),
        ("$$P(\\theta \\mid D) = \\frac{P(D \\mid \\theta)\\, P(\\theta)}{P(D)}$$", 0.7),
        ("", 0.2),
        ("Here $\\theta$ is a parameter, $D$ is observed data, and $P(D)$ is the evidence.", 0.45),
        ("", 0.2),
        ("## Beta–Binomial conjugate pair", 0.35),
        (
            "For $n$ trials with $k$ successes, use a $\\mathrm{Beta}(\\alpha,\\beta)$ prior on "
            "$p$:",
            0.45,
        ),
        ("$$p \\sim \\mathrm{Beta}(\\alpha,\\beta), \\qquad X \\mid p \\sim \\mathrm{Binomial}(n,p)$$", 0.65),
        ("", 0.2),
        ("The posterior is", 0.35),
        ("$$p \\mid X=k \\;\\sim\\; \\mathrm{Beta}(\\alpha + k,\\; \\beta + n - k)$$", 0.7),
        ("", 0.2),
        ("Posterior mean and variance:", 0.4),
        ("$$\\mathbb{E}[p \\mid k] = \\frac{\\alpha + k}{\\alpha + \\beta + n}$$", 0.55),
        (
            "$$\\mathrm{Var}(p \\mid k) = \\frac{(\\alpha+k)(\\beta+n-k)}"
            "{(\\alpha+\\beta+n)^2(\\alpha+\\beta+n+1)}$$",
            0.65,
        ),
        ("", 0.2),
        ("## Worked example", 0.35),
        ("Observed $k=7$ heads in $n=10$ flips. Prior $\\mathrm{Beta}(2,2)$ (weakly informative).", 0.45),
        ("$$\\alpha' = 2+7 = 9, \\qquad \\beta' = 2+3 = 5$$", 0.45),
        ("$$\\mathbb{E}[p \\mid k] = \\frac{9}{14} \\approx 0.643$$", 0.55),
        ("", 0.2),
        ("A 95% **credible interval** uses the posterior quantiles $[q_{0.025}, q_{0.975}]$.", 0.45),
        ("", 0.2),
        ("## MAP and predictive density", 0.35),
        ("The **MAP** estimate maximizes the posterior (mode of $\\mathrm{Beta}(9,5)$).", 0.45),
        ("For new data $x_{\\mathrm{new}}$:", 0.4),
        (
            "$$P(x_{\\mathrm{new}} \\mid D) = \\int P(x_{\\mathrm{new}} \\mid p)\\, P(p \\mid D)\\, dp$$",
            0.65,
        ),
        ("", 0.2),
        ("With a Beta–Binomial model this integral is closed form (**posterior predictive**).", 0.45),
        ("", 0.2),
        ("## When to prefer Bayes", 0.35),
        ("- Small samples: priors stabilize estimates.", 0.4),
        ("- Sequential learning: today's posterior is tomorrow's prior.", 0.4),
        ("- Uncertainty quantification: full distributions, not point estimates alone.", 0.4),
    ],
    "bayes-ja": [
        ("# ベイズ統計 — 要点まとめ", 0.45),
        ("", 0.2),
        ("**事後分布**は **事前分布** と **尤度** から更新される:", 0.45),
        ("", 0.2),
        ("$$P(\\theta \\mid D) = \\frac{P(D \\mid \\theta)\\, P(\\theta)}{P(D)}$$", 0.7),
        ("", 0.2),
        ("$\\theta$ は未知パラメータ、$D$ は観測データ、$P(D)$ は周辺尤度（証拠）である。", 0.45),
        ("", 0.2),
        ("## ベータ–二項の共役ペア", 0.35),
        ("$n$ 回試行で成功 $k$ 回のとき、成功確率 $p$ に $\\mathrm{Beta}(\\alpha,\\beta)$ 事前を置く:", 0.45),
        ("$$p \\sim \\mathrm{Beta}(\\alpha,\\beta), \\qquad X \\mid p \\sim \\mathrm{Binomial}(n,p)$$", 0.65),
        ("", 0.2),
        ("事後分布は", 0.35),
        ("$$p \\mid X=k \\;\\sim\\; \\mathrm{Beta}(\\alpha + k,\\; \\beta + n - k)$$", 0.7),
        ("", 0.2),
        ("事後平均と事後分散:", 0.4),
        ("$$\\mathbb{E}[p \\mid k] = \\frac{\\alpha + k}{\\alpha + \\beta + n}$$", 0.55),
        (
            "$$\\mathrm{Var}(p \\mid k) = \\frac{(\\alpha+k)(\\beta+n-k)}"
            "{(\\alpha+\\beta+n)^2(\\alpha+\\beta+n+1)}$$",
            0.65,
        ),
        ("", 0.2),
        ("## 数値例", 0.35),
        ("$n=10$ 回中 $k=7$ 回成功。事前 $\\mathrm{Beta}(2,2)$（弱情報事前）。", 0.45),
        ("$$\\alpha' = 2+7 = 9, \\qquad \\beta' = 2+3 = 5$$", 0.45),
        ("$$\\mathbb{E}[p \\mid k] = \\frac{9}{14} \\approx 0.643$$", 0.55),
        ("", 0.2),
        ("95% **信用区間**は事後分布の分位点 $[q_{0.025}, q_{0.975}]$ で与える。", 0.45),
        ("", 0.2),
        ("## MAP と予測分布", 0.35),
        ("**MAP推定**は事後分布を最大化する点（$\\mathrm{Beta}(9,5)$ の最頻値）。", 0.45),
        ("新しい観測 $x_{\\mathrm{new}}$ の予測:", 0.4),
        (
            "$$P(x_{\\mathrm{new}} \\mid D) = \\int P(x_{\\mathrm{new}} \\mid p)\\, P(p \\mid D)\\, dp$$",
            0.65,
        ),
        ("", 0.2),
        ("ベータ–二項モデルではこの積分は閉形式（**事後予測分布**）。", 0.45),
        ("", 0.2),
        ("## ベイズ推論が向く場面", 0.35),
        ("- サンプルが小さい: 事前分布で推定を安定化。", 0.4),
        ("- 逐次学習: 今日の事後が明日の事前になる。", 0.4),
        ("- 不確実性の明示: 点推定だけでなく分布全体を得る。", 0.4),
    ],
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
