"""Turns run records into `summary.json` and `summary.md`."""

from __future__ import annotations

import json
from collections import defaultdict
from collections.abc import Callable
from typing import Any

from . import stats

# (key, label, getter, format) for each compared metric. Lower is better
# for all of them.
METRICS: list[tuple[str, str, Callable[[dict[str, Any]], float | None], str]] = [
    ("input_tokens", "input tokens (all context read)", lambda r: _u(r, "input_tokens"), "tokens"),
    ("uncached_input_tokens", "input tokens not from cache", lambda r: _uncached(r), "tokens"),
    ("weighted_input_tokens", "input tokens, cache reads at 1/10", lambda r: _weighted(r), "tokens"),
    ("output_tokens", "output tokens", lambda r: _u(r, "output_tokens"), "tokens"),
    ("cost_usd", "cost (USD, as the harness reports it)", lambda r: _u(r, "cost_usd"), "usd"),
    ("requests", "model requests", lambda r: _a(r, "requests"), "count"),
    ("calls", "tool calls", lambda r: _a(r, "calls"), "count"),
    ("failed_calls", "failed tool calls", lambda r: _a(r, "failed_calls"), "count"),
    ("shell_commands", "completed shell commands", lambda r: _shell(r, "completed"), "count"),
    ("shell_failures", "failed shell commands", lambda r: _shell(r, "failed"), "count"),
    ("tool_output_bytes", "tool output (bytes)", lambda r: _a(r, "tool_output_bytes"), "bytes"),
    ("wall_s", "wall time (s)", lambda r: r.get("wall_s"), "seconds"),
]


def _u(r: dict[str, Any], key: str) -> float | None:
    usage = r.get("usage") or {}
    if key in {"input_tokens", "cached_input_tokens", "output_tokens", "cost_usd"} and (
        (usage.get("extra") or {}).get("usage_complete") is False or r.get("stopped")
        or (r.get("exit_code") is not None and r["exit_code"] != 0)
    ):
        return None
    return usage.get(key)


def _a(r: dict[str, Any], key: str) -> float | None:
    a = r.get("audit")
    return a.get(key) if a else None


def _shell(r: dict[str, Any], key: str) -> float | None:
    return (r.get("shell") or {}).get(key)


def _uncached(r: dict[str, Any]) -> float | None:
    total = _u(r, "input_tokens")
    cached = _u(r, "cached_input_tokens")
    if total is None or cached is None:
        return None
    return total - cached


# What a cached input token costs next to an uncached one: a tenth, for both
# Anthropic's cache reads and OpenAI's cached input.
CACHE_READ_WEIGHT = 0.1


def _weighted(r: dict[str, Any]) -> float | None:
    """Input tokens as they are priced: uncached ones whole, cache reads at a
    tenth. Raw input tokens count a cache read like any other token."""
    total = _u(r, "input_tokens")
    cached = _u(r, "cached_input_tokens")
    if total is None or cached is None:
        return None
    return total - cached + CACHE_READ_WEIGHT * cached


# Spent per solved task: everything the scored runs spent, failed ones
# included, over the runs that passed. The comparison the goal uses when the
# arms' pass rates differ.
PER_SOLVED = [
    ("input_tokens", lambda r: _u(r, "input_tokens")),
    ("weighted_input_tokens", _weighted),
    ("cost_usd", lambda r: _u(r, "cost_usd")),
]


def per_solved(runs: list[dict[str, Any]]) -> dict[str, float | None]:
    solved = sum(1 for r in runs if r.get("success"))
    out: dict[str, float | None] = {}
    for key, get in PER_SOLVED:
        spent = [get(r) for r in runs]
        # A run that reports nothing for it leaves the sum unknown.
        out[key] = sum(spent) / solved if solved and spent and None not in spent else None
    return out


def group(records: list[dict[str, Any]]) -> dict[tuple[str, str], dict[str, list[dict[str, Any]]]]:
    out: dict[tuple[str, str], dict[str, list[dict[str, Any]]]] = defaultdict(lambda: defaultdict(list))
    for r in records:
        out[(r["task"], r["harness"])][r["arm"]].append(r)
    return out


