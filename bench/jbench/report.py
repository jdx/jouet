"""Turns run records into `summary.json` and `summary.md`."""

from __future__ import annotations

from collections import defaultdict
from collections.abc import Callable
from typing import Any

from . import stats

# (key, label, getter, format) for each compared metric. Lower is better
# for all of them.
METRICS: list[tuple[str, str, Callable[[dict[str, Any]], float | None], str]] = [
    ("input_tokens", "input tokens (all context read)", lambda r: _u(r, "input_tokens"), "tokens"),
    ("uncached_input_tokens", "input tokens not from cache", lambda r: _uncached(r), "tokens"),
    ("output_tokens", "output tokens", lambda r: _u(r, "output_tokens"), "tokens"),
    ("cost_usd", "cost (USD, as the harness reports it)", lambda r: _u(r, "cost_usd"), "usd"),
    ("requests", "model requests", lambda r: _a(r, "requests"), "count"),
    ("calls", "tool calls", lambda r: _a(r, "calls"), "count"),
    ("failed_calls", "failed tool calls", lambda r: _a(r, "failed_calls"), "count"),
    ("tool_output_bytes", "tool output (bytes)", lambda r: _a(r, "tool_output_bytes"), "bytes"),
    ("wall_s", "wall time (s)", lambda r: r.get("wall_s"), "seconds"),
]


def _u(r: dict[str, Any], key: str) -> float | None:
    return (r.get("usage") or {}).get(key)


def _a(r: dict[str, Any], key: str) -> float | None:
    a = r.get("audit")
    return a.get(key) if a else None


def _uncached(r: dict[str, Any]) -> float | None:
    u = r.get("usage")
    if not u:
        return None
    return u["input_tokens"] - u["cached_input_tokens"]


def group(records: list[dict[str, Any]]) -> dict[tuple[str, str], dict[str, list[dict[str, Any]]]]:
    out: dict[tuple[str, str], dict[str, list[dict[str, Any]]]] = defaultdict(lambda: defaultdict(list))
    for r in records:
        out[(r["task"], r["harness"])][r["arm"]].append(r)
    return out


def summarize(records: list[dict[str, Any]], meta: dict[str, Any]) -> dict[str, Any]:
    cells = []
    for (task, harness), by_arm in sorted(group(records).items()):
        cell: dict[str, Any] = {"task": task, "harness": harness, "arms": {}, "paired": {}}
        for arm, runs in sorted(by_arm.items()):
            finished = [r for r in runs if not r.get("error")]
            decisions: dict[str, int] = defaultdict(int)
            unanswered: set[str] = set()
            for r in finished:
                for k, v in ((r.get("ledger") or {}).get("decisions") or {}).items():
                    decisions[k] += v
                unanswered.update((r.get("github") or {}).get("unanswered_examples") or [])
            cell["arms"][arm] = {
                "runs": len(runs),
                "errors": [r["error"] for r in runs if r.get("error")],
                "stopped": [r["stopped"] for r in finished if r.get("stopped")],
                "successes": sum(1 for r in runs if r.get("success")),
                "metrics": {key: stats.summarize(get(r) for r in finished) for key, _, get, _ in METRICS},
                "rule_decisions": dict(sorted(decisions.items())),
                "github_unanswered": sorted(unanswered),
            }
        if "plain" in by_arm and "jouet" in by_arm:
            paired_runs = {
                arm: {r["round"]: r for r in by_arm[arm] if not r.get("error") and isinstance(r.get("success"), bool)}
                for arm in ("plain", "jouet")
            }
            rounds = sorted(paired_runs["plain"].keys() & paired_runs["jouet"].keys())
            outcomes = [(paired_runs["plain"][n]["success"], paired_runs["jouet"][n]["success"]) for n in rounds]
            cell["paired_outcomes"] = {
                "both_passed": outcomes.count((True, True)),
                "plain_only": outcomes.count((True, False)),
                "jouet_only": outcomes.count((False, True)),
                "both_failed": outcomes.count((False, False)),
            }
            passed_rounds = [n for n in rounds if paired_runs["plain"][n]["success"] and paired_runs["jouet"][n]["success"]]
            cell["paired_when_both_pass"] = {}
            for key, _, get, _ in METRICS:
                plain = {r["round"]: get(r) for r in by_arm["plain"] if not r.get("error")}
                jouet = {r["round"]: get(r) for r in by_arm["jouet"] if not r.get("error")}
                cell["paired"][key] = stats.paired(plain, jouet)
                cell["paired_when_both_pass"][key] = stats.paired(
                    {n: get(paired_runs["plain"][n]) for n in passed_rounds},
                    {n: get(paired_runs["jouet"][n]) for n in passed_rounds},
                )
        cells.append(cell)
    return {"schema": 1, "meta": meta, "cells": cells}


def fmt(value: float | None, kind: str) -> str:
    if value is None:
        return "–"
    if kind == "usd":
        return f"${value:.3f}"
    if kind == "seconds":
        return f"{value:.0f}"
    if kind == "bytes":
        return human(value) + "B"
    if kind == "tokens":
        return human(value)
    return f"{value:g}" if value != int(value) else f"{int(value)}"


