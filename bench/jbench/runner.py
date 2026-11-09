"""Planning and running the suite, and collecting each run's metrics.

A suite is rounds × tasks × harnesses × arms. Within a round each task runs
under every arm before the next round starts, in an order shuffled by the
seed, so a slow afternoon or a provider incident lands on both arms rather
than on one. Each pair (task, harness, round) is what the report compares.
"""

from __future__ import annotations

import hashlib
import json
import os
import random
import shutil
import subprocess
import tempfile
import threading
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from . import arms as arms_mod
from . import codex_hooks, credentials, timing
from . import costs as costs_mod
from . import init as init_mod
from . import tasks as tasks_mod
from .cleanup import remove
from .fake_github import FakeGitHub
from .harness import HARNESSES, Harness, codex_shell_metrics

REPO_ROOT = Path(__file__).resolve().parents[2]
RESULTS_DIR = REPO_ROOT / "bench" / "results"


class ResultsWriteError(Exception):
    """The latest run could not be saved; earlier records remain intact."""


def append_result(path: Path, record: dict[str, Any]) -> None:
    """Replace the JSONL only after its old contents and new record are complete."""
    try:
        previous = path.read_bytes()
    except FileNotFoundError:
        previous = b""
    fd, name = tempfile.mkstemp(prefix=".runs-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as out:
            out.write(previous)
            out.write((json.dumps(record) + "\n").encode())
            out.flush()
            os.fsync(out.fileno())
        os.replace(name, path)
    finally:
        Path(name).unlink(missing_ok=True)


def snapshot_git_env() -> dict[str, str]:
    """Use benchmark Git identity without inherited command-scope overrides."""
    env = {k: v for k, v in os.environ.items() if k not in {"GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS"}
           and not k.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))}
    return env | tasks_mod.GIT_ENV


def worktree_snapshot(workdir: Path) -> tuple[str, str] | None:
    """Snapshot the index and unignored worktree without changing the real index.

    This is taken outside the timed agent run. An isolated index includes
    committed, staged and untracked work without disturbing the agent's index.
    """
    try:
        with tempfile.TemporaryDirectory() as directory:
            index = Path(directory) / "index"
            env = snapshot_git_env()
            git_index = subprocess.run(
                ["git", "rev-parse", "--git-path", "index"], cwd=workdir, env=env,
                capture_output=True, check=True, timeout=120,
            ).stdout.decode().strip()
            original = workdir / git_index
            env["GIT_INDEX_FILE"] = str(index)
            if original.exists():
                shutil.copyfile(original, index)
            else:
                subprocess.run(["git", "read-tree", "HEAD"], cwd=workdir, env=env,
                               capture_output=True, check=True, timeout=120)
            trees = []
            for args in (["write-tree"], ["add", "-A"], ["write-tree"]):
                result = subprocess.run(
                    ["git", "-c", f"core.excludesFile={os.devnull}", *args], cwd=workdir, env=env,
                    capture_output=True, check=True, timeout=120,
                )
                if args == ["write-tree"]:
                    trees.append(result.stdout.decode("ascii").strip())
            return (trees[0], trees[1])
    except (OSError, UnicodeError, subprocess.SubprocessError):
        return None


def changed_paths(workdir: Path, before: tuple[str, str] | None, after: tuple[str, str] | None) -> list[str] | None:
    """Names changed between snapshots, or None if evidence is unavailable."""
    if before is None or after is None:
        return None
    try:
        paths = set()
        for old, new in zip(before, after):
            result = subprocess.run(
                ["git", "diff", "--name-only", "--no-renames", "-z", old, new, "--"],
                cwd=workdir, env=snapshot_git_env(),
                capture_output=True, check=True, timeout=120,
            )
            paths.update(os.fsdecode(path) for path in result.stdout.split(b"\0") if path)
        return sorted(paths)
    except (OSError, subprocess.SubprocessError):
        return None


@dataclass
class Planned:
    round: int
    task: str
    harness: str
    arm: str

    @property
    def id(self) -> str:
        return f"{self.task}.{self.harness}.{self.arm}.r{self.round}"

    @property
    def directory(self) -> str:
        return hashlib.sha256(self.id.encode()).hexdigest()[:16]


