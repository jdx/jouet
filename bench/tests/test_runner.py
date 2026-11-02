import json
import sys
import tempfile
import unittest
from pathlib import Path

from jbench import arms, report, runner, stats, tasks
from jbench.harness import HARNESSES


def sample_wheel(root):
    import zipfile

    wheel = root / "bench_sample-1.0-py3-none-any.whl"
    with zipfile.ZipFile(wheel, "w") as archive:
        archive.writestr("bench_sample/__init__.py", "VALUE = 'installed'\n")
        archive.writestr("bench_sample-1.0.dist-info/METADATA", "Metadata-Version: 2.1\nName: bench-sample\nVersion: 1.0\n")
        archive.writestr("bench_sample-1.0.dist-info/WHEEL", "Wheel-Version: 1.0\nGenerator: test\nRoot-Is-Purelib: true\nTag: py3-none-any\n")
        archive.writestr("bench_sample-1.0.dist-info/RECORD", "")
    return wheel


class StatsTest(unittest.TestCase):
    def test_quantiles(self):
        self.assertEqual(stats.median([3, 1, 2]), 2)
        self.assertEqual(stats.median([1, 2, 3, 4]), 2.5)
        self.assertEqual(stats.quantile([1, 2, 3, 4, 5], 0.25), 2)

    def test_sign_test(self):
        self.assertEqual(stats.sign_test([-1] * 5), 0.0625)
        self.assertEqual(stats.sign_test([-1, -1, 1]), 1.0)
        self.assertIsNone(stats.sign_test([0, 0]))

    def test_paired_uses_rounds_both_arms_finished(self):
        p = stats.paired({1: 100, 2: 200, 3: 300}, {1: 90, 2: 150, 4: 10})
        self.assertEqual((p["pairs"], p["median_diff"], p["jouet_lower"]), (2, -30, 2))
        self.assertAlmostEqual(p["median_change"], -0.2)


class PlanTest(unittest.TestCase):
    def test_every_round_runs_every_arm_in_a_seeded_order(self):
        planned = runner.plan(["a", "b"], ["claude"], ["plain", "jouet"], 3, seed=7)
        self.assertEqual(len(planned), 12)
        for r in (1, 2, 3):
            in_round = [p for p in planned if p.round == r]
            self.assertEqual({(p.task, p.arm) for p in in_round}, {(t, a) for t in "ab" for a in ("plain", "jouet")})
        again = runner.plan(["a", "b"], ["claude"], ["plain", "jouet"], 3, seed=7)
        self.assertEqual([p.id for p in planned], [p.id for p in again])

    def test_failed_environment_skips_only_that_task(self):
        import os
        from unittest.mock import patch

        planned = [runner.Planned(1, "bad", "codex", "plain"),
                   runner.Planned(1, "bad", "codex", "jouet"),
                   runner.Planned(1, "good", "codex", "plain")]

        def execute(item, *_):
            if item.task == "bad":
                raise tasks.EnvironmentSetupError("missing dependency")
            return {"id": item.id, "task": item.task, "success": True, "usage": {}}

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            messages = []
            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root)}), \
                    patch.object(runner, "snapshot", return_value=Path(sys.executable)), \
                    patch.object(runner.costs_mod, "seeds", return_value={}), \
                    patch.object(runner, "execute", side_effect=execute):
                records = runner.run_suite(planned, {"jouet_bin": Path(sys.executable)}, root, messages.append)
        self.assertEqual([r["task"] for r in records], ["bad", "good"])
        self.assertIn("EnvironmentSetupError", records[0]["error"])
        self.assertTrue(records[1]["success"])
        self.assertTrue(any("environment setup already failed" in message for message in messages))