def human(value: float) -> str:
    sign = "-" if value < 0 else ""
    v = abs(value)
    for unit, size in (("M", 1e6), ("k", 1e3)):
        if v >= size:
            return f"{sign}{v / size:.1f}{unit}"
    return f"{sign}{v:.0f}"


def signed(value: float | None, kind: str) -> str:
    if value is None:
        return "–"
    text = fmt(value, kind)
    return text if value < 0 or text.startswith("-") else f"+{text}"


def markdown(summary: dict[str, Any]) -> str:
    meta = summary["meta"]
    lines = ["# jouet agent benchmark", ""]
    if meta.get("label"):
        lines += [f"> **{meta['label']}**", ""]
    lines += [
        f"- **When:** {meta.get('started')} ({meta.get('rounds')} round(s), seed {meta.get('seed')})",
        f"- **jouet:** {meta.get('jouet_version')} at `{meta.get('commit')}`",
    ]
    for name, info in (meta.get("harnesses") or {}).items():
        lines.append(
            f"- **{name}:** {info.get('version')}, model `{info.get('model')}`, effort {meta.get('effort') or 'default'}"
        )
    lines += [
        "",
        "Lower is better for every metric. A paired difference is jouet minus plain within",
        "one round; `jouet lower` counts the rounds where jouet used less, and `p` is an",
        "exact two-sided sign test over those rounds. Medians with a few runs are noisy:",
        "read the spread and the pair counts before the medians.",
        "",
        "## Summary",
        "",
        "| task | harness | success plain | success jouet | input tokens Δ | output tokens Δ | requests Δ | tool calls Δ | wall Δ |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for cell in summary["cells"]:
        arms = cell["arms"]
        row = [cell["task"], cell["harness"]]
        for arm in ("plain", "jouet"):
            a = arms.get(arm)
            row.append(f"{a['successes']}/{a['runs']}" if a else "–")
        for key, kind in (
            ("input_tokens", "tokens"),
            ("output_tokens", "tokens"),
            ("requests", "count"),
            ("calls", "count"),
            ("wall_s", "seconds"),
        ):
            p = cell["paired"].get(key)
            if not p:
                row.append("–")
                continue
            pct = f" ({p['median_change'] * 100:+.0f}%)" if p.get("median_change") is not None else ""
            row.append(f"{signed(p['median_diff'], kind)}{pct}")
        lines.append("| " + " | ".join(row) + " |")

    for cell in summary["cells"]:
        lines += ["", f"## {cell['task']} · {cell['harness']}", ""]
        if outcomes := cell.get("paired_outcomes"):
            lines += [
                f"Paired outcomes: {outcomes['both_passed']} both passed; {outcomes['plain_only']} plain only; "
                f"{outcomes['jouet_only']} jouet only; {outcomes['both_failed']} both failed. "
                "Unpaired runs and harness errors are excluded.",
                "",
            ]
        lines += [
            "| metric | plain median [p25–p75] | jouet median [p25–p75] | paired Δ median [min, max] | jouet lower | p |",
            "|---|---|---|---|---|---|",
        ]
        for key, label, _, kind in METRICS:
            row = [label]
            for arm in ("plain", "jouet"):
                m = (cell["arms"].get(arm) or {}).get("metrics", {}).get(key)
                row.append(f"{fmt(m['median'], kind)} [{fmt(m['p25'], kind)}–{fmt(m['p75'], kind)}]" if m else "–")
            p = cell["paired"].get(key)
            if p:
                row.append(
                    f"{signed(p['median_diff'], kind)} [{signed(p['min_diff'], kind)}, {signed(p['max_diff'], kind)}]"
                )
                row.append(f"{p['jouet_lower']}/{p['pairs']}")
                row.append(f"{p['p_sign']:.2f}" if p.get("p_sign") is not None else "–")
            else:
                row += ["–", "–", "–"]
            lines.append("| " + " | ".join(row) + " |")
        if cell.get("paired_outcomes"):
            success_metrics = cell["paired_when_both_pass"]
            parts = []
            for key, label, kind in (
                ("input_tokens", "input", "tokens"),
                ("uncached_input_tokens", "uncached input", "tokens"),
                ("output_tokens", "output", "tokens"),
                ("wall_s", "wall", "seconds"),
            ):
                metric = success_metrics.get(key)
                if metric:
                    parts.append(f"{label} {signed(metric['median_diff'], kind)}")
            count = cell["paired_outcomes"]["both_passed"]
            detail = "; ".join(parts) if parts else "no comparable metrics"
            lines += ["", f"Both-pass pairs ({count}): median jouet minus plain: {detail}."]
        notes = []
        for arm, a in sorted(cell["arms"].items()):
            if a["rule_decisions"]:
                notes.append(
                    f"- **{arm} rule decisions:** " + ", ".join(f"`{k}` ×{v}" for k, v in a["rule_decisions"].items())
                )
            if a["stopped"]:
                notes.append(f"- **{arm} stopped early:** " + ", ".join(a["stopped"]))
            if a["errors"]:
                notes.append(f"- **{arm} harness errors:** " + "; ".join(a["errors"]))
            if a["github_unanswered"]:
                notes.append(
                    f"- **{arm} requests the fake GitHub could not answer:** "
                    + ", ".join(f"`{x}`" for x in a["github_unanswered"])
                )
        if notes:
            lines += [""] + notes
    return "\n".join(lines) + "\n"
