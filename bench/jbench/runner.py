"""Planning and running the suite, and collecting each run's metrics.

A suite is rounds × tasks × harnesses × arms. Within a round each task runs
under every arm before the next round starts, in an order shuffled by the
seed, so a slow afternoon or a provider incident lands on both arms rather
than on one. Each pair (task, harness, round) is what the report compares.
"""

from __future__ import annotations

import json
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
from . import credentials
from . import tasks as tasks_mod
from .fake_github import FakeGitHub
from .harness import HARNESSES, Harness

REPO_ROOT = Path(__file__).resolve().parents[2]
RESULTS_DIR = REPO_ROOT / "bench" / "results"


@dataclass
class Planned:
    round: int
    task: str
    harness: str
    arm: str

    @property
    def id(self) -> str:
        return f"{self.task}.{self.harness}.{self.arm}.r{self.round}"


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


def run_agent(
    harness: Harness,
    cmd: list[str],
    env: dict[str, str],
    workdir: Path,
    config_dir: Path,
    timeout_s: int,
    max_tokens: int | None,
    stream_path: Path,
) -> dict[str, Any]:
    """Runs the agent, stopping it at the timeout or the token cap."""
    lines: list[str] = []
    stopped: list[str] = []
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
                lines.append(line)
                stream.write(line)
                stream.flush()

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        try:
            while True:
                try:
                    proc.wait(timeout=1)
                    break
                except subprocess.TimeoutExpired:
                    pass
                if time.perf_counter() - started > timeout_s:
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
        reader.join(timeout=10)
    return {
        "wall_s": round(wall_s, 3),
        "exit_code": proc.returncode,
        "stopped": stopped[0] if stopped else None,
        "lines": lines,
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
    agent and the check. Without /proc (macOS) only the session is stopped.
    """
    import os
    import signal

    found = {}
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
    elif session is not None:
        try:
            os.killpg(session, signal.SIGKILL)
            return [f"session {session}"]
        except (ProcessLookupError, PermissionError):
            return []
    for sig in (signal.SIGTERM, signal.SIGKILL):
        alive = []
        for pid in found:
            try:
                os.kill(pid, sig)
                alive.append(pid)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 5
        while alive and time.monotonic() < deadline:
            alive = [pid for pid in alive if Path(f"/proc/{pid}").exists() and not zombie(pid)]
            time.sleep(0.1)
        if not alive:
            break
    return list(found.values())


def zombie(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] == "Z"
    except OSError:
        return True


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
    run_dir = work_root / p.id
    workdir = run_dir / "work"
    tmp_dir = Path(opts["tmp_root"]) / p.id
    keep = out_dir / "runs" / p.id
    keep.mkdir(parents=True, exist_ok=True)
    shas = tasks_mod.prepare(task, workdir)
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
        task.agent_env(env, run_dir / "bin", workdir)
        setup =arms_mod.setup_jouet(harness, env, workdir) if arm.jouet else None
        if arm.jouet:
            # After setup, so that turning priming off here measures a
            # session without the primer rather than one with the skill.
            env.update(opts.get("jouet_env") or {})
        cmd = harness.command(harness.binary, task.prompt, opts, workdir)
        timeout_s = min(task.timeout_s, opts["timeout_s"]) if opts.get("timeout_s") else task.timeout_s
        config_dir = Path(env[harness.config_dir_env])
        lent = credentials.lend(arms_mod.credential_files(harness), config_dir)
        try:
            agent = run_agent(
                harness, cmd, env, workdir, config_dir, timeout_s, opts.get("max_tokens_per_run"), keep / "agent.jsonl"
            )
        finally:
            # Before anything else can fail: a refresh the run made is the
            # user's only working login.
            notes = credentials.give_back(lent)
        usage = harness.usage(agent["lines"], config_dir)
        return {
            "env": env,
            "setup": setup,
            "agent": agent,
            "usage": usage,
            "config_dir": config_dir,
            "credentials": notes,
        }

    if fixture is not None:
        with FakeGitHub(fixture, keep / "github-requests.jsonl") as fake:
            ran = go(fake.url)
            state_path.write_text(json.dumps(fake.world.fixture, indent=2))
            github = fake.world.summary()
    else:
        ran = go(None)
        github = None

    env = ran["env"]
    left = sweep(run_dir, ran["agent"]["session"]) + sweep(tmp_dir, None)
    # The agent's environment without its credentials: the check builds and
    # runs what the agent did, in the same home, cache and TMPDIR.
    base = {k: v for k, v in env.items() if k not in harness.auth_env}
    check = tasks_mod.run_check(task, workdir, base, tasks_mod.check_env(shas, state_path if fixture else None))
    left += sweep(run_dir, None) + sweep(tmp_dir, None)
    audit_report = audit(opts["jouet_bin"], harness, env)
    (keep / "audit.json").write_text(json.dumps(audit_report, indent=2))
    agent = ran["agent"]
    usage = ran["usage"]
    record = {
        "id": p.id,
        "round": p.round,
        "task": p.task,
        "harness": p.harness,
        "arm": p.arm,
        "success": check["passed"] and not agent["stopped"],
        "check": {k: v for k, v in check.items() if k != "output"},
        "stopped": agent["stopped"],
        "exit_code": agent["exit_code"],
        "wall_s": agent["wall_s"],
        "usage": asdict(usage),
        "audit": summarize_audit(audit_report),
        "ledger": ledger(env) if arm.jouet else None,
        "github": github,
        "setup": ran["setup"],
        "credentials": ran["credentials"],
        "left_running": left,
    }
    (keep / "check.log").write_text(check["output"] + "\n")
    (keep / "result.json").write_text(json.dumps(record, indent=2))
    copy_transcripts(ran["config_dir"], keep / "transcripts")
    if not opts.get("keep_work"):
        remove(run_dir)
        remove(tmp_dir)
    return record


def remove(path: Path) -> None:
    """Deletes a run's tree, including directories a project's tests left
    without read or search permission (mise's leave `d-w-------` ones)."""

    def unlock(_func: Any, failed: str, _exc: BaseException) -> None:
        for p in (Path(failed).parent, Path(failed)):
            try:
                p.chmod(0o700)
            except OSError:
                pass
        if Path(failed).is_dir():
            shutil.rmtree(failed, onexc=unlock)
        else:
            Path(failed).unlink(missing_ok=True)

    if path.exists():
        shutil.rmtree(path, onexc=unlock)


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


def run_suite(planned: list[Planned], opts: dict[str, Any], out_dir: Path, log: Any) -> list[dict[str, Any]]:
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
    try:
        for n, p in enumerate(planned, 1):
            if budget.exhausted():
                log(f"budget: {budget.spent} tokens spent of {budget.max_total_tokens}; skipping the rest")
                break
            log(f"[{n}/{len(planned)}] {p.id}")
            try:
                record = execute(p, opts, work_root, out_dir)
            except Exception as e:  # one broken run must not lose the others
                record = {
                    "id": p.id,
                    "round": p.round,
                    "task": p.task,
                    "harness": p.harness,
                    "arm": p.arm,
                    "success": False,
                    "error": f"{type(e).__name__}: {e}",
                }
            records.append(record)
            budget.spent += (record.get("usage") or {}).get("input_tokens", 0) + (record.get("usage") or {}).get(
                "output_tokens", 0
            )
            with results_path.open("a") as f:
                f.write(json.dumps(record) + "\n")
            u = record.get("usage") or {}
            log(
                f"    {'ok' if record['success'] else 'FAILED'}"
                f"  {record.get('wall_s', '-')}s"
                f"  in {u.get('input_tokens', '-')} out {u.get('output_tokens', '-')}"
                + (f"  stopped: {record['stopped']}" if record.get("stopped") else "")
                + (f"  error: {record['error']}" if record.get("error") else "")
            )
            for note in record.get("credentials") or []:
                log(f"    credentials: {note}")
            if auth_failed(record):
                # Every later run of this harness would fail the same way,
                # and record a failure that says nothing about jouet.
                log(f"stopping: {p.harness} could not authenticate ({u.get('result')}); log in again and rerun")
                break
    finally:
        if not opts.get("keep_work"):
            remove(work_root)
            remove(tmp_root)
    return records