class StopTest(unittest.TestCase):
    """The cost controls, with a stand-in agent that prints Claude Code's events."""

    SCRIPT = (
        "import json, sys, time\n"
        "for n in range(int(sys.argv[1])):\n"
        "    usage = {'input_tokens': 600, 'output_tokens': 10}\n"
        "    print(json.dumps({'type': 'assistant', 'message': {'id': f'm{n}', 'usage': usage}}), flush=True)\n"
        "time.sleep(30)\n"
    )

    def run_agent(self, responses, timeout_s, max_tokens):
        with tempfile.TemporaryDirectory() as d:
            return runner.run_agent(
                HARNESSES["claude"],
                [sys.executable, "-c", self.SCRIPT, str(responses)],
                {"PATH": "/usr/bin:/bin"},
                Path(d),
                Path(d),
                timeout_s,
                max_tokens,
                Path(d) / "agent.jsonl",
            )

    def test_token_cap(self):
        result = self.run_agent(responses=3, timeout_s=20, max_tokens=1000)
        self.assertEqual(result["stopped"], "token_cap")
        self.assertLess(result["wall_s"], 15)

    def test_timeout(self):
        result = self.run_agent(responses=0, timeout_s=1, max_tokens=None)
        self.assertEqual(result["stopped"], "timeout")
        self.assertLess(result["wall_s"], 15)


class HarnessOutputTest(unittest.TestCase):
    def test_claude_stream(self):
        lines = [
            json.dumps({"type": "system", "subtype": "init", "model": "claude-x", "session_id": "s"}),
            json.dumps(
                {
                    "type": "assistant",
                    "message": {
                        "id": "m1",
                        "usage": {"input_tokens": 10, "cache_read_input_tokens": 100, "output_tokens": 5},
                    },
                }
            ),
            json.dumps(
                {
                    "type": "assistant",
                    "message": {
                        "id": "m1",
                        "usage": {"input_tokens": 10, "cache_read_input_tokens": 100, "output_tokens": 5},
                    },
                }
            ),
            json.dumps(
                {
                    "type": "assistant",
                    "message": {
                        "id": "m2",
                        "usage": {"input_tokens": 1, "cache_creation_input_tokens": 50, "output_tokens": 7},
                    },
                }
            ),
        ]
        claude = HARNESSES["claude"]
        self.assertEqual(claude.tokens_so_far(lines, Path("/nonexistent")), 173)
        killed = claude.usage(lines, Path("/nonexistent"))
        self.assertEqual((killed.input_tokens, killed.cached_input_tokens, killed.output_tokens), (161, 100, 12))
        self.assertTrue(killed.is_error)
        done = lines + [
            json.dumps(
                {
                    "type": "result",
                    "subtype": "success",
                    "is_error": False,
                    "num_turns": 3,
                    "total_cost_usd": 0.01,
                    "result": "done",
                    "usage": {
                        "input_tokens": 11,
                        "cache_read_input_tokens": 100,
                        "cache_creation_input_tokens": 50,
                        "output_tokens": 12,
                    },
                }
            )
        ]
        u = claude.usage(done, Path("/nonexistent"))
        self.assertEqual(
            (u.input_tokens, u.cache_write_tokens, u.turns, u.model, u.is_error), (161, 50, 3, "claude-x", False)
        )

    def test_claude_stream_resumed_after_a_background_command(self):
        def result(turns, cost, input_tokens, output_tokens, text):
            usage = {"input_tokens": input_tokens, "cache_read_input_tokens": 100, "output_tokens": output_tokens}
            return json.dumps(
                {
                    "type": "result",
                    "subtype": "success",
                    "is_error": False,
                    "num_turns": turns,
                    "total_cost_usd": cost,
                    "result": text,
                    "usage": usage,
                }
            )

        lines = [
            json.dumps({"type": "system", "subtype": "init", "model": "claude-x", "session_id": "s"}),
            result(10, 0.19, 18, 3270, "waiting for the build"),
            result(4, 0.23, 8, 1010, "done"),
        ]
        u = HARNESSES["claude"].usage(lines, Path("/nonexistent"))
        self.assertEqual(
            (u.input_tokens, u.output_tokens, u.turns, u.cost_usd, u.result), (226, 4280, 14, 0.23, "done")
        )

    def test_codex_stream(self):
        lines = [
            json.dumps({"type": "thread.started", "thread_id": "t"}),
            json.dumps({"type": "item.completed", "item": {"type": "agent_message", "text": "done"}}),
            json.dumps(
                {
                    "type": "turn.completed",
                    "usage": {"input_tokens": 1000, "cached_input_tokens": 800, "output_tokens": 20},
                }
            ),
        ]
        u = HARNESSES["codex"].usage(lines, Path("/nonexistent"))
        self.assertEqual(
            (u.input_tokens, u.cached_input_tokens, u.output_tokens, u.result, u.is_error),
            (1000, 800, 20, "done", False),
        )


