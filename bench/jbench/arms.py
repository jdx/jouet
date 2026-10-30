"""Arms and the isolated environment each run gets.

Every run starts from nothing: its own `HOME`, harness config directory
(`CLAUDE_CONFIG_DIR` or `CODEX_HOME`), XDG directories, `TMPDIR` and a
`bin/` directory that is the front of a short `PATH`. Nothing reads or
writes the user's own `~/.claude`, `~/.codex` or jouet state; the one
exception is the harness's credentials file, which is linked (not copied,
so a token refresh lands where the harness will look next time) unless an
API key is passed in the environment instead.

The arms differ only in what `bin/` holds and what the config directory
says:

- `plain`: the harness as installed, with no jouet on `PATH`;
- `jouet`: `jouet` on `PATH` and `jouet setup <harness>` run in the
  isolated home, which installs the hooks, the skill and the session-start
  primer that points the agent at `jouet agent-help`.

`gh` and `jouet` are wrapper scripts when a task has a fake GitHub, so the
agent's own environment carries no hint of it.
"""

from __future__ import annotations

import json
import os
import shlex
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .harness import Harness

# Tools linked into every run's bin/ when the machine has them.
TOOLS = ("git", "python3", "node", "curl", "jq", "rg", "diff", "patch")


@dataclass(frozen=True)
class Arm:
    name: str
    jouet: bool
    description: str


ARMS = {
    "plain": Arm("plain", False, "the harness as installed, without jouet"),
    "jouet": Arm("jouet", True, "jouet on PATH, with `jouet setup` hooks, skill and primer"),
}


def resolve(tool: str) -> Path | None:
    """The real binary behind `tool`, looking through mise shims."""
    found = shutil.which(tool)
    if not found:
        return None
    real = Path(found).resolve()
    if real.name == "mise" and shutil.which("mise"):
        proc = subprocess.run(["mise", "which", tool], capture_output=True, text=True)
        if proc.returncode != 0 or not proc.stdout.strip():
            return None
        return Path(proc.stdout.strip()).resolve()
    return real


def wrapper(path: Path, target: Path, env: dict[str, str]) -> None:
    exports = "".join(f"export {k}={shlex.quote(v)}\n" for k, v in env.items())
    path.write_text(f'#!/bin/sh\n{exports}exec {shlex.quote(str(target))} "$@"\n')
    path.chmod(0o755)


def gh_env(fake_url: str, repo: str) -> dict[str, str]:
    """Points `gh` at the fake: `github.localhost` is the one host `gh`
    talks to over plain HTTP, and the fake is its proxy."""
    return {
        "GH_HOST": "github.localhost",
        "GH_REPO": f"github.localhost/{repo}",
        "GH_TOKEN": "bench-token",
        "GH_ENTERPRISE_TOKEN": "bench-token",
        "HTTP_PROXY": fake_url,
        "http_proxy": fake_url,
        "GH_NO_UPDATE_NOTIFIER": "1",
        "GH_PROMPT_DISABLED": "1",
    }


def jouet_env(fake_url: str) -> dict[str, str]:
    # Not HTTP_PROXY: jouet's client would send its requests through it.
    return {"JOUET_GITHUB_API_URL": fake_url, "JOUET_GITHUB_TOKEN": "bench-token"}


def user_config_dir(harness: Harness) -> Path:
    configured = os.environ.get(harness.config_dir_env)
    return Path(configured) if configured else Path.home() / harness.default_config_dir


def auth_status(harness: Harness) -> str | None:
    """How a run will authenticate, or None when it cannot."""
    for key in harness.auth_env:
        if os.environ.get(key):
            return f"${key}"
    for name in harness.auth_files:
        if (user_config_dir(harness) / name).exists():
            return str(user_config_dir(harness) / name)
    return None


def build(
    run_dir: Path,
    harness: Harness,
    arm: Arm,
    harness_bin: Path,
    jouet_bin: Path,
    fake_url: str | None,
    repo: str,
) -> dict[str, str]:
    """Creates the run's home and returns the environment to run it in."""
    home = run_dir / "home"
    bin_dir = run_dir / "bin"
    tmp = run_dir / "tmp"
    config_dir = home / harness.default_config_dir
    for d in (home, bin_dir, tmp, config_dir):
        d.mkdir(parents=True, exist_ok=True)
    (home / ".gitconfig").write_text(
        "[user]\n\tname = Bench Agent\n\temail = agent@example.invalid\n[init]\n\tdefaultBranch = main\n"
    )

    for tool in TOOLS:
        real = resolve(tool)
        if real:
            (bin_dir / tool).symlink_to(real)
    (bin_dir / harness.binary).symlink_to(harness_bin)

    gh = resolve("gh")
    if gh and fake_url:
        wrapper(bin_dir / "gh", gh, gh_env(fake_url, repo))
    elif gh:
        (bin_dir / "gh").symlink_to(gh)

    if arm.jouet:
        wrapper(bin_dir / "jouet", jouet_bin.resolve(), jouet_env(fake_url) if fake_url else {})

    env = {
        "PATH": f"{bin_dir}:/usr/local/bin:/usr/bin:/bin",
        "HOME": str(home),
        "USER": os.environ.get("USER", "bench"),
        "LANG": "C.UTF-8",
        "TERM": "dumb",
        "TMPDIR": str(tmp),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "XDG_STATE_HOME": str(home / ".local" / "state"),
        harness.config_dir_env: str(config_dir),
        "GIT_TERMINAL_PROMPT": "0",
        "DISABLE_AUTOUPDATER": "1",
    }
    linked = False
    for key in harness.auth_env:
        if os.environ.get(key):
            env[key] = os.environ[key]
            linked = True
    if not linked:
        for name in harness.auth_files:
            source = user_config_dir(harness) / name
            if source.exists():
                (config_dir / name).symlink_to(source)
    return env


def setup_jouet(harness: Harness, env: dict[str, str], workdir: Path) -> dict[str, Any]:
    """Runs `jouet setup` in the isolated home, as a user would."""
    proc = subprocess.run(
        ["jouet", "setup", harness.name, "--json"],
        cwd=workdir,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"jouet setup {harness.name} failed: {proc.stderr.strip() or proc.stdout.strip()}")
    report = json.loads(proc.stdout)
    return {"files": [{"path": f["path"], "action": f["action"]} for f in report.get("files", [])]}
