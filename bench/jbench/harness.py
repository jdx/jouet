"""How each harness is started headlessly, and what its output says.

Claude Code runs as `claude -p --output-format stream-json`, whose events
carry each model response's usage and end with a result holding the totals,
turns and cost. Codex runs as `codex exec --json`, whose usage arrives only
when the turn completes, so a run's token budget is enforced from the
session file Codex writes as it goes (its cumulative `token_count` events).

Neither CLI exposes a temperature or a seed, so runs are not reproducible
token for token; the suite repeats runs and pairs them instead.
"""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class Usage:
    """Token totals for one run, the same fields for every harness.

    `input_tokens` is all context the model read, cached or not, which is
    what `jouet audit` counts; `cached_input_tokens` is the part served from
    the prompt cache, and `cache_write_tokens` the part written to it.
    """

    input_tokens: int = 0
    cached_input_tokens: int = 0
    cache_write_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float | None = None
    turns: int | None = None
    is_error: bool = False
    result: str | None = None
    session_id: str | None = None
    model: str | None = None
    extra: dict[str, Any] = field(default_factory=dict)

    @property
    def total_tokens(self) -> int:
        return self.input_tokens + self.output_tokens


class Harness:
    name = ""
    binary = ""
    # The environment variables that may carry credentials into a run.
    auth_env: tuple[str, ...] = ()
    # Credentials files linked into a run's isolated config directory.
    auth_files: tuple[str, ...] = ()
    config_dir_env = ""
    default_config_dir = ""

    def command(self, binary: str, prompt: str, opts: dict[str, Any], workdir: Path) -> list[str]:
        raise NotImplementedError

    def tokens_so_far(self, stream_lines: list[str], config_dir: Path) -> int:
        raise NotImplementedError

    def usage(self, stream_lines: list[str], config_dir: Path) -> Usage:
        raise NotImplementedError


def _json_lines(lines: list[str]) -> list[dict[str, Any]]:
    out = []
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            out.append(json.loads(line))
        except ValueError:
            continue
    return out


def codex_shell_metrics(lines: list[str]) -> dict[str, int | None] | None:
    """Count complete, linked command events in a complete ephemeral turn."""
    try:
        events = [json.loads(line) for line in lines if line.strip()]
    except (TypeError, ValueError):
        return None
    if (len(events) < 3 or not all(isinstance(event, dict) for event in events)
            or events[0].get("type") != "jouet.ephemeral"
            or events[1].get("type") != "thread.started"
            or events[-1].get("type") != "turn.completed"):
        return None
    started: set[str] = set()
    completed: dict[str, int | None] = {}
    for event in events[2:-1]:
        if event.get("type") in ("error", "turn.failed", "turn.completed", "thread.started"):
            return None
        if event.get("type") not in ("item.started", "item.completed"):
            continue
        item = event.get("item")
        if not isinstance(item, dict) or item.get("type") != "commandExecution":
            continue
        ident = item.get("id")
        if not isinstance(ident, str) or not ident:
            return None
        if event["type"] == "item.started":
            if ident in started:
                return None
            started.add(ident)
        else:
            if ident not in started or ident in completed:
                return None
            code = item.get("exitCode")
            completed[ident] = code if type(code) is int else None
    if started != completed.keys():
        return None
    failures = (sum(code != 0 for code in completed.values())
                if all(code is not None for code in completed.values()) else None)
    return {"completed": len(completed), "failed": failures}