class SweepTest(unittest.TestCase):
    """What a run leaves running is stopped, and nothing else is."""

    def start(self, cwd, *args, session=True):
        import subprocess

        proc = subprocess.Popen(
            [sys.executable, "-c", "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)", *args],
            cwd=cwd,
            start_new_session=session,
        )
        self.addCleanup(proc.kill)
        return proc

    def test_macos_discovery_reports_a_timeout(self):
        import subprocess
        from unittest.mock import patch

        with patch.object(runner.subprocess, "run", side_effect=subprocess.TimeoutExpired("lsof", 10)):
            found, error = runner.cwd_processes(Path("/tmp/run"))
        self.assertEqual(found, {})
        self.assertIn("process discovery failed", error)

    @unittest.skipUnless(sys.platform == "darwin", "needs macOS lsof")
    def test_macos_stops_detached_process_in_run_but_not_neighbor(self):
        with tempfile.TemporaryDirectory() as d:
            run_dir = Path(d) / "run\\name"
            other = Path(d) / "run\\name0"
            run_dir.mkdir()
            other.mkdir()
            inside = self.start(run_dir)
            outside = self.start(other)
            left = runner.sweep(run_dir, None)
            self.assertTrue(any(f"pid {inside.pid}:" in item for item in left), left)
            self.assertIsNotNone(inside.wait(timeout=10))
            self.assertIsNone(outside.poll())

    @unittest.skipUnless(Path("/proc").is_dir(), "needs /proc")
    def test_stops_processes_in_the_run_or_its_session(self):
        with tempfile.TemporaryDirectory() as d:
            run_dir = Path(d) / "task.claude.jouet.r1"
            other = Path(d) / "task.claude.jouet.r10"
            run_dir.mkdir()
            other.mkdir()
            # In its own session, as a test suite's watcher is, and deaf to SIGTERM.
            in_cwd = self.start(run_dir)
            in_args = self.start(d, f"--watch={run_dir}/work")
            leader = self.start(d)
            outside = self.start(other, str(other))
            left = runner.sweep(run_dir, leader.pid)
            self.assertEqual(len(left), 3, left)
            for proc in (in_cwd, in_args, leader):
                self.assertIsNotNone(proc.wait(timeout=10))
            self.assertIsNone(outside.poll())