def plan(task_names: list[str], harnesses: list[str], arm_names: list[str], rounds: int, seed: int) -> list[Planned]:
    rng = random.Random(seed)
    out = []
    for r in range(1, rounds + 1):
        for task in task_names:
            for harness in harnesses:
                order = list(arm_names)
                rng.shuffle(order)
                out.extend(Planned(r, task, harness, arm) for arm in order)
    return out


def version(cmd: list[str]) -> str | None:
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return (proc.stdout or proc.stderr).strip().splitlines()[0] if proc.returncode == 0 else None


def analysis_env(env: dict[str, str]) -> dict[str, str]:
    """The run's environment for jouet's own readers, without its wrappers."""
    return {k: v for k, v in env.items() if k != "PATH"} | {"PATH": "/usr/bin:/bin"}


def audit(jouet_bin: Path, harness: Harness, env: dict[str, str]) -> dict[str, Any] | None:
    """`jouet audit --json` over the run's isolated transcripts."""
    proc = subprocess.run(
        [str(jouet_bin), "audit", "--json", "--harness", harness.name, "--examples"],
        env=analysis_env(env),
        capture_output=True,
        text=True,
        timeout=120,
    )
    if proc.returncode != 0:
        return None
    report = json.loads(proc.stdout)
    for h in report.get("harnesses", []):
        if h.get("harness") == harness.name:
            return h
    return None


def ledger(env: dict[str, str]) -> dict[str, Any]:
    """Rule decisions and hook events from the run's jouet ledger."""
    root = Path(env["XDG_STATE_HOME"]) / "jouet" / "ledger"
    decisions: dict[str, int] = {}
    events: dict[str, int] = {}
    if root.exists():
        for path in sorted(root.rglob("*.jsonl")):
            for line in path.read_text(errors="replace").splitlines():
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                events[entry.get("event", "?")] = events.get(entry.get("event", "?"), 0) + 1
                for d in entry.get("decisions", []):
                    key = f"{d.get('rule')}:{d.get('action')}"
                    decisions[key] = decisions.get(key, 0) + 1
    return {"events": events, "decisions": decisions}


def summarize_audit(report: dict[str, Any] | None) -> dict[str, Any]:
    if not report:
        return {}
    totals = report.get("totals", {})
    categories = {c["category"]: c.get("calls", 0) for c in report.get("categories", []) if c.get("calls")}
    patterns = {p["pattern"]: p.get("calls", 0) for p in report.get("patterns", [])}
    shapes = []
    for c in report.get("categories", []):
        for s in c.get("top_shapes", []):
            shapes.append({"category": c["category"], "shape": s["shape"], "calls": s["calls"]})
    return {
        "requests": totals.get("requests", 0),
        "calls": totals.get("calls", 0),
        "commands": totals.get("commands", 0),
        "failed_calls": totals.get("failed_calls", 0),
        "command_ms": totals.get("command_ms", 0),
        "tool_output_bytes": totals.get("output_bytes", 0),
        "audit_input_tokens": totals.get("input_tokens"),
        "categories": categories,
        "patterns": patterns,
        "shapes": shapes,
    }


class Budget:
    """Stops the suite once its token total is spent."""

    def __init__(self, max_total_tokens: int | None):
        self.max_total_tokens = max_total_tokens
        self.spent = 0

    def exhausted(self) -> bool:
        return self.max_total_tokens is not None and self.spent >= self.max_total_tokens


class DiskSamples:
    """Free space observed on the run's filesystems, without storing paths."""

    def __init__(self, paths: dict[str, Path]):
        self.paths = paths
        self.values: dict[str, dict[str, int | None]] = {
            name: {"start_bytes": None, "end_bytes": None, "min_bytes": None, "samples": 0, "errors": 0}
            for name in paths
        }

    def below(self, minimum: int) -> bool:
        return minimum > 0 and any(entry["end_bytes"] is not None and entry["end_bytes"] < minimum
                                   for entry in self.values.values())

    def sample(self) -> None:
        for name, path in self.paths.items():
            entry = self.values[name]
            first = entry["samples"] == 0 and entry["errors"] == 0
            try:
                free = shutil.disk_usage(path).free
            except OSError:
                entry["end_bytes"] = None
                entry["errors"] += 1
                continue
            if first:
                entry["start_bytes"] = free
            entry["end_bytes"] = free
            previous = entry["min_bytes"]
            entry["min_bytes"] = free if previous is None else min(previous, free)
            entry["samples"] += 1


