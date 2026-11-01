import json
import sys
import tempfile
import unittest
from pathlib import Path

from jbench import report, runner, stats, tasks
from jbench.harness import HARNESSES


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


if __name__ == "__main__":
    unittest.main()
