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
        result = next((e for e in reversed(events) if e.get("type") == "result"), None)
        init = next((e for e in events if e.get("type") == "system" and e.get("subtype") == "init"), {})
        if result and result.get("usage"):
            u = result["usage"]
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
            turns=(result or {}).get("num_turns"),
            is_error=bool((result or {}).get("is_error")) or result is None,
            result=(result or {}).get("result"),
            session_id=(result or init).get("session_id"),
            model=init.get("model"),
            extra={
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
            binary,
            "exec",
            "--json",
            "--model",
            opts["codex_model"],
            "--dangerously-bypass-approvals-and-sandbox",
            # jouet's hooks are new to this throwaway CODEX_HOME, and Codex
            # skips hooks nobody has trusted. The plain arm has no hooks.
            "--dangerously-bypass-hook-trust",
            "--skip-git-repo-check",
            "--cd",
            str(workdir),
        ]
        if opts.get("effort"):
            cmd += ["-c", f'model_reasoning_effort="{opts["effort"]}"']
        return cmd + [prompt]

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
        u, _ = self._session_usage(config_dir)
        return int(u.get("input_tokens") or 0) + int(u.get("output_tokens") or 0)

    def usage(self, stream_lines: list[str], config_dir: Path) -> Usage:
        events = _json_lines(stream_lines)
        done = [e for e in events if e.get("type") == "turn.completed"]
        failed = any(e.get("type") in ("turn.failed", "error") for e in events)
        u = done[-1]["usage"] if done else self._session_usage(config_dir)[0]
        _, model = self._session_usage(config_dir)
        messages = [
            e["item"].get("text")
            for e in events
            if e.get("type") == "item.completed" and (e.get("item") or {}).get("type") == "agent_message"
        ]
        thread = next((e.get("thread_id") for e in events if e.get("type") == "thread.started"), None)
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
            extra={"reasoning_tokens": u.get("reasoning_output_tokens")},
        )


HARNESSES: dict[str, Harness] = {h.name: h for h in (Claude(), Codex())}