def unverified(record: dict[str, Any]) -> bool:
    check = record.get("check") or {}
    return record.get("success") is None or ("passed" in check and check["passed"] is None)


def scored(record: dict[str, Any]) -> bool:
    """A failed or stopped agent is a known failure even if its check is incomplete."""
    exit_code = record.get("exit_code")
    agent_failed = record.get("success") is False and (
        bool(record.get("stopped")) or (type(exit_code) is int and exit_code != 0)
    )
    return (not record.get("error") and isinstance(record.get("success"), bool)
            and (not unverified(record) or agent_failed))


def summarize(records: list[dict[str, Any]], meta: dict[str, Any]) -> dict[str, Any]:
    cells = []
    for (task, harness), by_arm in sorted(group(records).items()):
        cell: dict[str, Any] = {"task": task, "harness": harness, "arms": {}, "paired": {}}
        if harness == "codex" and {"jouet", "jouet-init"}.intersection(by_arm):
            cell["limitations"] = ["codex_stop_gate_unverified"]
        for arm, runs in sorted(by_arm.items()):
            finished = [r for r in runs if not r.get("error") and not unverified(r)
                        and isinstance(r.get("success"), bool)]
            outcomes = [r for r in runs if scored(r)]
            decisions: dict[str, int] = defaultdict(int)
            unanswered: set[str] = set()
            for r in finished:
                for k, v in ((r.get("ledger") or {}).get("decisions") or {}).items():
                    decisions[k] += v
                unanswered.update((r.get("github") or {}).get("unanswered_examples") or [])
            cell["arms"][arm] = {
                "runs": len(outcomes),
                "unverified": [{"round": r.get("round"), "reason": (r.get("check") or {}).get("reason", "coverage unavailable")}
                               for r in runs if not r.get("error") and unverified(r)],
                "errors": [r["error"] for r in runs if r.get("error")],
                "stopped": [r["stopped"] for r in outcomes if r.get("stopped")],
                "successes": sum(1 for r in outcomes if r.get("success")),
                "per_solved": per_solved(outcomes),
                "metrics": {key: stats.summarize(get(r) for r in finished) for key, _, get, _ in METRICS},
                "rule_decisions": dict(sorted(decisions.items())),
                "github_unanswered": sorted(unanswered),
                "changes": [
                    {"round": r.get("round"), "paths": r.get("changed_paths")}
                    for r in runs
                ],
            }
        # Prefer the committed project configuration when the pilot includes it.
        treat = next((a for a in ("jouet-init", "jouet") if a in by_arm), None)
        cell["treatment"] = treat
        init_runs = [r["init"] for r in by_arm.get("jouet-init", []) if r.get("init")]
        if init_runs:
            cell["init"] = init_runs[0]
        if "jouet" in by_arm and "jouet-init" in by_arm:
            pairs = {
                arm: {r["round"]: r for r in by_arm[arm] if scored(r)}
                for arm in ("jouet", "jouet-init")
            }
            rounds = sorted(pairs["jouet"].keys() & pairs["jouet-init"].keys())
            passed = [n for n in rounds if pairs["jouet"][n]["success"] and pairs["jouet-init"][n]["success"]]
            cell["init_comparison"] = {
                "pairs": len(rounds),
                "both_passed": len(passed),
                "paired_when_both_pass": {
                    key: stats.paired({n: get(pairs["jouet"][n]) for n in passed},
                                      {n: get(pairs["jouet-init"][n]) for n in passed})
                    for key, _, get, _ in METRICS
                },
            }
        if "plain" in by_arm and treat:
            paired_runs = {
                arm: {r["round"]: r for r in by_arm[arm]
                      if not r.get("error") and not unverified(r) and isinstance(r.get("success"), bool)}
                for arm in ("plain", treat)
            }
            paired_runs["jouet"] = paired_runs[treat]
            rounds = sorted(paired_runs["plain"].keys() & paired_runs["jouet"].keys())
            outcome_pairs = {
                arm: {r["round"]: r for r in by_arm[arm] if scored(r)}
                for arm in ("plain", treat)
            }
            outcome_rounds = sorted(outcome_pairs["plain"].keys() & outcome_pairs[treat].keys())
            outcomes = [(outcome_pairs["plain"][n]["success"], outcome_pairs[treat][n]["success"])
                        for n in outcome_rounds]
            cell["paired_outcomes"] = {
                "both_passed": outcomes.count((True, True)),
                "plain_only": outcomes.count((True, False)),
                "jouet_only": outcomes.count((False, True)),
                "both_failed": outcomes.count((False, False)),
            }
            passed_rounds = [n for n in rounds if paired_runs["plain"][n]["success"] and paired_runs["jouet"][n]["success"]]
            cell["paired_when_both_pass"] = {}
            for key, _, get, _ in METRICS:
                plain = {n: get(r) for n, r in paired_runs["plain"].items()}
                jouet = {n: get(r) for n, r in paired_runs["jouet"].items()}
                cell["paired"][key] = stats.paired(plain, jouet)
                cell["paired_when_both_pass"][key] = stats.paired(
                    {n: get(paired_runs["plain"][n]) for n in passed_rounds},
                    {n: get(paired_runs["jouet"][n]) for n in passed_rounds},
                )
        cells.append(cell)
    return {"schema": 1, "meta": meta, "cells": cells}


