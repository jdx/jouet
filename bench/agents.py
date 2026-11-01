#!/usr/bin/env python3
"""Measure what jouet does for coding agents: the same tasks, with and without it.

    python3 bench/agents.py run --dry-run            # print the plan, spend nothing
    python3 bench/agents.py run --smoke              # one cheap task, once per arm
    python3 bench/agents.py run --harness claude --rounds 5
    python3 bench/agents.py report bench/results/<id>
    python3 bench/agents.py check-tasks              # no agents: prove each task's check

See docs/benchmarks.md for the method and its limits.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import UTC, datetime
from pathlib import Path

if sys.version_info < (3, 11):
    sys.exit("bench/agents.py needs Python 3.11 or newer (for tomllib)")
sys.path.insert(0, str(Path(__file__).resolve().parent))

from jbench import arms, report, runner, tasks  # noqa: E402
from jbench.fake_github import FakeGitHub  # noqa: E402
from jbench.harness import HARNESSES  # noqa: E402

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
    run.add_argument("--arms", type=csv, default=["plain", "jouet"], help="plain,jouet (default both)")
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
    chk.add_argument("--jouet", type=Path, default=runner.REPO_ROOT / "target" / "debug" / "jouet")
    return parser.parse_args(argv)


def git_commit() -> str | None:
    proc = subprocess.run(
        ["git", "rev-parse", "--short=12", "HEAD"], cwd=runner.REPO_ROOT, capture_output=True, text=True
    )
    return proc.stdout.strip() or None


def cmd_run(args: argparse.Namespace) -> int:
    names = tasks.all_names()
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
    if "jouet" in args.arms and not args.jouet.exists():
        problems.append(f"no jouet binary at {args.jouet}; run `mise run build` or pass --jouet")
    if len(planned) > args.max_runs:
        problems.append(f"{len(planned)} runs planned, over --max-runs {args.max_runs}")

    print(f"plan: {len(planned)} runs, seed {seed}")
    for name in selected:
        t = tasks.load(name)
        print(f"  task {name}: {t.chore}; rules {', '.join(t.rules) or '-'}; timeout {t.timeout_s}s")
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
        "jouet_version": runner.version([str(args.jouet), "--version"]),
        "effort": args.effort,
        "tasks": selected,
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
        "jouet_env": jouet_env,
        "harness_bins": bins,
        "keep_work": args.keep_work,
    }
    records = runner.run_suite(planned, opts, out, log)
    write_summary(out, records, meta)
    print(f"results: {out}")
    return 0


def write_summary(out: Path, records: list[dict], meta: dict) -> None:
    summary = report.summarize(records, meta)
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    (out / "summary.md").write_text(report.markdown(summary))


def cmd_report(args: argparse.Namespace) -> int:
    meta = json.loads((args.dir / "meta.json").read_text())
    records = [json.loads(line) for line in (args.dir / "runs.jsonl").read_text().splitlines() if line.strip()]
    write_summary(args.dir, records, meta)
    sys.stdout.write((args.dir / "summary.md").read_text())
    return 0


def cmd_check_tasks(args: argparse.Namespace) -> int:
    """For each task: the untouched fixture fails its check, the reference
    solution passes it, and each probe gets the exit code it expects from
    the fake GitHub without a request the fake cannot answer."""
    import tempfile

    failures = []
    for name in args.tasks or [n for n in tasks.all_names() if not tasks.load(n).opt_in]:
        task = tasks.load(name)
        with tempfile.TemporaryDirectory(prefix="jouet-bench-check-") as tmp:
            root = Path(tmp)
            for label, solve in (("unsolved", False), ("solved", True)):
                workdir = root / label
                shas = tasks.prepare(task, workdir)
                fixture = task.github_fixture(shas)
                state = root / f"{label}-state.json"
                fake = FakeGitHub(fixture, offset_s=10_000) if fixture is not None else None
                if fake:
                    fake.__enter__()
                try:
                    if solve:
                        env = {**os.environ, **tasks.GIT_ENV}
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
                # The environment a run's check gets, with no agent before it.
                run_dir = root / f"{label}-run"
                base = arms.base_env(run_dir, None, "jdx/demo")
                task.agent_env(base, run_dir / "bin", workdir)
                check = tasks.run_check(task, workdir, base, tasks.check_env(shas, state if fake else None))
                runner.sweep(run_dir, None)
                runner.sweep(workdir, None)
                if check["passed"] != solve:
                    want = "pass" if solve else "fail"
                    failures.append(f"{name}: the check should {want} when {label}:\n{check['output']}")
                else:
                    print(f"ok   {name}: check {'passes' if solve else 'fails'} when {label}")
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
    return {"run": cmd_run, "report": cmd_report, "check-tasks": cmd_check_tasks}[args.command](args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