def run_agent(
    harness: Harness,
    cmd: list[str],
    env: dict[str, str],
    workdir: Path,
    config_dir: Path,
    timeout_s: int,
    max_tokens: int | None,
    stream_path: Path,
    min_free_bytes: int = 0,
    disk_guard_event: threading.Event | None = None,
) -> dict[str, Any]:
    """Runs the agent, stopping at the timeout, token cap or disk reserve."""
    lines: list[str] = []
    elapsed: list[float] = []
    stopped: list[str] = []
    reader_lock = threading.Lock()
    reader_closed = threading.Event()
    reader_eof = threading.Event()
    tmp = Path(env.get("TMPDIR") or workdir)
    if not tmp.is_absolute():
        tmp = workdir / tmp
    disk = DiskSamples({"work": workdir, "tmp": tmp, "results": stream_path.parent})
    disk.sample()
    if disk.below(min_free_bytes):
        if disk_guard_event is not None:
            disk_guard_event.set()
        return {"wall_s": 0, "exit_code": None, "stopped": "disk_guard", "lines": [], "elapsed": [],
                "stream_complete": True, "disk_free": disk.values, "session": None}
    with stream_path.open("w") as stream, (stream_path.parent / "agent.stderr").open("w") as err:
        proc = subprocess.Popen(
            cmd,
            cwd=workdir,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=err,
            text=True,
            start_new_session=True,
        )
        started = time.perf_counter()

        def read() -> None:
            assert proc.stdout is not None
            for line in proc.stdout:
                at = round(time.perf_counter() - started, 6)
                with reader_lock:
                    if reader_closed.is_set():
                        return
                    stream.write(line)
                    stream.flush()
                    lines.append(line)
                    elapsed.append(at)
            reader_eof.set()

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        try:
            while True:
                try:
                    proc.wait(timeout=1)
                    break
                except subprocess.TimeoutExpired:
                    pass
                disk.sample()
                if disk.below(min_free_bytes):
                    if disk_guard_event is not None:
                        disk_guard_event.set()
                    stopped.append("disk_guard")
                elif time.perf_counter() - started > timeout_s:
                    stopped.append("timeout")
                elif max_tokens and harness.tokens_so_far(list(lines), config_dir) > max_tokens:
                    stopped.append("token_cap")
                if stopped:
                    break
        finally:
            # Also on Ctrl-C: the agent runs in its own process group, which
            # would otherwise outlive the harness.
            if proc.poll() is None:
                kill(proc)
        proc.wait()
        wall_s = time.perf_counter() - started
        disk.sample()
        if disk.below(min_free_bytes):
            if disk_guard_event is not None:
                disk_guard_event.set()
            stopped[:] = ["disk_guard"]
        reader.join(timeout=10)
        # A descendant can keep stdout open after the agent exits. Freeze the
        # transcript and timestamps together before closing the stream; a
        # late reader must neither append nor write to the closed file.
        with reader_lock:
            reader_closed.set()
            complete = reader_eof.is_set()
            captured_lines = list(lines)
            captured_elapsed = list(elapsed)
        # One timestamp per line; no transcript content or project paths.
        if not stopped or stopped[0] != "disk_guard":
            (stream_path.parent / "agent-timing.json").write_text(json.dumps({
                "elapsed_s": captured_elapsed, "stream_complete": complete,
            }))
    return {
        "wall_s": round(wall_s, 3),
        "exit_code": proc.returncode,
        "stopped": stopped[0] if stopped else None,
        "lines": captured_lines,
        "elapsed": captured_elapsed,
        "stream_complete": complete,
        "disk_free": disk.values,
        # The agent leads a session of its own; what it started and left
        # running is still in it, unless it started a session too.
        "session": proc.pid,
    }


