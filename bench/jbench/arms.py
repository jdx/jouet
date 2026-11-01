"""Arms and the isolated environment each run gets.

Every run starts from nothing: its own `HOME`, harness config directory
(`CLAUDE_CONFIG_DIR` or `CODEX_HOME`), XDG directories, `TMPDIR` and a
`bin/` directory that is the front of a short `PATH`. Nothing reads or
writes the user's own `~/.claude`, `~/.codex` or jouet state; the one
exception is the harness's credentials file, which a run gets a copy of
unless an API key or token is passed in the environment instead. A token
the run refreshes is written back afterwards (see `credentials`), so the
user stays logged in.

The arms differ only in what `bin/` holds and what the config directory
says:

- `plain`: the harness as installed, with no jouet on `PATH`;
- `jouet`: `jouet` on `PATH` and `jouet setup <harness>` run in the
  isolated home, which installs the hooks and the session-start primer that
  points the agent at `jouet agent-help`.

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
    "jouet": Arm("jouet", True, "jouet on PATH, with `jouet setup` hooks and primer"),
}


def resolve(tool: str) -> Path | None:
    """The real binary behind `tool`, looking through mise shims."""
    for directory in os.get_exec_path():
        found = shutil.which(tool, path=directory or os.curdir)
        if not found:
            continue
        real = Path(found).resolve()
        if real.name != "mise":
            return real
        proc = subprocess.run([str(real), "which", tool], capture_output=True, text=True)
        if proc.returncode == 0 and proc.stdout.strip():
            return Path(proc.stdout.strip()).resolve()
        # An inactive mise shim can fall through to a later PATH entry.
    return None


def wrapper(path: Path, target: Path, env: dict[str, str]) -> None:
    exports = "".join(f"export {k}={shlex.quote(v)}\n" for k, v in env.items())
    path.write_text(f'#!/bin/sh\n{exports}exec {shlex.quote(str(target))} "$@"\n')
    path.chmod(0o755)


def login_shell_path(env: dict[str, str]) -> None:
    """Restore the run's PATH after system-wide login shell profiles."""
    home = Path(env["HOME"])
    profile = f"export PATH={shlex.quote(env['PATH'])}\n"
    for name in (".zprofile", ".bash_profile"):
        (home / name).write_text(profile)


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
    tmp: Path | None = None,
) -> dict[str, str]:
    """Creates the run's home and returns the environment to run it in."""
    env = base_env(run_dir, fake_url, repo, tmp)
    bin_dir = run_dir / "bin"
    config_dir = Path(env["HOME"]) / harness.default_config_dir
    config_dir.mkdir(parents=True, exist_ok=True)
    (bin_dir / harness.binary).symlink_to(harness_bin)
    if arm.jouet:
        wrapper(bin_dir / "jouet", jouet_bin.resolve(), jouet_env(fake_url) if fake_url else {})
    env[harness.config_dir_env] = str(config_dir)
    env["DISABLE_AUTOUPDATER"] = "1"
    for key in harness.auth_env:
        if os.environ.get(key):
            env[key] = os.environ[key]
    return env


def base_env(run_dir: Path, fake_url: str | None, repo: str, tmp: Path | None = None) -> dict[str, str]:
    """A home, `bin/`, TMPDIR and environment of the run's own, with no
    harness: what the agent runs in, less its harness, and what a check
    runs in when there was no agent. TMPDIR is `tmp`, else `<run>/t`."""
    home = run_dir / "home"
    bin_dir = run_dir / "bin"
    # Short, because a Unix socket's path has to fit in about 100 bytes and
    # a project's tests make theirs several directories below TMPDIR.
    tmp = tmp or run_dir / "t"
    for d in (home, bin_dir, tmp):
        d.mkdir(parents=True, exist_ok=True)
    (home / ".gitconfig").write_text(
        "[user]\n\tname = Bench Agent\n\temail = agent@example.invalid\n[init]\n\tdefaultBranch = main\n"
    )

    for tool in TOOLS:
        real = resolve(tool)
        if real:
            (bin_dir / tool).symlink_to(real)
    # An agent with root can write anywhere; this one cannot.
    (bin_dir / "sudo").write_text("#!/bin/sh\necho 'sudo is not available in the benchmark' >&2\nexit 1\n")
    (bin_dir / "sudo").chmod(0o755)

    gh = resolve("gh")
    if gh and fake_url:
        wrapper(bin_dir / "gh", gh, gh_env(fake_url, repo))
    elif gh:
        (bin_dir / "gh").symlink_to(gh)

    return {
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
        "GIT_TERMINAL_PROMPT": "0",
        # mise reads every mise.toml, and `.config/mise/config.toml`, in the
        # directories above the one it runs in: above the run, those are the
        # user's own. The ceiling itself is not searched.
        "MISE_CEILING_PATHS": str(run_dir),
    }


def credential_files(harness: Harness) -> list[Path]:
    """The user's credentials files a run needs a copy of: none when an API
    key or token in the environment authenticates it."""
    if any(os.environ.get(key) for key in harness.auth_env):
        return []
    return [user_config_dir(harness) / name for name in harness.auth_files]


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