class Claude(Harness):
    name = "claude"
    binary = "claude"
    auth_env = ("ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN")
    auth_files = (".credentials.json",)
    config_dir_env = "CLAUDE_CONFIG_DIR"
    default_config_dir = ".claude"

    def command(self, binary: str, prompt: str, opts: dict[str, Any], workdir: Path) -> list[str]:
        cmd = [
            binary,
            "-p",
            prompt,
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            opts["claude_model"],
            "--permission-mode",
            "bypassPermissions",
            "--strict-mcp-config",
            # A claude.ai login otherwise downloads the account's skills in
            # the background, and their listing (about 3.8k tokens) joins
            # every request after it lands: in a long run, not in a short
            # one, which made the arm with more requests look costlier.
            "--settings",
            '{"syncClaudeAiSkills": false}',
        ]
        if opts.get("effort"):
            cmd += ["--effort", opts["effort"]]
        if opts.get("max_usd_per_run"):
            cmd += ["--max-budget-usd", str(opts["max_usd_per_run"])]
        return cmd

    def _messages(self, lines: list[str]) -> dict[str, dict[str, Any]]:
        """Each model response's usage, once per message ID."""
        seen: dict[str, dict[str, Any]] = {}
        for event in _json_lines(lines):
            if event.get("type") != "assistant":
                continue
            message = event.get("message") or {}
            if message.get("usage") and message.get("id"):
                seen[message["id"]] = message["usage"]
        return seen

    def tokens_so_far(self, stream_lines: list[str], config_dir: Path) -> int:
        total = 0
        for u in self._messages(stream_lines).values():
            total += sum(
                int(u.get(k) or 0)
                for k in (
                    "input_tokens",
                    "cache_read_input_tokens",
                    "cache_creation_input_tokens",
                    "output_tokens",
                )
            )
        return total

    def usage(self, stream_lines: list[str], config_dir: Path) -> Usage:
        events = _json_lines(stream_lines)
        results = [e for e in events if e.get("type") == "result"]
        result = results[-1] if results else None
        # A later assistant message can belong to a resumed turn whose final
        # result was never captured. Its usage is only a partial observation.
        last_result = max((i for i, event in enumerate(events) if event.get("type") == "result"), default=-1)
        complete = (bool(results) and all(
            isinstance(r.get("usage"), dict)
            and all(type(r["usage"].get(key)) is int and r["usage"][key] >= 0
                    for key in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"))
            for r in results
        ) and not any(event.get("type") == "assistant" for event in events[last_result + 1:]))
        init = next((e for e in events if e.get("type") == "system" and e.get("subtype") == "init"), {})
        turns = sum(int(r.get("num_turns") or 0) for r in results) if results else None
        if result and result.get("usage"):
            # A session that ends its turn while a background command runs
            # is resumed when it finishes, and each turn ends in a result of
            # its own. A result's usage and turns cover its turn alone; its
            # cost is the session's so far.
            u = {}
            for r in results:
                for k, v in (r.get("usage") or {}).items():
                    if isinstance(v, int):
                        u[k] = u.get(k, 0) + v
        else:
            # Killed before the result: add up the responses seen so far.
            u = {}
            for m in self._messages(stream_lines).values():
                for k, v in m.items():
                    if isinstance(v, int):
                        u[k] = u.get(k, 0) + v
        read = int(u.get("cache_read_input_tokens") or 0)
        write = int(u.get("cache_creation_input_tokens") or 0)
        return Usage(
            input_tokens=int(u.get("input_tokens") or 0) + read + write,
            cached_input_tokens=read,
            cache_write_tokens=write,
            output_tokens=int(u.get("output_tokens") or 0),
            cost_usd=(result or {}).get("total_cost_usd"),
            turns=turns,
            is_error=bool((result or {}).get("is_error")) or result is None,
            result=(result or {}).get("result"),
            session_id=(result or init).get("session_id"),
            model=init.get("model"),
            extra={
                "usage_complete": complete,
                "subtype": (result or {}).get("subtype"),
                "api_ms": (result or {}).get("duration_api_ms"),
                "permission_denials": len((result or {}).get("permission_denials") or []),
            },
        )


class Codex(Harness):
    name = "codex"
    binary = "codex"
    auth_env = ("OPENAI_API_KEY", "CODEX_API_KEY")
    auth_files = ("auth.json",)
    config_dir_env = "CODEX_HOME"
    default_config_dir = ".codex"

    def command(self, binary: str, prompt: str, opts: dict[str, Any], workdir: Path) -> list[str]:
        cmd = [
            sys.executable, str(Path(__file__).with_name("codex_ephemeral.py")),
            "--codex", binary, "--cwd", str(workdir), "--model", opts["codex_model"],
        ]
        if opts.get("effort"):
            cmd += ["--effort", opts["effort"]]
        return cmd + ["--", prompt]

    def _session_usage(self, config_dir: Path) -> tuple[dict[str, Any], str | None]:
        latest: dict[str, Any] = {}
        model = None
        for path in sorted((config_dir / "sessions").rglob("*.jsonl")):
            for event in _json_lines(path.read_text(errors="replace").splitlines()):
                payload = event.get("payload") or {}
                if payload.get("type") == "token_count" and payload.get("info"):
                    latest = payload["info"].get("total_token_usage") or latest
                if event.get("type") == "turn_context":
                    model = payload.get("model") or model
        return latest, model

    def tokens_so_far(self, stream_lines: list[str], config_dir: Path) -> int:
        live = [e.get("usage") or {} for e in _json_lines(stream_lines) if e.get("type") == "jouet.token_usage"]
        if live:
            return int(live[-1].get("inputTokens") or 0) + int(live[-1].get("outputTokens") or 0)
        if any(e.get("type") == "jouet.ephemeral" for e in _json_lines(stream_lines)):
            return 0
        u, _ = self._session_usage(config_dir)
        return int(u.get("input_tokens") or 0) + int(u.get("output_tokens") or 0)

    def usage(self, stream_lines: list[str], config_dir: Path) -> Usage:
        events = _json_lines(stream_lines)
        done = [e for e in events if e.get("type") == "turn.completed"]
        failed = any(e.get("type") in ("turn.failed", "error") for e in events)
        ephemeral = any(e.get("type") == "jouet.ephemeral" for e in events)
        live = [e.get("usage") or {} for e in events if e.get("type") == "jouet.token_usage"]
        if done:
            u = done[-1].get("usage") or {}
        elif ephemeral:
            last = live[-1] if live else {}
            u = {"input_tokens": last.get("inputTokens"), "cached_input_tokens": last.get("cachedInputTokens"),
                 "cache_write_input_tokens": last.get("cacheWriteInputTokens"), "output_tokens": last.get("outputTokens"),
                 "reasoning_output_tokens": last.get("reasoningOutputTokens")}
        else:
            u = self._session_usage(config_dir)[0]
        model = next((e.get("model") for e in events if e.get("type") == "thread.started"), None) if ephemeral else self._session_usage(config_dir)[1]
        messages = [
            e["item"].get("text")
            for e in events
            if e.get("type") == "item.completed" and (e.get("item") or {}).get("type") == "agent_message"
        ]
        thread = next((e.get("thread_id") for e in events if e.get("type") == "thread.started"), None)
        terminal = next((e for e in reversed(events) if e.get("type") in ("turn.failed", "turn.completed")), {})
        capacity = (terminal.get("type") == "turn.failed"
                    and isinstance(terminal.get("error"), dict)
                    and "selected model is at capacity" in str(terminal["error"].get("message") or "").lower())
        return Usage(
            # OpenAI counts cached input inside input_tokens.
            input_tokens=int(u.get("input_tokens") or 0),
            cached_input_tokens=int(u.get("cached_input_tokens") or 0),
            cache_write_tokens=int(u.get("cache_write_input_tokens") or 0),
            output_tokens=int(u.get("output_tokens") or 0),
            turns=None,
            is_error=failed or not done,
            result=messages[-1] if messages else None,
            session_id=thread,
            model=model,
            extra={"reasoning_tokens": u.get("reasoning_output_tokens"), "capacity_error": capacity,
                   "usage_complete": bool(done and not failed and all(type(u.get(key)) is int and u[key] >= 0
                                                       for key in ("input_tokens", "cached_input_tokens", "output_tokens"))),
                   **({"usage_observed": bool(done or live)} if ephemeral else {})},
        )


HARNESSES: dict[str, Harness] = {h.name: h for h in (Claude(), Codex())}