def kill(proc: subprocess.Popen[str]) -> None:
    import os
    import signal

    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(proc.pid, sig)
        except ProcessLookupError:
            return
        try:
            proc.wait(timeout=10)
            return
        except subprocess.TimeoutExpired:
            continue


def sweep(run_dir: Path, session: int | None) -> list[str]:
    """Stops what a run left running, and returns each one's command line.

    A run's processes are ours that are in the agent's session, or whose
    working directory or arguments are in the run's directory: a test
    suite's watchers can start sessions of their own and outlive both the
    agent and the check. On macOS, lsof finds processes by working directory;
    detached processes that leave that directory may not be found.
    """
    import os
    import signal

    found = {}
    stopped_session = None
    discovery_error = None
    proc_dir = Path("/proc")
    if proc_dir.is_dir():
        under = str(run_dir)
        for entry in proc_dir.iterdir():
            if not entry.name.isdigit() or int(entry.name) == os.getpid():
                continue
            try:
                if entry.stat().st_uid != os.getuid():
                    continue
                stat = (entry / "stat").read_text()
                args = [a.decode(errors="replace") for a in (entry / "cmdline").read_bytes().split(b"\0") if a]
                cwd = os.readlink(entry / "cwd")
            except OSError:
                continue
            # The fields after the name in parentheses: state, ppid, pgrp, session.
            sid = int(stat.rsplit(")", 1)[1].split()[3])
            inside = any(s.endswith(under) or under + "/" in s for s in [cwd, *args])
            if inside or (session is not None and sid == session):
                found[int(entry.name)] = " ".join(args)[:200]
    else:
        matches, discovery_error = cwd_processes(run_dir)
        found.update(matches)
        if discovery_error:
            import sys

            print(f"benchmark cleanup: {discovery_error}", file=sys.stderr)
        if session is not None:
            try:
                os.killpg(session, signal.SIGKILL)
                stopped_session = f"session {session}"
            except (ProcessLookupError, PermissionError):
                pass
    for sig in (signal.SIGTERM, signal.SIGKILL):
        alive = []
        for pid in found:
            try:
                os.kill(pid, sig)
                alive.append(pid)
            except (ProcessLookupError, PermissionError):
                pass
        deadline = time.monotonic() + 5
        while alive and time.monotonic() < deadline:
            alive = [pid for pid in alive if process_alive(pid)]
            time.sleep(0.1)
        if not alive:
            break
    return list(found.values()) + ([stopped_session] if stopped_session else []) + ([discovery_error] if discovery_error else [])