class TasksTest(unittest.TestCase):
    def test_source_keeps_export_ignored_and_unnormalized_blobs(self):
        import dataclasses
        import subprocess

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source = root / "source"
            source.mkdir()
            tasks.git(source, "init", "-q", "-b", "main")
            (source / ".gitattributes").write_text("hidden.txt export-ignore\ncrlf.txt text eol=crlf\n")
            (source / "hidden.txt").write_text("included in the commit\n")
            (source / "crlf.txt").write_bytes(b"line\r\n")
            tasks.git(source, "add", ".gitattributes", "hidden.txt")
            blob = subprocess.run(["git", "hash-object", "-w", "--stdin"], cwd=source,
                                  input=b"line\r\n", capture_output=True, check=True).stdout.decode().strip()
            tasks.git(source, "update-index", "--add", "--cacheinfo", f"100644,{blob},crlf.txt")
            tasks.git(source, "commit", "-q", "-m", "source")
            revision = tasks.git(source, "rev-parse", "HEAD")
            task = dataclasses.replace(tasks.load("smoke"), source={"git": str(source), "commit": revision},
                                       setup=["true"])
            workdir = root / "work"
            tasks.prepare(task, workdir)
            self.assertEqual((workdir / "hidden.txt").read_text(), "included in the commit\n")
            self.assertEqual(tasks.git(workdir, "rev-parse", "HEAD^{tree}"), tasks.git(source, "rev-parse", "HEAD^{tree}"))
            modified = dataclasses.replace(task, setup=["sh -c 'printf altered > crlf.txt'"])
            with self.assertRaisesRegex(RuntimeError, "crlf.txt"):
                tasks.prepare(modified, root / "modified")

    def test_python_environment_is_isolated_and_available_to_the_run(self):
        import dataclasses
        import hashlib
        import os
        import subprocess

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task_dir = root / "task"
            task_dir.mkdir()
            wheel = sample_wheel(root)
            requirements = task_dir / "requirements.lock"
            requirements.write_text(f"bench-sample==1.0 --hash=sha256:{hashlib.sha256(wheel.read_bytes()).hexdigest()}\n")
            task = dataclasses.replace(tasks.load("smoke"), dir=task_dir, python_requirements="requirements.lock")
            home = root / "home"
            home.mkdir()
            workdir = root / "work"
            workdir.mkdir()
            env = {"HOME": str(home), "PATH": os.pathsep.join((str(Path(sys.executable).parent), os.defpath)),
                   "PIP_NO_INDEX": "1", "PIP_FIND_LINKS": str(root)}
            info = tasks.setup_environment(task, env, workdir, root / "cache", root / "setup.log")
            venv = home / ".venv"
            self.assertEqual(env["VIRTUAL_ENV"], str(venv))
            self.assertEqual(Path(env["PATH"].split(os.pathsep)[0]), venv / ("Scripts" if os.name == "nt" else "bin"))
            self.assertFalse((workdir / ".venv").exists())
            self.assertEqual(info["requirements_sha256"], hashlib.sha256(requirements.read_bytes()).hexdigest())
            result = subprocess.run(["python3", "-c", "import sys, bench_sample; print(sys.prefix); print(bench_sample.VALUE)"], env=env, capture_output=True, text=True, check=True)
            prefix, value = result.stdout.splitlines()
            self.assertEqual(Path(prefix).resolve(), venv.resolve())
            self.assertEqual(value, "installed")

            # A changed artifact must not silently enter another run.
            requirements.write_text("bench-sample==1.0 --hash=sha256:" + "0" * 64 + "\n")
            other_home = root / "other-home"
            other_home.mkdir()
            other_env = {**env, "HOME": str(other_home), "PATH": os.pathsep.join((str(Path(sys.executable).parent), os.defpath))}
            other_env.pop("VIRTUAL_ENV")
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "environment setup failed"):
                tasks.setup_environment(task, other_env, workdir, root / "cache", root / "bad-setup.log")
            self.assertIn("hash", (root / "bad-setup.log").read_text().lower())

    def test_reference_solution_and_check_use_task_dependencies(self):
        import argparse
        import contextlib
        import dataclasses
        import hashlib
        import io
        from unittest.mock import patch
        import agents

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task_dir = root / "task"
            (task_dir / "repo").mkdir(parents=True)
            (task_dir / "repo" / "initial").write_text("base\n")
            wheel = sample_wheel(root)
            digest = hashlib.sha256(wheel.read_bytes()).hexdigest()
            (task_dir / "requirements.lock").write_text(f"{wheel.as_uri()} --hash=sha256:{digest}\n")
            (task_dir / "solve.py").write_text("import bench_sample\nfrom pathlib import Path\nPath('answer').write_text(bench_sample.VALUE)\n")
            (task_dir / "check.py").write_text("import bench_sample\nfrom pathlib import Path\nassert Path('answer').read_text() == bench_sample.VALUE\n")
            task = dataclasses.replace(tasks.load("smoke"), dir=task_dir, python_requirements="requirements.lock")
            with patch.object(tasks, "load", return_value=task), contextlib.redirect_stdout(io.StringIO()):
                result = agents.cmd_check_tasks(argparse.Namespace(tasks=["sample"], jouet=root / "unused"))
            self.assertEqual(result, 0)

    def test_python_environment_rejects_requirements_outside_task(self):
        import dataclasses

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task_dir = root / "task"
            task_dir.mkdir()
            (root / "outside").write_text("")
            task = dataclasses.replace(tasks.load("smoke"), dir=task_dir, python_requirements="../outside")
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "inside the task directory"):
                tasks.setup_environment(task, {}, root, root / "cache", root / "setup.log")

    def test_source_preserves_and_initializes_submodules(self):
        import dataclasses
        import os
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d, patch.dict(os.environ, {"GIT_ALLOW_PROTOCOL": "file"}):
            root = Path(d)
            sub = root / "sub"
            sub.mkdir()
            tasks.git(sub, "init", "-q", "-b", "main")
            (sub / "fixture.txt").write_text("from submodule\n")
            tasks.git(sub, "add", "fixture.txt")
            tasks.git(sub, "commit", "-q", "-m", "submodule")
            source = root / "source"
            source.mkdir()
            tasks.git(source, "init", "-q", "-b", "main")
            tasks.git(source, "submodule", "add", "-q", str(sub), "vendor/sub")
            tasks.git(source, "commit", "-q", "-m", "source")
            revision = tasks.git(source, "rev-parse", "HEAD")
            task = dataclasses.replace(tasks.load("smoke"), source={"git": str(source), "commit": revision})
            workdir = root / "work"
            tasks.prepare(task, workdir)
            self.assertEqual((workdir / "vendor/sub/fixture.txt").read_text(), "from submodule\n")
            self.assertEqual(tasks.git(workdir, "rev-parse", "HEAD^{tree}"), tasks.git(source, "rev-parse", "HEAD^{tree}"))

    def test_the_check_runs_in_the_run_environment(self):
        import dataclasses
        import os

        with tempfile.TemporaryDirectory() as d:
            task_dir = Path(d) / "task"
            task_dir.mkdir()
            (task_dir / "check.py").write_text("import os\nprint(os.environ['HOME'], os.environ.get('BENCH_X'), os.environ.get('LEAK'))\n")
            task = dataclasses.replace(tasks.load("smoke"), dir=task_dir)
            os.environ["LEAK"] = "yes"
            self.addCleanup(os.environ.pop, "LEAK")
            check = tasks.run_check(task, Path(d), {"PATH": "/usr/bin:/bin", "HOME": f"{d}/home"}, {"BENCH_X": "1"})
            self.assertEqual(check["output"], f"{d}/home 1 None")

    def test_an_upstream_task_tracks_a_local_origin_at_the_base(self):
        import dataclasses

        with tempfile.TemporaryDirectory() as d:
            task = dataclasses.replace(tasks.load("smoke"), remote=False, upstream=True)
            workdir = Path(d) / "run" / "work"
            workdir.parent.mkdir()
            shas = tasks.prepare(task, workdir)
            self.assertEqual(tasks.git(workdir, "rev-parse", "--abbrev-ref", "@{upstream}"), "origin/main")
            self.assertEqual(tasks.git(workdir, "rev-parse", "origin/main"), shas["base_sha"])
            self.assertEqual(tasks.git(workdir, "symbolic-ref", "refs/remotes/origin/HEAD"), "refs/remotes/origin/main")
            self.assertTrue((Path(d) / "run" / "origin.git").is_dir())


    def test_setup_runs_in_the_work_tree_and_must_leave_it_clean(self):
        import dataclasses

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            smoke = tasks.load("smoke")
            ignored = dataclasses.replace(smoke, setup=["mkdir __pycache__", "touch __pycache__/x.pyc"])
            tasks.prepare(ignored, root / "ignored")
            self.assertTrue((root / "ignored" / "__pycache__" / "x.pyc").exists())
            dirty = dataclasses.replace(smoke, setup=["touch {workdir}/stray.txt"])
            with self.assertRaisesRegex(RuntimeError, "stray.txt"):
                tasks.prepare(dirty, root / "dirty")
            failing = dataclasses.replace(smoke, setup=["false"])
            with self.assertRaisesRegex(RuntimeError, "exited 1"):
                tasks.prepare(failing, root / "failing")

    def test_a_bare_tool_name_links_the_host_program(self):
        import dataclasses

        with tempfile.TemporaryDirectory() as d:
            bin_dir = Path(d) / "bin"
            bin_dir.mkdir()
            task = dataclasses.replace(tasks.load("smoke"), tools={"git": "git"})
            env = {"PATH": str(bin_dir)}
            task.agent_env(env, bin_dir, Path(d))
            self.assertEqual((bin_dir / "git").resolve(), arms.resolve("git"))
            missing = dataclasses.replace(task, tools={"nope": "no-such-program-here"})
            with self.assertRaisesRegex(RuntimeError, "no-such-program-here"):
                missing.agent_env(env, bin_dir, Path(d))

    def test_every_task_loads(self):
        names = tasks.all_names()
        self.assertIn("smoke", names)
        smoke = [n for n in names if tasks.load(n).smoke]
        self.assertEqual(smoke, ["smoke"])
        for name in names:
            task = tasks.load(name)
            self.assertTrue(task.prompt and task.description and task.chore, name)
            if task.has_github:
                fixture = task.github_fixture({"head_sha": "a" * 40, "base_sha": "b" * 40})
                self.assertNotIn("{{", json.dumps(fixture), name)
                self.assertNotIn("api.github.com", json.dumps(fixture), name)


