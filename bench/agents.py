#!/usr/bin/env python3
"""Measure what jouet does for coding agents: the same tasks, with and without it.

    python3 bench/agents.py run --dry-run            # print the plan, spend nothing
    python3 bench/agents.py run --smoke              # one cheap task, once per arm
    python3 bench/agents.py run --harness claude --rounds 5
    python3 bench/agents.py report bench/results/<id>
    python3 bench/agents.py check-tasks              # no agents: prove each task's check
    python3 bench/agents.py run --tasks-dir ~/bench-tasks --tasks NAME   # a task kept elsewhere

See docs/benchmarks.md for the method and its limits.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from datetime import UTC, datetime
from pathlib import Path

if sys.version_info < (3, 11):
    sys.exit("bench/agents.py needs Python 3.11 or newer (for tomllib)")
sys.path.insert(0, str(Path(__file__).resolve().parent))

from jbench import arms, init, report, runner, tasks  # noqa: E402
from jbench.fake_github import FakeGitHub  # noqa: E402
from jbench.harness import HARNESSES, codex_shell_metrics  # noqa: E402

DEFAULTS = {
    "claude_model": "claude-sonnet-5-5",
    "codex_model": "gpt-6-sol",
    "effort": "medium",
    "rounds": 3,
    "max_runs": 40,
    "max_tokens_per_run": 4_000_000,
    "max_usd_per_run": 3.0,
    "max_total_tokens": 60_000_000,
}


TASKS_DIR_HELP = "another directory of tasks, after bench/tasks/ (repeatable; also JOUET_BENCH_TASKS)"


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def csv(value: str) -> list[str]:
    return [v.strip() for v in value.split(",") if v.strip()]


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="bench/agents.py", description=__doc__.split("\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)

    run = sub.add_parser("run", help="run the suite (or print its plan with --dry-run)")
    run.add_argument("--harness", type=csv, default=["claude", "codex"], help="claude,codex (default both)")
    run.add_argument("--tasks", type=csv, help="task names (default every task)")
    run.add_argument("--tasks-dir", type=Path, action="append", default=[], help=TASKS_DIR_HELP)
    run.add_argument("--arms", type=csv, default=["plain", "jouet"], help="plain, jouet and jouet-init (default plain,jouet)")
    run.add_argument("--rounds", type=int, default=DEFAULTS["rounds"], help="runs per task, harness and arm")
    run.add_argument("--seed", type=int, help="shuffles the arm order in each round (default: the time)")
    run.add_argument("--smoke", action="store_true", help="only the smoke task, one round, labelled a smoke test")
    run.add_argument("--dry-run", action="store_true", help="print the plan and the budget, run nothing")
    run.add_argument("--claude-model", default=DEFAULTS["claude_model"])
    run.add_argument("--codex-model", default=DEFAULTS["codex_model"])
    run.add_argument("--effort", default=DEFAULTS["effort"], help="reasoning effort for both harnesses")
    run.add_argument("--max-runs", type=int, default=DEFAULTS["max_runs"], help="refuse a plan with more runs")
    run.add_argument(
        "--max-tokens-per-run",
        type=int,
        default=DEFAULTS["max_tokens_per_run"],
        help="stop a run once its input plus output tokens pass this",
    )
    run.add_argument(
        "--max-usd-per-run",
        type=float,
        default=DEFAULTS["max_usd_per_run"],
        help="Claude Code's --max-budget-usd (Codex reports no cost)",
    )
    run.add_argument(
        "--max-total-tokens",
        type=int,
        default=DEFAULTS["max_total_tokens"],
        help="skip the remaining runs once the suite has spent this many tokens",
    )
    run.add_argument("--timeout", type=int, help="cap every task's timeout, in seconds")
    run.add_argument("--jouet", type=Path, default=runner.REPO_ROOT / "target" / "debug" / "jouet")
    run.add_argument("--codex-reviewed-home", type=Path,
                     help="dedicated CODEX_HOME whose jouet hooks were reviewed in Codex")
    run.add_argument(
        "--jouet-env",
        action="append",
        default=[],
        metavar="KEY=VALUE",
        help="set this in the jouet arm's agent environment, after `jouet setup` (repeatable), "
        "such as JOUET_HOOKS_PRIME=0 to measure a session without the primer",
    )
    run.add_argument("--out", type=Path, help="results directory (default bench/results/<timestamp>)")
    run.add_argument("--label", help="a line shown at the top of the summary")
    run.add_argument("--keep-work", action="store_true", help="keep each run's work tree and home")

    rep = sub.add_parser("report", help="rewrite summary.json and summary.md from a results directory")
    rep.add_argument("dir", type=Path)

    chk = sub.add_parser("check-tasks", help="prove each task's check without running an agent")
    chk.add_argument("--tasks", type=csv)
    chk.add_argument("--tasks-dir", type=Path, action="append", default=[], help=TASKS_DIR_HELP)
    chk.add_argument("--jouet", type=Path, default=runner.REPO_ROOT / "target" / "debug" / "jouet")

    prepare = sub.add_parser("prepare-codex", help="prepare a new isolated Codex home for hook review")
    prepare.add_argument("--home", required=True, type=Path, help="new dedicated benchmark home")
    prepare.add_argument("--jouet", type=Path, default=runner.REPO_ROOT / "target" / "debug" / "jouet")
    return parser.parse_args(argv)


def git_commit() -> str | None:
    proc = subprocess.run(
        ["git", "rev-parse", "--short=12", "HEAD"], cwd=runner.REPO_ROOT, capture_output=True, text=True
    )
    return proc.stdout.strip() or None


def cmd_prepare_codex(args: argparse.Namespace) -> int:
    """Prepare the exact hooks for review, without changing Codex trust."""
    home = args.home.expanduser().resolve()
    binary = args.jouet.expanduser().resolve()
    try:
        for root in tasks.checkout_roots(runner.REPO_ROOT):
            if home.is_relative_to(root) or any(
                ancestor.exists() and ancestor.samefile(root) for ancestor in (home, *home.parents)
            ):
                print("the benchmark home must be outside jouet checkouts", file=sys.stderr)
                return 2
    except (OSError, RuntimeError, subprocess.CalledProcessError):
        print("cannot verify jouet worktree locations", file=sys.stderr)
        return 2
    if not binary.is_file():
        print(f"no jouet binary at {binary}", file=sys.stderr)
        return 2
    try:
        home.mkdir(parents=True, mode=0o700, exist_ok=False)
    except OSError as error:
        print(f"cannot create a new benchmark home: {error}", file=sys.stderr)
        return 2
    config = home / ".codex"
    env = {**os.environ, "HOME": str(home), "CODEX_HOME": str(config),
           "XDG_CONFIG_HOME": str(home / ".config"), "XDG_CACHE_HOME": str(home / ".cache"),
           "XDG_DATA_HOME": str(home / ".local/share"), "XDG_STATE_HOME": str(home / ".local/state")}
    try:
        result = subprocess.run([str(binary), "setup", "codex", "--json"], cwd=home, env=env,
                                capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"jouet setup failed: {error}", file=sys.stderr)
        return 1
    if result.returncode or not (config / "hooks.json").is_file():
        print("jouet setup did not create the Codex hooks", file=sys.stderr)
        return 1
    try:
        with (config / "config.toml").open("x") as file:
            file.write('cli_auth_credentials_store = "file"\n')
    except OSError as error:
        print(f"cannot configure the dedicated credential store: {error}", file=sys.stderr)
        return 1
    (home / ".jouet-bench-codex-home").write_text("jouet benchmark Codex home\n")
    print(f"Prepared Codex hooks in {config}. Review them in Codex with /hooks before running trials.")
    print(f"Use --codex-reviewed-home {config} when running the benchmark.")
    return 0


def use_task_dirs(args: argparse.Namespace) -> list[str] | None:
    """Adds --tasks-dir to the task directories; None, after saying why, when they cannot be read."""
    tasks.EXTRA_DIRS[:] = args.tasks_dir
    try:
        return tasks.all_names()
    except (OSError, ValueError) as error:
        log(f"task directories: {error}")
        return None


def cmd_run(args: argparse.Namespace) -> int:
    if args.codex_reviewed_home:
        args.codex_reviewed_home = args.codex_reviewed_home.expanduser().resolve()
    names = use_task_dirs(args)
    if names is None:
        return 2
    if args.smoke:
        selected = [n for n in names if tasks.load(n).smoke]
        args.rounds = 1
        args.label = args.label or "Smoke test: one run per arm. This checks the harness works; it is not a result."
    else:
        selected = args.tasks or [n for n in names if not tasks.load(n).opt_in]
    unknown = [n for n in selected if n not in names] + [h for h in args.harness if h not in HARNESSES]
    unknown += [a for a in args.arms if a not in arms.ARMS]
    if unknown:
        log(f"unknown task, harness or arm: {', '.join(unknown)}")
        return 2
    bad_env = [e for e in args.jouet_env if "=" not in e]
    if bad_env:
        log(f"--jouet-env takes KEY=VALUE: {', '.join(bad_env)}")
        return 2
    jouet_env = dict(e.split("=", 1) for e in args.jouet_env)
    if "codex" in args.harness and arms.CODEX_ISOLATION_ENV.intersection(jouet_env):
        log("--jouet-env cannot override Codex isolation paths")
        return 2
    seed = args.seed if args.seed is not None else int(time.time())
    planned = runner.plan(selected, args.harness, args.arms, args.rounds, seed)

    problems = []
    bins = {}
    for name in args.harness:
        h = HARNESSES[name]
        found = arms.resolve(h.binary)
        if found is None:
            problems.append(f"{name}: `{h.binary}` is not on PATH")
        else:
            bins[name] = found
        if arms.auth_status(h) is None:
            problems.append(f"{name}: no credentials ({', '.join(h.auth_env)} or {h.auth_files[0]})")
    if any(arms.ARMS[a].jouet for a in args.arms if a in arms.ARMS) and not args.jouet.exists():
        problems.append(f"no jouet binary at {args.jouet}; run `mise run build` or pass --jouet")
    if "codex" in args.harness and any(arms.ARMS[a].jouet for a in args.arms if a in arms.ARMS):
        if not args.codex_reviewed_home:
            problems.append("Codex jouet runs require --codex-reviewed-home with reviewed hooks")
        elif not (args.codex_reviewed_home / "hooks.json").is_file() or not (
            args.codex_reviewed_home.parent / ".jouet-bench-codex-home"
        ).is_file():
            problems.append("use a dedicated home created by prepare-codex")
    if len(planned) > args.max_runs:
        problems.append(f"{len(planned)} runs planned, over --max-runs {args.max_runs}")

    print(f"plan: {len(planned)} runs, seed {seed}")
    for name in selected:
        t = tasks.load(name)
        timeout = min(t.timeout_s, args.timeout) if args.timeout else t.timeout_s
        print(f"  task {name}: {t.chore}; rules {', '.join(t.rules) or '-'}; timeout {timeout}s")
        if t.python_requirements:
            print(f"    Python setup: isolated venv from {t.python_requirements} before each run")
    for name in args.harness:
        model = args.claude_model if name == "claude" else args.codex_model
        print(
            f"  harness {name}: model {model}, effort {args.effort}, auth {arms.auth_status(HARNESSES[name]) or 'none'}"
        )
    print(
        f"  budget: stop a run past {args.max_tokens_per_run:,} tokens"
        f" or ${args.max_usd_per_run} (Claude Code); stop the suite past {args.max_total_tokens:,} tokens"
    )
    for p in planned:
        print(f"  {p.id}")
    for problem in problems:
        print(f"problem: {problem}", file=sys.stderr)
    if args.dry_run:
        return 4 if problems else 0
    if problems:
        return 4

    started = datetime.now(UTC)
    # Absolute: task checks run in their work trees and read state under it.
    out = (args.out or runner.RESULTS_DIR / started.strftime("%Y%m%dT%H%M%SZ")).resolve()
    out.mkdir(parents=True, exist_ok=True)
    meta = {
        "started": started.isoformat(timespec="seconds"),
        "label": args.label,
        "seed": seed,
        "rounds": args.rounds,
        "commit": git_commit(),
        "jouet_version": None,
        "effort": args.effort,
        "tasks": selected,
        "task_dirs": [str(d) for d in tasks.directories()],
        "arms": args.arms,
        "jouet_env": jouet_env,
        "harnesses": {
            name: {
                "version": runner.version([str(bins[name]), "--version"]),
                "model": args.claude_model if name == "claude" else args.codex_model,
            }
            for name in args.harness
        },
        "budget": {
            "max_tokens_per_run": args.max_tokens_per_run,
            "max_usd_per_run": args.max_usd_per_run,
            "max_total_tokens": args.max_total_tokens,
            "timeout_s": args.timeout,
        },
    }
    (out / "meta.json").write_text(json.dumps(meta, indent=2) + "\n")
    opts = {
        "claude_model": args.claude_model,
        "codex_model": args.codex_model,
        "effort": args.effort,
        "max_tokens_per_run": args.max_tokens_per_run,
        "max_usd_per_run": args.max_usd_per_run,
        "max_total_tokens": args.max_total_tokens,
        "timeout_s": args.timeout,
        "jouet_bin": args.jouet.resolve(),
        "codex_reviewed_home": str(args.codex_reviewed_home.resolve()) if args.codex_reviewed_home else None,
        "jouet_env": jouet_env,
        "harness_bins": bins,
        "keep_work": args.keep_work,
    }
    def record_snapshot(binary: Path) -> None:
        with binary.open("rb") as f:
            meta["jouet_sha256"] = hashlib.file_digest(f, "sha256").hexdigest()
        meta["jouet_version"] = runner.version([str(binary), "--version"])
        (out / "meta.json").write_text(json.dumps(meta, indent=2) + "\n")

    lock = None
    if opts["codex_reviewed_home"] and "codex" in args.harness:
        try:
            lock_path = Path(opts["codex_reviewed_home"]).parent / ".jouet-bench.lock"
            lock = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if lock is not None:
                os.close(lock)
            print(f"cannot lock the reviewed Codex home: {error}", file=sys.stderr)
            return 4
    try:
        if opts["codex_reviewed_home"] and "codex" in args.harness and any(
            arms.ARMS[name].jouet for name in args.arms
        ):
            try:
                with tempfile.TemporaryDirectory(prefix="jouet-hook-preflight-") as directory:
                    work = Path(directory).resolve()
                    harness = HARNESSES["codex"]
                    env = arms.build(work, harness, arms.ARMS["jouet"], bins["codex"],
                                     opts["jouet_bin"], None, "jdx/demo")
                    arms.setup_jouet(harness, env, work)
                    arms.reviewed_codex(harness, env, work, opts)
            except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                print(f"Codex hook preflight failed: {error}", file=sys.stderr)
                return 4
        records = runner.run_suite(planned, opts, out, log, on_snapshot=record_snapshot)
    finally:
        if lock is not None:
            os.close(lock)
    write_summary(out, records, meta)
    print(f"results: {out}")
    return 4 if any(r.get("error_kind") in ("codex_hook_preflight", "codex_usage_unavailable")
                    for r in records) else 0


def write_summary(out: Path, records: list[dict], meta: dict) -> None:
    summary = report.summarize(records, meta)
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    (out / "summary.md").write_text(report.markdown(summary))


def cmd_report(args: argparse.Namespace) -> int:
    meta = json.loads((args.dir / "meta.json").read_text())
    records = [json.loads(line) for line in (args.dir / "runs.jsonl").read_text().splitlines() if line.strip()]
    runs = (args.dir / "runs").resolve()
    for record in records:
        if not runs.is_relative_to(args.dir.resolve()):
            break
        ident = record.get("id")
        if (record.get("harness") != "codex" or record.get("shell") is not None
                or record.get("stopped") or record.get("exit_code") != 0
                or not isinstance(ident, str) or not ident or Path(ident).name != ident
                or ident in (".", "..")):
            continue
        path = (runs / ident / "agent.jsonl").resolve()
        if not path.is_relative_to(runs):
            continue
        try:
            record["shell"] = codex_shell_metrics(path.read_text().splitlines())
        except (OSError, UnicodeError):
            pass
    write_summary(args.dir, records, meta)
    sys.stdout.write((args.dir / "summary.md").read_text())
    return 0


def cmd_check_tasks(args: argparse.Namespace) -> int:
    """For each task: the untouched fixture fails its check, the reference
    solution passes it, and each probe gets the exit code it expects from
    the fake GitHub without a request the fake cannot answer."""
    names = use_task_dirs(args)
    if names is None:
        return 2
    unknown = [n for n in args.tasks or [] if n not in names]
    if unknown:
        log(f"unknown task: {', '.join(unknown)}")
        return 2
    failures = []
    for name in args.tasks or [n for n in names if not tasks.load(n).opt_in]:
        task = tasks.load(name)
        with tempfile.TemporaryDirectory(prefix="jouet-bench-check-") as tmp:
            root = Path(tmp)
            prepared = True
            for label, solve in (("unsolved", False), ("solved", True)):
                workdir = root / label
                try:
                    shas = tasks.prepare(task, workdir)
                except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
                    failures.append(f"{name}: preparation failed: {error}")
                    prepared = False
                    break
                # The reference solution and check use the same dependencies
                # that an agent sees, in a fresh environment for each arm.
                run_dir = root / f"{label}-run"
                base = arms.base_env(run_dir, None, "jdx/demo")
                task.agent_env(base, run_dir / "bin", workdir)
                try:
                    tasks.setup_environment(task, base, workdir, root / "python-cache", root / f"{label}-environment.log")
                except (OSError, RuntimeError, ValueError) as error:
                    env_log = root / f"{label}-environment.log"
                    details = env_log.read_text(errors="replace")[-4000:] if env_log.exists() else ""
                    failures.append(f"{name}: environment setup failed: {error}\n{details}")
                    continue
                fixture = task.github_fixture(shas)
                state = root / f"{label}-state.json"
                fake = FakeGitHub(fixture, offset_s=10_000) if fixture is not None else None
                if fake:
                    fake.__enter__()
                try:
                    if solve:
                        env = {**base, **tasks.GIT_ENV, **tasks.helpers_env()}
                        if fake:
                            env["BENCH_GITHUB_URL"] = fake.url
                        proc = subprocess.run(
                            ["python3", "-I", str(task.dir / "solve.py")],
                            cwd=workdir,
                            env=env,
                            capture_output=True,
                            text=True,
                        )
                        if proc.returncode != 0:
                            failures.append(f"{name}: solve.py failed: {proc.stderr.strip()}")
                            continue
                    if fake:
                        state.write_text(json.dumps(fake.world.fixture))
                finally:
                    if fake:
                        fake.__exit__()
                check = tasks.run_check(task, workdir, base, tasks.check_env(shas, state if fake else None))
                runner.sweep(run_dir, None)
                runner.sweep(workdir, None)
                if check["passed"] != solve:
                    want = "pass" if solve else "fail"
                    failures.append(f"{name}: the check should {want} when {label}:\n{check['output']}")
                else:
                    print(f"ok   {name}: check {'passes' if solve else 'fails'} when {label}")
            if prepared:
                failures += probe(task, root, args.jouet)
    for f in failures:
        print(f"FAIL {f}", file=sys.stderr)
    return 1 if failures else 0


def probe(task: tasks.Task, root: Path, jouet_bin: Path) -> list[str]:
    """Runs the task's probes: the commands an agent would use against its fake GitHub."""
    failures = []
    for n, pr in enumerate(task.probes):
        tool = pr["tool"]
        target = jouet_bin if tool == "jouet" else arms.resolve(tool)
        if target is None or not Path(target).exists():
            print(f"skip {task.name}: probe `{tool} {' '.join(pr['args'])}` ({tool} not found)")
            continue
        workdir = root / f"probe{n}"
        shas = tasks.prepare(task, workdir)
        fixture = task.github_fixture(shas)
        with FakeGitHub(fixture, offset_s=pr.get("at_s", 10_000)) as fake:
            env = {
                "PATH": os.environ["PATH"],
                "HOME": str(root / f"home{n}"),
                "XDG_STATE_HOME": str(root / f"home{n}" / "state"),
                "XDG_CACHE_HOME": str(root / f"home{n}" / "cache"),
                **(arms.gh_env(fake.url, "jdx/demo") if tool == "gh" else arms.jouet_env(fake.url)),
            }
            # `before` runs first against the same fake, as a reply before
            # a listing that must still read the thread.
            for args in [*pr.get("before", []), pr["args"]]:
                proc = subprocess.run(
                    [str(target), *args], cwd=workdir, env=env, capture_output=True, text=True, timeout=120
                )
                if proc.returncode != 0 and args is not pr["args"]:
                    break
            gaps = fake.world.summary()["unanswered_examples"]
        command = f"{tool} {' '.join(args)}"
        if proc.returncode != pr.get("exit", 0):
            failures.append(
                f"{task.name}: `{command}` exited {proc.returncode}, not {pr.get('exit', 0)}:\n"
                f"{proc.stdout[-1500:]}{proc.stderr[-1500:]}"
            )
        elif gaps:
            failures.append(f"{task.name}: `{command}` asked the fake GitHub for what it lacks: {gaps}")
        elif pr.get("stdout_contains") and pr["stdout_contains"] not in proc.stdout:
            failures.append(f"{task.name}: `{command}` printed no {pr['stdout_contains']!r}:\n{proc.stdout[-1500:]}")
        else:
            print(f"ok   {task.name}: `{command}` exits {proc.returncode}")
    return failures


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    extra_dirs = [*getattr(args, "tasks_dir", []), *(Path(p) for p in os.environ.get("JOUET_BENCH_TASKS", "").split(os.pathsep) if p)]
    if args.command in ("run", "check-tasks") and extra_dirs:
        try:
            checkouts = tasks.checkout_roots(runner.REPO_ROOT)
        except (OSError, RuntimeError, subprocess.CalledProcessError):
            print("cannot verify jouet worktree locations", file=sys.stderr)
            return 2
        def in_checkout(path: Path) -> bool:
            for root in checkouts:
                if path.is_relative_to(root):
                    return True
                # On case-insensitive filesystems an existing ancestor can be
                # the same directory even when its spelling differs.
                for ancestor in (path, *path.parents):
                    try:
                        if ancestor.samefile(root):
                            return True
                    except OSError:
                        continue
            return False

        if args.command == "run" and not args.dry_run and args.out is None:
            print("--tasks-dir requires an explicit --out for run results", file=sys.stderr)
            return 2
        if args.command == "run" and args.out is not None and in_checkout(args.out.expanduser().resolve()):
            print("--tasks-dir requires --out outside the jouet checkout", file=sys.stderr)
            return 2
        for task_dir in extra_dirs:
            if in_checkout(task_dir.expanduser().resolve()):
                print("external task directories must be outside the jouet checkout", file=sys.stderr)
                return 2
        temporary = Path(tempfile.gettempdir() if args.command == "check-tasks" else "/tmp").resolve()
        if in_checkout(temporary):
            print("--tasks-dir requires temporary directories outside the jouet checkout", file=sys.stderr)
            return 2
        if args.command == "run":
            cache = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache").expanduser().resolve()
            if in_checkout(cache):
                print("--tasks-dir requires XDG_CACHE_HOME outside the jouet checkout", file=sys.stderr)
                return 2
            init_selected = any(arms.ARMS.get(name) and arms.ARMS[name].init for name in args.arms)
            if init_selected and in_checkout(init.CACHE.resolve()):
                print("external tasks require the jouet-init cache outside the jouet checkout", file=sys.stderr)
                return 2
    return {"run": cmd_run, "report": cmd_report, "check-tasks": cmd_check_tasks,
            "prepare-codex": cmd_prepare_codex}[args.command](args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