def cwd_processes(run_dir: Path) -> tuple[dict[int, str], str | None]:
    """Find this user's processes whose working directory is inside a run."""
    import os

    try:
        proc = subprocess.run(
            ["lsof", "-n", "-P", "-a", "-u", str(os.getuid()), "-d", "cwd", "-Fpn"],
            capture_output=True, text=True, timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return {}, f"process discovery failed: {error}"
    if proc.returncode:
        return {}, f"process discovery failed: lsof exited {proc.returncode}"
    root = run_dir.resolve()
    found = {}
    pid = None
    for line in proc.stdout.splitlines():
        if line.startswith("p") and line[1:].isdigit():
            pid = int(line[1:])
        elif line.startswith("n") and pid and pid != os.getpid():
            # lsof doubles literal backslashes. Do not decode ambiguous
            # control-character escapes into a potentially unrelated path.
            cwd = Path(line[1:].replace("\\\\", "\\"))
            if cwd == root or root in cwd.parents:
                found[pid] = f"pid {pid}: cwd {cwd}"
    return found, None


def process_alive(pid: int) -> bool:
    import os

    if Path("/proc").is_dir():
        return Path(f"/proc/{pid}").exists() and not zombie(pid)
    try:
        # A zombie can persist until its parent reaps it and cannot be killed.
        proc = subprocess.run(["ps", "-p", str(pid), "-o", "stat="], capture_output=True, text=True, timeout=2)
        return proc.returncode == 0 and bool(proc.stdout.strip()) and not proc.stdout.strip().startswith("Z")
    except (OSError, subprocess.TimeoutExpired):
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False


def zombie(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] == "Z"
    except OSError:
        return True


def task_dir(name: str) -> str | None:
    """The directory a task came from, for a record whose run failed before loading it."""
    try:
        found = tasks_mod.index().get(name)
    except (OSError, ValueError):
        return None
    return str(found.parent) if found else None


def execute(
    p: Planned,
    opts: dict[str, Any],
    work_root: Path,
    out_dir: Path,
) -> dict[str, Any]:
    """One run: a fresh work tree and home, the agent, the check, metrics."""
    task = tasks_mod.load(p.task)
    harness = HARNESSES[p.harness]
    arm = arms_mod.ARMS[p.arm]
    # Keep task names out of paths: programs and tests can inspect their
    # working and temporary directories, so those names can change behavior.
    run_name = p.directory
    run_dir = work_root / run_name
    workdir = run_dir / "work"
    tmp_dir = Path(opts["tmp_root"]) / run_name
    keep = out_dir / "runs" / p.id
    keep.mkdir(parents=True, exist_ok=True)
    shas = tasks_mod.prepare(task, workdir)
    init = None
    if arm.init:
        # Made once per task and build, by running the skill, then reused.
        init = init_mod.prepared(task, harness, shas, opts, work_root, run_agent)
        init_mod.apply(workdir, init["patch"], task, shas)
    if note := (opts.get("cost_notes") or {}).get(task.name):
        costs_mod.apply(workdir, note, shas["head_sha"])
    fixture = task.github_fixture(shas)
    state_path = keep / "github-state.json"

    def go(fake_url: str | None) -> dict[str, Any]:
        env = arms_mod.build(
            run_dir,
            harness,
            arm,
            opts["harness_bins"][harness.name],
            opts["jouet_bin"],
            fake_url,
            "jdx/demo",
            tmp_dir,
        )
        codex_home = env.get("CODEX_HOME")
        codex_tools = arms_mod.codex_tools(run_dir / "bin", arm.jouet) if harness.name == "codex" else None
        if harness.name == "codex" and arms_mod.CODEX_ISOLATION_ENV.intersection(task.env):
            raise codex_hooks.HookPreflightError("A task changed a Codex isolation path")
        task.agent_env(env, run_dir / "bin", workdir)
        if codex_tools is not None and arms_mod.codex_tools(run_dir / "bin", arm.jouet) != codex_tools:
            raise codex_hooks.HookPreflightError("A task replaced a Codex benchmark tool")
        if harness.name == "codex" and (env.get("CODEX_HOME") != codex_home or
                                        arms_mod.CODEX_ISOLATION_ENV.intersection(opts.get("jouet_env") or {})):
            raise codex_hooks.HookPreflightError("A task or override changed CODEX_HOME")
        environment = tasks_mod.setup_environment(task, env, workdir, work_root / "python-cache", keep / "environment.log")
        if codex_tools is not None:
            if arms_mod.codex_tools(run_dir / "bin", arm.jouet) != codex_tools:
                raise codex_hooks.HookPreflightError("Setup replaced a Codex benchmark tool")
            arms_mod.verify_codex_path(run_dir / "bin", env, arm.jouet)
        setup = arms_mod.setup_jouet(harness, env, workdir) if arm.jouet else None
        if arm.jouet:
            arms_mod.reviewed_codex(harness, env, workdir, opts)
        if arm.init:
            init_mod.trust(env, workdir)
            if init_mod.has_mise(workdir):
                init_mod.mise_env(env, workdir, created=bool(init and init["meta"].get("mise_created")))
        if arm.jouet:
            # After setup, so that turning priming off here measures a
            # session without the primer rather than one with the skill.
            env.update(opts.get("jouet_env") or {})
        arms_mod.login_shell_path(env)
        before = worktree_snapshot(workdir)
        cmd = harness.command(harness.binary, task.prompt, opts, workdir)
        timeout_s = min(task.timeout_s, opts["timeout_s"]) if opts.get("timeout_s") else task.timeout_s
        config_dir = Path(env[harness.config_dir_env])
        try:
            lent = credentials.lend(arms_mod.credential_files(harness), config_dir,
                                    exclusive=harness.name == "codex" and arm.jouet)
        except FileExistsError as error:
            raise codex_hooks.HookPreflightError("Dedicated Codex credentials appeared during setup") from error
        try:
            agent = run_agent(
                harness, cmd, env, workdir, config_dir, timeout_s, opts.get("max_tokens_per_run"),
                keep / "agent.jsonl", opts.get("min_free_bytes", 0), opts.get("disk_guard_event")
            )
        finally:
            # Before anything else can fail: a refresh the run made is the
            # user's only working login.
            notes = (credentials.give_back_and_remove(lent) if harness.name == "codex"
                     else credentials.give_back(lent))
        usage = harness.usage(agent["lines"], config_dir)
        return {
            "env": env,
            "setup": setup,
            "agent": agent,
            "usage": usage,
            "config_dir": config_dir,
            "credentials": notes,
            "environment": environment,
            "before_snapshot": before,
        }

    if fixture is not None:
        with FakeGitHub(fixture, keep / "github-requests.jsonl") as fake:
            ran = go(fake.url)
            if ran["agent"]["stopped"] != "disk_guard":
                state_path.write_text(json.dumps(fake.world.fixture, indent=2))
            github = fake.world.summary()
    else:
        ran = go(None)
        github = None

    env = ran["env"]
    left = sweep(run_dir, ran["agent"]["session"]) + sweep(tmp_dir, None)
    disk_stopped = ran["agent"]["stopped"] == "disk_guard"
    changes = None if disk_stopped else changed_paths(workdir, ran["before_snapshot"], worktree_snapshot(workdir))
    # The agent's environment without its credentials: the check builds and
    # runs what the agent did, in the same home, cache and TMPDIR.
    base = {k: v for k, v in env.items() if k not in harness.auth_env}
    usage = ran["usage"]
    if ran["agent"]["stopped"] or ran["agent"]["exit_code"] != 0:
        # A captured total from an interrupted or failed process may be only
        # a lower bound. Keep it for budget enforcement and mark it partial.
        usage.extra["usage_complete"] = False
    capacity = (usage.is_error and not ran["agent"]["stopped"]
                and usage.extra.get("capacity_error", False))
    missing_hook = harness.name == "codex" and arm.jouet and not arms_mod.codex_started(env)
    missing_usage = harness.name == "codex" and usage.extra.get("usage_observed") is False
    if ran["agent"]["stopped"] == "disk_guard":
        check = {"passed": None, "exit_code": None, "output": "Check skipped: disk reserve reached."}
    elif missing_hook:
        check = {"passed": False, "exit_code": None, "output": "Check skipped: jouet's Codex session-start hook was not observed."}
    elif capacity:
        check = {"passed": False, "exit_code": None, "output": "Check skipped: Codex model at capacity."}
    elif missing_usage:
        check = {"passed": False, "exit_code": None, "output": "Check skipped: Codex reported no token usage."}
    else:
        check = tasks_mod.run_check(task, workdir, base, tasks_mod.check_env(shas, state_path if fixture else None, task.check_docker))
    left += sweep(run_dir, None) + sweep(tmp_dir, None)
    # Ephemeral Codex sessions have no transcript for jouet audit to inspect.
    audit_report = None if harness.name == "codex" or disk_stopped else audit(opts["jouet_bin"], harness, env)
    if not disk_stopped:
        (keep / "audit.json").write_text(json.dumps(audit_report, indent=2))
    agent = ran["agent"]
    record = {
        "id": p.id,
        "round": p.round,
        "task": p.task,
        "task_dir": str(task.dir.parent),
        "harness": p.harness,
        "arm": p.arm,
        "command_activity": (command_activity := timing.codex_commands(agent["lines"], agent.get("elapsed", []),
                                                                        agent.get("stream_complete", False))
                             if harness.name == "codex" else None),
        "check": {k: v for k, v in check.items() if k != "output"},
        "stopped": agent["stopped"],
        "exit_code": agent["exit_code"],
        "wall_s": agent["wall_s"],
        "disk_free": agent.get("disk_free"),
        "usage": asdict(usage),
        "audit": summarize_audit(audit_report),
        "shell": (shell := (codex_shell_metrics(agent["lines"])
                  if harness.name == "codex" and not agent["stopped"] and agent["exit_code"] == 0
                  else None)),
        **check_outcome(agent, check, command_activity, shell, harness.name),
        "ledger": ledger(env) if arm.jouet and not disk_stopped else None,
        "github": github,
        "setup": ran["setup"],
        "init": {"cached": init["cached"], **init["meta"]} if init else None,
        "environment": ran["environment"],
        "changed_paths": changes,
        "credentials": ran["credentials"],
        "left_running": left,
    }
    if agent["stopped"] == "disk_guard":
        record["error"] = "Disk reserve reached; task result is unverified"
        record["error_kind"] = "disk_guard"
    elif missing_hook:
        record["error"] = "Codex did not record the jouet session-start hook"
        record["error_kind"] = "codex_hook_preflight"
    elif capacity:
        record["error"] = "Codex model at capacity before the run completed"
        if missing_usage:
            record["error_kind"] = "codex_usage_unavailable"
    elif missing_usage:
        record["error"] = "Codex token usage is unavailable"
        record["error_kind"] = "codex_usage_unavailable"
    if opts.get("keep_work"):
        record["workdir"] = str(workdir)
        record["tmp_dir"] = str(tmp_dir)
    try:
        (keep / "check.log").write_text(check["output"] + "\n")
        (keep / "result.json").write_text(json.dumps(record, indent=2))
    except OSError:
        if not disk_stopped:
            raise
    if harness.name != "codex" and not disk_stopped:
        copy_transcripts(ran["config_dir"], keep / "transcripts")
    if not opts.get("keep_work"):
        remove(run_dir)
        remove(tmp_dir)
    return record


def snapshot(jouet_bin: Path, work_root: Path) -> Path:
    """A copy of the jouet under test that the whole suite runs.

    `target/debug/jouet` can be a link into a shared build cache (mbx's), and
    a build in another checkout during a suite that runs for hours can clean
    it up. A copy also pins what is measured: rebuilding mid-suite does not
    change the binary later runs use."""
    copy = work_root / "jouet-under-test" / "jouet"
    copy.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(jouet_bin.resolve(), copy)
    copy.chmod(0o755)
    return copy


def auth_failed(record: dict[str, Any]) -> bool:
    """Whether a run failed before the model answered, for lack of a login."""
    u = record.get("usage") or {}
    return bool(u.get("is_error")) and not u.get("input_tokens") and "authenticat" in str(u.get("result") or "").lower()


def copy_transcripts(config_dir: Path, dest: Path) -> None:
    for sub in ("projects", "sessions"):
        src = config_dir / sub
        if src.exists():
            for path in src.rglob("*.jsonl"):
                target = dest / sub / path.relative_to(src)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, target)