class ReportTest(unittest.TestCase):
    def test_markdown(self):
        def record(arm, r, tokens, success=True):
            return {
                "task": "smoke",
                "harness": "claude",
                "arm": arm,
                "round": r,
                "success": success,
                "wall_s": 10.0,
                "usage": {
                    "input_tokens": tokens,
                    "cached_input_tokens": tokens // 2,
                    "output_tokens": 100,
                    "cost_usd": 0.01,
                },
                "audit": {"requests": 4, "calls": 3, "failed_calls": 0, "tool_output_bytes": 2048},
                "ledger": {"decisions": {"python-edit:deny": 1}} if arm == "jouet" else None,
            }

        records = [
            record("plain", 1, 1000),
            record("jouet", 1, 800),
            record("plain", 2, 1200),
            record("jouet", 2, 1300, False),
        ]
        summary = report.summarize(
            records, {"label": "Smoke test", "harnesses": {"claude": {"version": "1", "model": "m"}}}
        )
        text = report.markdown(summary)
        self.assertIn("> **Smoke test**", text)
        self.assertIn("| smoke | claude | 2/2 | 1/2 | -50 (-5%)", text)
        self.assertIn("`python-edit:deny` ×2", text)
        self.assertIn("Paired outcomes: 1 both passed; 1 plain only; 0 jouet only; 0 both failed.", text)
        self.assertIn("Both-pass pairs (1): median jouet minus plain: input -200", text)
        cell = summary["cells"][0]
        self.assertEqual(cell["paired"]["input_tokens"]["median_diff"], -50)
        self.assertEqual(cell["paired_when_both_pass"]["input_tokens"]["median_diff"], -200)

    def test_paired_outcomes_exclude_errors_and_unpaired_runs(self):
        def record(arm, round_number, success, **extra):
            return {"task": "t", "harness": "codex", "arm": arm, "round": round_number,
                    "success": success, "usage": {"input_tokens": 100, "cached_input_tokens": 50}, **extra}

        records = [
            record("plain", 1, True), record("jouet", 1, False),
            record("plain", 2, False), record("jouet", 2, True),
            record("plain", 3, False), record("jouet", 3, False),
            record("plain", 4, True), record("jouet", 4, False, error="auth"),
            record("plain", 5, True),
        ]
        cell = report.summarize(records, {})["cells"][0]
        self.assertEqual(cell["paired_outcomes"], {"both_passed": 0, "plain_only": 1, "jouet_only": 1, "both_failed": 1})
        self.assertIsNone(cell["paired_when_both_pass"]["input_tokens"])
        self.assertIn("Both-pass pairs (0): median jouet minus plain: no comparable metrics.", report.markdown({"meta": {}, "cells": [cell]}))


