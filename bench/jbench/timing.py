"""Observed command intervals in the agent's streamed events.

These are receipt times, not process CPU time or model inference time. Codex
can buffer events and commands can overlap; incomplete events stay explicit.
"""

import json


def codex_commands(lines: list[str], elapsed: list[float], complete: bool) -> dict | None:
    if len(lines) != len(elapsed):
        return None
    starts: dict[str, float] = {}
    intervals: list[tuple[float, float]] = []
    unmatched_completions = 0
    duplicate_starts = 0
    for line, at in zip(lines, elapsed):
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            continue
        if not isinstance(event, dict) or not isinstance(event.get("item"), dict):
            continue
        item = event["item"]
        if item.get("type") != "commandExecution" or not isinstance(item.get("id"), str):
            continue
        item_id = item["id"]
        if event.get("type") == "item.started":
            if item_id in starts:
                duplicate_starts += 1
            else:
                starts[item_id] = at
        elif event.get("type") == "item.completed":
            start = starts.pop(item_id, None)
            if start is None or start > at:
                unmatched_completions += 1
            else:
                intervals.append((start, at))
    merged: list[tuple[float, float]] = []
    for start, end in sorted(intervals):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return {
        "observed_command_active_s": round(sum(end - start for start, end in merged), 3),
        "completed_intervals": len(intervals),
        # Events received within a millisecond may be buffered together;
        # they are not evidence that the command itself took that long.
        "coalesced_intervals": sum(end - start <= 0.001 for start, end in intervals),
        "unmatched_starts": len(starts),
        "unmatched_completions": unmatched_completions,
        "duplicate_starts": duplicate_starts,
        "stream_complete": complete,
    }
