"""Small-sample statistics for paired agent runs.

Agent runs are expensive, so a suite has a handful of runs per arm, not
hundreds. The numbers here suit that: medians and quartiles rather than
means, paired differences (each round runs both arms on the same task), and
an exact two-sided sign test that makes no assumption about the
distribution. With five pairs the smallest p-value it can give is 0.0625,
which is the honest answer for five pairs.
"""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from math import comb


def quantile(values: Sequence[float], q: float) -> float:
    """Linear interpolation between closest ranks (numpy's default)."""
    if not values:
        raise ValueError("no values")
    s = sorted(values)
    pos = (len(s) - 1) * q
    lo = int(pos)
    hi = min(lo + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (pos - lo)


def median(values: Sequence[float]) -> float:
    return quantile(values, 0.5)


def summarize(values: Iterable[float | None]) -> dict[str, float | int] | None:
    """n, median, quartiles and range, or None when there are no values."""
    vs = [float(v) for v in values if v is not None]
    if not vs:
        return None
    return {
        "n": len(vs),
        "median": median(vs),
        "p25": quantile(vs, 0.25),
        "p75": quantile(vs, 0.75),
        "min": min(vs),
        "max": max(vs),
    }


def sign_test(diffs: Sequence[float]) -> float | None:
    """Exact two-sided sign test p-value; ties are dropped."""
    nonzero = [d for d in diffs if d != 0]
    n = len(nonzero)
    if n == 0:
        return None
    k = min(sum(1 for d in nonzero if d > 0), sum(1 for d in nonzero if d < 0))
    tail = sum(comb(n, i) for i in range(k + 1)) / 2**n
    return min(1.0, 2 * tail)


def paired(plain: dict[int, float | None], jouet: dict[int, float | None]) -> dict[str, float | int | None] | None:
    """Differences jouet minus plain over the rounds both arms finished."""
    rounds = sorted(r for r in plain if r in jouet and plain[r] is not None and jouet[r] is not None)
    if not rounds:
        return None
    diffs = [float(jouet[r]) - float(plain[r]) for r in rounds]  # type: ignore[arg-type]
    base = median([float(plain[r]) for r in rounds])  # type: ignore[arg-type]
    return {
        "pairs": len(diffs),
        "median_diff": median(diffs),
        "min_diff": min(diffs),
        "max_diff": max(diffs),
        "median_change": median(diffs) / base if base else None,
        "jouet_lower": sum(1 for d in diffs if d < 0),
        "jouet_higher": sum(1 for d in diffs if d > 0),
        "p_sign": sign_test(diffs),
    }