def run_suite(planned: list[Planned], opts: dict[str, Any], out_dir: Path, log: Any,
              on_snapshot: Any = None) -> list[dict[str, Any]]:
    budget = Budget(opts.get("max_total_tokens"))
    # Work trees outside /tmp: a project's tests may refuse a checkout below a
    # directory anyone can write to (mise's brew cask tests do). TMPDIR stays
    # in /tmp, short for sockets, and away from configs above the run root.
    import os

    cache = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    cache.mkdir(parents=True, exist_ok=True)
    work_root = Path(tempfile.mkdtemp(prefix="jb-", dir=cache))
    tmp_root = Path(tempfile.mkdtemp(prefix="jb-", dir="/tmp"))
    opts = {**opts, "jouet_bin": snapshot(opts["jouet_bin"], work_root), "tmp_root": tmp_root}
    records = []
    results_path = out_dir / "runs.jsonl"
    failed_environments = set()
    preserve_work = False
    try:
        if on_snapshot is not None:
            on_snapshot(opts["jouet_bin"])
        opts["cost_notes"] = costs_mod.seeds(planned, opts["jouet_bin"], work_root, log, out_dir / "cost-seeds")
        for n, p in enumerate(planned, 1):
            if p.task in failed_environments:
                log(f"skipping {p.id}: environment setup already failed for {p.task}")
                continue
            if budget.exhausted():
                log(f"budget: {budget.spent} tokens spent of {budget.max_total_tokens}; skipping the rest")
                break
            log(f"[{n}/{len(planned)}] {p.id}")
            guard = threading.Event()
            opts["disk_guard_event"] = guard
            try:
                record = execute(p, opts, work_root, out_dir)
            except Exception as e:  # one broken run must not lose the others
                if isinstance(e, tasks_mod.EnvironmentSetupError):
                    failed_environments.add(p.task)
                record = {
                    "id": p.id,
                    "round": p.round,
                    "task": p.task,
                    "harness": p.harness,
                    "arm": p.arm,
                    "success": False,
                    "error": f"{type(e).__name__}: {e}",
                }
                if guard.is_set() or isinstance(e, init_mod.DiskGuard):
                    record["error_kind"] = "disk_guard"
                    record["stopped"] = "disk_guard"
                    record["check"] = {"passed": None, "exit_code": None}
                elif isinstance(e, codex_hooks.HookPreflightError):
                    record["error_kind"] = "codex_hook_preflight"
                if found := task_dir(p.task):
                    record["task_dir"] = found
                if opts.get("keep_work"):
                    record["workdir"] = str(work_root / p.directory / "work")
                    record["tmp_dir"] = str(tmp_root / p.directory)
            records.append(record)
            budget.spent += (record.get("usage") or {}).get("input_tokens", 0) + (record.get("usage") or {}).get(
                "output_tokens", 0
            )
            try:
                append_result(results_path, record)
            except OSError as error:
                preserve_work = True
                cause = f"; run error: {record['error']}" if record.get("error") else ""
                raise ResultsWriteError(
                    f"could not save {p.id} to {results_path}: {error}{cause}; "
                    f"earlier saved records were preserved; work: {work_root}; temp: {tmp_root}"
                ) from error
            u = record.get("usage") or {}
            log(
                f"    {'error' if record.get('error') else 'unverified' if record['success'] is None else 'ok' if record['success'] else 'FAILED'}"
                f"  {record.get('wall_s', '-')}s"
                f"  in {u.get('input_tokens', '-')} out {u.get('output_tokens', '-')}"
                + (f"  stopped: {record['stopped']}" if record.get("stopped") else "")
                + (f"  error: {record['error']}" if record.get("error") else "")
            )
            for note in record.get("credentials") or []:
                log(f"    credentials: {note}")
            if record.get("error_kind") == "disk_guard":
                log("stopping: disk reserve reached")
                break
            if record.get("error_kind") == "codex_hook_preflight":
                log("stopping: Codex hooks could not be verified")
                break
            if record.get("error_kind") == "codex_usage_unavailable":
                log("stopping: Codex usage could not be measured")
                break
            if auth_failed(record):
                # Every later run of this harness would fail the same way,
                # and record a failure that says nothing about jouet.
                log(f"stopping: {p.harness} could not authenticate ({u.get('result')}); log in again and rerun")
                break
    finally:
        if not opts.get("keep_work") and not preserve_work:
            remove(work_root)
            remove(tmp_root)
    return records


def check_outcome(agent: dict[str, Any], check: dict[str, Any], activity: dict | None,
                  shell: dict | None, harness: str) -> dict[str, Any]:
    """Keep the independent checker result while flagging unfinished commands."""
    success = False if agent["stopped"] or agent["exit_code"] != 0 else check["passed"]
    issue = timing.verification_issue(activity, shell) if harness == "codex" else None
    if success is True and issue:
        check["output"] += f"\n{issue}; the task outcome is unverified."
        return {"success": None, "unverified_reason": issue}
    return {"success": success}