if __name__ == "__main__":
    unittest.main()


class CostSeedTest(unittest.TestCase):
    """A task's seed commands run once, through `jouet test --cmd`, and every
    run starts with the note they left."""

    def test_unreadable_requirements_do_not_abort_other_tasks(self):
        import dataclasses
        from unittest.mock import patch
        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "requirements.lock").write_text("# locked\n")
            bad = dataclasses.replace(tasks.load("smoke"), name="bad", dir=root,
                                      cost_seed=["python3 -V"], python_requirements="requirements.lock")
            good = dataclasses.replace(tasks.load("smoke"), name="good", cost_seed=[])
            planned = [runner.Planned(1, name, "codex", "plain") for name in ("bad", "good")]
            messages = []
            with patch.object(tasks, "load", side_effect=lambda name: {"bad": bad, "good": good}[name]), \
                    patch.object(Path, "read_bytes", side_effect=PermissionError("permission denied")):
                notes = costs.seeds(planned, Path("jouet"), root, messages.append)
            self.assertEqual(notes, {})
            self.assertEqual(len(messages), 1)
            self.assertIn("bad: cannot read Python requirements", messages[0])
            self.assertIn("trials will retry setup", messages[0])

    def test_seed_setup_failure_leaves_trials_to_retry(self):
        from unittest.mock import patch
        from jbench import costs

        planned = [runner.Planned(1, "broken", "codex", "plain")]
        messages = []
        with patch.object(costs.tasks_mod, "load", return_value=tasks.load("smoke")), \
                patch.object(costs, "seed", side_effect=tasks.EnvironmentSetupError("network unavailable")):
            notes = costs.seeds(planned, Path("jouet"), Path("work"), messages.append)
        self.assertEqual(notes, {})
        self.assertIn("trials will retry setup", messages[0])

    def test_seed_setup_failure_keeps_its_log_outside_the_work_tree(self):
        import dataclasses
        import os
        from unittest.mock import patch
        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task_dir = root / "task"
            (task_dir / "repo").mkdir(parents=True)
            (task_dir / "repo" / "a").write_text("a\n")
            (task_dir / "requirements.lock").write_text("# locked\n")
            task = dataclasses.replace(tasks.load("smoke"), dir=task_dir, cost_seed=["python3 -V"],
                                       python_requirements="requirements.lock")

            def fail_setup(_task, _env, _workdir, _cache, log):
                log.write_text("setup details\n")
                raise tasks.EnvironmentSetupError(f"setup failed (log: {log})")

            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root / "cache")}), \
                    patch.object(tasks, "setup_environment", side_effect=fail_setup):
                with self.assertRaises(tasks.EnvironmentSetupError):
                    costs.seed(task, Path(sys.executable), root / "work", lambda _: None, root / "results")
            self.assertEqual((root / "results" / "smoke.environment.log").read_text(), "setup details\n")

    def test_seed_times_the_commands_once_and_runs_start_with_the_note(self):
        import os

        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task_dir = root / "task"
            (task_dir / "repo").mkdir(parents=True)
            (task_dir / "repo" / "a.txt").write_text("a\n")
            task = tasks.Task(
                name="t",
                dir=task_dir,
                description="",
                prompt="",
                chore="",
                rules=[],
                timeout_s=60,
                cost_seed=["cargo build", "cargo test -p a"],
            )
            # A stand-in jouet that keeps a line naming each command it ran.
            calls = root / "calls"
            stand_in = root / "jouet"
            stand_in.write_text(
                "#!/bin/sh\n"
                f'echo "$*" >> {calls}\n'
                "shift 3\n"
                "line=$(printf '{\"cmd\":\"%s\"}' \"$*\")\n"
                "old=$(git notes --ref jouet-cost show HEAD 2>/dev/null)\n"
                'git notes --ref jouet-cost add -f -m "$(printf "%s\\n%s" "$old" "$line" | sed /^$/d)" HEAD\n'
            )
            stand_in.chmod(0o755)
            os.environ["XDG_CACHE_HOME"] = str(root / "cache")
            try:
                note = costs.seed(task, stand_in, root / "work", lambda _: None)
                again = costs.seed(task, stand_in, root / "work2", lambda _: None)
            finally:
                del os.environ["XDG_CACHE_HOME"]
            self.assertEqual(note, '{"cmd":"cargo build"}\n{"cmd":"cargo test -p a"}\n')
            self.assertEqual(again, note)
            # Timed once: the second call read the cache.
            self.assertEqual(
                calls.read_text().splitlines(), ["test --cmd -- cargo build", "test --cmd -- cargo test -p a"]
            )

            run = root / "run"
            shas = tasks.prepare(task, run)
            costs.apply(run, note, shas["head_sha"])
            self.assertEqual(tasks.git(run, "notes", "--ref", "jouet-cost", "show", "HEAD") + "\n", note)