def per_solved_line(cell: dict[str, Any]) -> str | None:
    """Each arm's spend per solved task, with the treatment's change from
    plain: what matters when the arms' pass rates differ."""
    arms = cell["arms"]
    names = [a for a in ("plain", "jouet", "jouet-init") if a in arms]
    parts = []
    for key, label, kind in (("input_tokens", "input", "tokens"),
                             ("weighted_input_tokens", "cache-weighted input", "tokens"),
                             ("cost_usd", "cost", "usd")):
        values = {a: (arms[a].get("per_solved") or {}).get(key) for a in names}
        if all(v is None for v in values.values()):
            continue
        each = []
        for a in names:
            v = values[a]
            text = "–" if v is None else fmt(v, kind)
            base = values.get("plain")
            if a != "plain" and v is not None and base:
                text += f" ({(v / base - 1) * 100:+.0f}%)"
            each.append(f"{a} {text}")
        parts.append(f"{label}: " + ", ".join(each))
    if not parts:
        return None
    return ("Per solved task (all scored runs' spend over the runs that passed): "
            + "; ".join(parts) + ".")


def init_line(cell: dict[str, Any], init: dict[str, Any]) -> str:
    """What the jouet-init skill changed. Its run is not a cost of the
    task: a project commits the configuration once and keeps it, as it
    keeps any other tooling config."""
    del cell
    files = init.get("files") or []
    if not files:
        return "jouet-init changed no files for this task, so the `jouet-init` arm ran it as the `jouet` arm does."
    named = ", ".join(files[:8]) + (f" and {len(files) - 8} more" if len(files) > 8 else "")
    return f"jouet-init configured the project with {len(files)} file(s), committed before each run: {named}."


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
        f"- **Benchmark checkout:** `{meta.get('commit') or 'unavailable'}`",
        (f"- **jouet:** {meta.get('jouet_version') or 'version unavailable'}; "
        f"binary SHA-256 `{meta.get('jouet_sha256') or 'unavailable'}`"),
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
        "Success means the agent exited normally and the task checker passed. Checkers vary:",
        "some verify behavior without requiring every requested deliverable, such as a commit.",
        "Inspect the task's checker and run evidence before treating success as complete accuracy.",
        ("Unverified checks exclude runs from performance metrics; agents that fail or stop still count as failures. "
        "Deltas compare only pairs where both arms passed the task checker."),
        "",
        "| task | harness | success plain | success jouet | both passed | input tokens Δ | output tokens Δ | requests Δ | tool calls Δ | wall Δ |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for cell in summary["cells"]:
        arms = cell["arms"]
        row = [cell["task"] + (" (jouet-init)" if cell.get("treatment") == "jouet-init" else ""), cell["harness"]]
        for arm in ("plain", cell.get("treatment") or "jouet"):
            a = arms.get(arm)
            row.append(f"{a['successes']}/{a['runs']}" if a else "–")
        row.append(str(cell["paired_outcomes"]["both_passed"]) if "paired_outcomes" in cell else "–")
        for key, kind in (
            ("input_tokens", "tokens"),
            ("output_tokens", "tokens"),
            ("requests", "count"),
            ("calls", "count"),
            ("wall_s", "seconds"),
        ):
            p = cell.get("paired_when_both_pass", {}).get(key)
            if not p:
                row.append("–")
                continue
            pct = f" ({p['median_change'] * 100:+.0f}%)" if p.get("median_change") is not None else ""
            row.append(f"{signed(p['median_diff'], kind)}{pct}")
        lines.append("| " + " | ".join(row) + " |")

    for cell in summary["cells"]:
        lines += ["", f"## {cell['task']} · {cell['harness']}", ""]
        if "codex_stop_gate_unverified" in cell.get("limitations", []):
            lines += [
                ("Codex stop-gate coverage is not independently verified by this report. "
                 "An installed, trusted hook or a recorded stop event does not establish that the gate "
                 "could read the session's checks and block on a failure."),
                "",
            ]
        if outcomes := cell.get("paired_outcomes"):
            lines += [
                (f"Paired outcomes: {outcomes['both_passed']} both passed; {outcomes['plain_only']} plain only; "
                f"{outcomes['jouet_only']} jouet only; {outcomes['both_failed']} both failed. "
                "Unpaired runs and harness errors are excluded."),
                "",
            ]
        lines += [
            "The following diagnostic metrics include finished runs even when a task failed or stopped early.",
            "",
            "| metric | plain median [p25–p75] | jouet median [p25–p75] | paired Δ median [min, max] | jouet lower | p |",
            "|---|---|---|---|---|---|",
        ]
        for key, label, _, kind in METRICS:
            row = [label]
            for arm in ("plain", cell.get("treatment") or "jouet"):
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
        if line := per_solved_line(cell):
            lines += ["", line]
        if init := cell.get("init"):
            lines += ["", init_line(cell, init)]
        if comparison := cell.get("init_comparison"):
            lines += ["", "### Initialized versus ordinary jouet", "",
                      (f"Paired rounds: {comparison['pairs']}; passed in both arms: {comparison['both_passed']}. "
                      "Medians include finished failures; deltas are initialized minus ordinary and include only both-pass pairs."), "",
                      "| metric | ordinary jouet median | initialized jouet median | both-pass paired Δ |",
                      "|---|---|---|---|"]
            for key, label, _, kind in METRICS:
                ordinary = cell["arms"]["jouet"]["metrics"].get(key)
                initialized = cell["arms"]["jouet-init"]["metrics"].get(key)
                delta = comparison["paired_when_both_pass"].get(key)
                lines.append(f"| {label} | {fmt(ordinary['median'] if ordinary else None, kind)} | "
                             f"{fmt(initialized['median'] if initialized else None, kind)} | "
                             f"{signed(delta['median_diff'] if delta else None, kind)} |")
        notes = []
        for arm, a in sorted(cell["arms"].items()):
            for item in a.get("unverified", []):
                notes.append(f"- **{arm} check unverified (round {item['round']}):** {item['reason']}; see the run's check.log.")
            for change in a.get("changes", []):
                paths = change["paths"]
                if paths is None:
                    shown = "unavailable"
                elif not paths:
                    shown = "none observed"
                else:
                    value = json.dumps(paths[:10], ensure_ascii=True)
                    ticks = "`"
                    while ticks in value:
                        ticks += "`"
                    shown = f"{ticks}{value}{ticks}"
                    if len(paths) > 10:
                        shown += f"; {len(paths) - 10} more (full list: runs.jsonl)"
                notes.append(f"- **{arm} changed paths (round {change['round']}):** {shown}")
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
