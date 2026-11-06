import json
import sys
import tempfile
import unittest
from pathlib import Path

from jbench import arms, init, report, runner, stats, tasks
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


class InitCacheTest(unittest.TestCase):
    def test_prepared_reuses_only_the_matching_setup(self):
        from dataclasses import replace
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "jouet"
            binary.write_text("binary")
            task = tasks.Task(name="example", dir=root, description="", prompt="", chore="",
                              rules=[], timeout_s=1)
            harness = HARNESSES["codex"]
            opts = {"jouet_bin": binary, "codex_model": "model", "effort": "medium"}
            cache = root / "cache"
            entry = cache / init.key(task, "base", binary, harness, opts)
            entry.mkdir(parents=True)
            (entry / "init.patch").write_text("saved patch")
            (entry / "meta.json").write_text("{}")
            with patch.object(init, "CACHE", cache), patch.object(init.tasks_mod, "prepare",
                                                                  side_effect=RuntimeError("cache miss")):
                cached = init.prepared(task, harness, {"head_sha": "base"}, opts, root, None)
                self.assertTrue(cached["cached"])
                self.assertEqual(cached["patch"].read_text(), "saved patch")
                with self.assertRaisesRegex(RuntimeError, "cache miss"):
                    init.prepared(replace(task, env={"MODE": "new"}), harness,
                                  {"head_sha": "base"}, opts, root, None)

    def test_cache_identity_changes_with_init_inputs(self):
        from dataclasses import replace
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "jouet"
            binary.write_text("binary")
            requirements = root / "requirements.txt"
            requirements.write_text("fixture==1\n")
            task = tasks.Task(name="example", dir=root, description="", prompt="", chore="",
                              rules=[], timeout_s=1, python_requirements="requirements.txt")
            harness = HARNESSES["codex"]
            opts = {"codex_model": "model", "effort": "medium"}
            original = init.key(task, "base", binary, harness, opts)
            self.assertEqual(original, init.key(task, "base", binary, harness, opts))
            with patch.object(init, "PROMPT", init.PROMPT + " different"):
                self.assertNotEqual(original, init.key(task, "base", binary, harness, opts))
            self.assertNotEqual(original, init.key(task, "base", binary, harness, opts | {"effort": "high"}))
            for changed in (replace(task, env={"MODE": "other"}), replace(task, path=["/other"]),
                            replace(task, setup=["prepare"]), replace(task, tools={"python3": "/other"}),
                            replace(task, isolated_setup=[["prepare"]])):
                with self.subTest(changed=changed):
                    self.assertNotEqual(original, init.key(changed, "base", binary, harness, opts))
            requirements.write_text("fixture==2\n")
            self.assertNotEqual(original, init.key(task, "base", binary, harness, opts))
            harness_binary = root / "codex"
            harness_binary.write_text("first")
            with_binary = opts | {"harness_bins": {"codex": harness_binary}}
            before_update = init.key(task, "base", binary, harness, with_binary)
            harness_binary.write_text("updated binary")
            self.assertNotEqual(before_update, init.key(task, "base", binary, harness, with_binary))


class StatsTest(unittest.TestCase):
    def test_shell_metrics_are_separate_from_tool_counts(self):
        def record(arm, shell):
            return {"task": "example", "harness": "codex", "arm": arm, "round": 1,
                    "success": True, "shell": shell}
        cell = report.summarize([record("plain", {"completed": 5, "failed": 1}),
                                 record("jouet", {"completed": 3, "failed": 0})], {})["cells"][0]
        self.assertEqual(cell["paired_when_both_pass"]["shell_commands"]["median_diff"], -2)
        self.assertEqual(cell["paired_when_both_pass"]["shell_failures"]["median_diff"], -1)
        self.assertIsNone(cell["paired_when_both_pass"]["calls"])

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
    def test_change_evidence_ignores_host_git_configuration(self):
        import os
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "repo"
            root.mkdir()
            tasks.git(root, "init", "-q", "-b", "main")
            (root / "tracked").write_text("initial\n")
            tasks.git(root, "add", "tracked")
            tasks.git(root, "commit", "-q", "-m", "initial")
            excludes = Path(d) / "excludes"
            excludes.write_text("agent.txt\n")
            config = Path(d) / "config"
            config.write_text(f"[core]\n\texcludesFile = {excludes}\n")
            xdg = Path(d) / "xdg"
            (xdg / "git").mkdir(parents=True)
            (xdg / "git" / "ignore").write_text("agent.txt\n")
            empty_xdg = Path(d) / "empty-xdg"
            empty_xdg.mkdir()
            common = {"GIT_CONFIG_GLOBAL": os.devnull, "XDG_CONFIG_HOME": str(empty_xdg),
                      "GIT_CONFIG_COUNT": "0"}
            cases = [
                {"GIT_CONFIG_GLOBAL": str(config)},
                {"XDG_CONFIG_HOME": str(xdg)},
                {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.excludesFile",
                 "GIT_CONFIG_VALUE_0": str(excludes)},
            ]
            for hostile in cases:
                with self.subTest(hostile=hostile), patch.dict(os.environ, common | hostile):
                    before = runner.worktree_snapshot(root)
                    (root / "agent.txt").write_text("new\n")
                    after = runner.worktree_snapshot(root)
                    self.assertEqual(runner.changed_paths(root, before, after), ["agent.txt"])
                    (root / "agent.txt").unlink()

    def test_change_evidence_compares_worktrees_without_touching_the_index(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            tasks.git(root, "init", "-q", "-b", "main")
            (root / ".gitignore").write_text("ignored\n")
            (root / "tracked").write_text("initial\n")
            (root / "deleted").write_text("initial\n")
            (root / "staged-only").write_text("initial\n")
            tasks.git(root, "add", ".gitignore", "tracked", "deleted", "staged-only")
            tasks.git(root, "commit", "-q", "-m", "initial")
            (root / "existing untracked").write_text("before\n")
            (root / "tracked").write_text("staged before\n")
            tasks.git(root, "add", "tracked")
            index_before = tasks.git(root, "diff", "--cached")
            before = runner.worktree_snapshot(root)

            (root / "tracked").write_text("agent change\n")
            (root / "deleted").unlink()
            (root / "new file").write_text("new\n")
            (root / "ignored").write_text("not evidence\n")
            (root / "staged-only").write_text("staged\n")
            tasks.git(root, "add", "staged-only")
            (root / "staged-only").write_text("initial\n")
            staged = tasks.git(root, "diff", "--cached")
            after = runner.worktree_snapshot(root)
            self.assertNotEqual(staged, index_before)
            self.assertEqual(tasks.git(root, "diff", "--cached"), staged)
            self.assertEqual(runner.changed_paths(root, before, after),
                             ["deleted", "new file", "staged-only", "tracked"])

            # Committed changes are just as visible, and a missing snapshot
            # must not be reported as a clean worktree.
            tasks.git(root, "add", "-A")
            tasks.git(root, "commit", "-q", "-m", "agent change")
            self.assertEqual(runner.changed_paths(root, before, runner.worktree_snapshot(root)),
                             ["deleted", "existing untracked", "new file", "tracked"])
            self.assertIsNone(runner.changed_paths(root, None, after))

    def test_change_evidence_excludes_checker_changes(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            home = root / "home"
            home.mkdir()
            env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": "/usr/bin:/bin"}
            opts = {"jouet_bin": Path(sys.executable), "harness_bins": {"codex": "codex"},
                    "codex_model": "test", "tmp_root": str(root), "keep_work": True}

            def agent(_harness, _cmd, _env, workdir, *_args):
                (workdir / "agent-only.txt").write_text("agent\n")
                return {"lines": [], "session": None, "stopped": None, "exit_code": 0, "wall_s": 1}

            def check(_task, workdir, *_args):
                (workdir / "checker-only.txt").write_text("checker\n")
                return {"passed": True, "exit_code": 0, "output": "ok"}

            def sweep(path, session):
                if session is not None:
                    (path / "work" / "cleanup-written.txt").write_text("child\n")
                return []

            def agent_with_session(*args):
                result = agent(*args)
                result["session"] = 123
                result["disk_free"] = {"work": {"min_bytes": 42}}
                return result

            with patch.object(runner.arms_mod, "build", return_value=env), \
                    patch.object(runner.arms_mod, "codex_tools", return_value=("test", None)), \
                    patch.object(runner.arms_mod, "verify_codex_path"), \
                    patch.object(runner.credentials, "lend", return_value=[]), \
                    patch.object(runner.credentials, "give_back", return_value=[]), \
                    patch.object(runner, "run_agent", side_effect=agent_with_session), \
                    patch.object(runner, "sweep", side_effect=sweep), \
                    patch.object(runner, "audit", return_value={}), \
                    patch.object(runner.tasks_mod, "run_check", side_effect=check):
                record = runner.execute(runner.Planned(1, "smoke", "codex", "plain"), opts, root, root)
            self.assertEqual(record["changed_paths"], ["agent-only.txt", "cleanup-written.txt"])
            self.assertEqual(record["disk_free"], {"work": {"min_bytes": 42}})

    def test_every_round_runs_every_arm_in_a_seeded_order(self):
        planned = runner.plan(["a", "b"], ["claude"], ["plain", "jouet"], 3, seed=7)
        self.assertEqual(len(planned), 12)
        for r in (1, 2, 3):
            in_round = [p for p in planned if p.round == r]
            self.assertEqual({(p.task, p.arm) for p in in_round}, {(t, a) for t in "ab" for a in ("plain", "jouet")})
        again = runner.plan(["a", "b"], ["claude"], ["plain", "jouet"], 3, seed=7)
        self.assertEqual([p.id for p in planned], [p.id for p in again])

    def test_hook_preflight_failure_stops_the_suite(self):
        import os
        from unittest.mock import patch

        planned = [runner.Planned(1, "first", "codex", "jouet"),
                   runner.Planned(1, "second", "codex", "plain")]
        failures = [runner.codex_hooks.HookPreflightError("untrusted"),
                    {"id": planned[0].id, "task": "first", "success": False,
                     "error_kind": "codex_hook_preflight", "usage": {"input_tokens": 12}}]
        for failure in failures:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as d:
                root = Path(d)
                messages = []
                with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root)}), \
                        patch.object(runner, "snapshot", return_value=Path(sys.executable)), \
                        patch.object(runner.costs_mod, "seeds", return_value={}), \
                        patch.object(runner, "execute", side_effect=[failure]) as execute:
                    records = runner.run_suite(planned, {"jouet_bin": Path(sys.executable)}, root, messages.append)
                self.assertEqual(len(records), 1)
                self.assertEqual(records[0]["error_kind"], "codex_hook_preflight")
                self.assertEqual(execute.call_count, 1)
                self.assertTrue(any("hooks could not be verified" in message for message in messages))

    def test_result_write_failure_preserves_previous_records_and_stops(self):
        import errno
        import os
        from unittest.mock import patch

        planned = [runner.Planned(1, "first", "codex", "plain"),
                   runner.Planned(1, "second", "codex", "plain"),
                   runner.Planned(1, "third", "codex", "plain")]
        first = {"id": planned[0].id, "task": "first", "success": True, "usage": {}}
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            replace = runner.os.replace
            work_root, tmp_root = root / "work-root", root / "tmp-root"
            work_root.mkdir()
            tmp_root.mkdir()

            def fail_second(source, target):
                if (root / "runs.jsonl").exists():
                    raise OSError(errno.ENOSPC, "No space left on device")
                replace(source, target)

            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root)}), \
                    patch.object(runner.tempfile, "mkdtemp", side_effect=[str(work_root), str(tmp_root)]), \
                    patch.object(runner, "snapshot", return_value=Path(sys.executable)), \
                    patch.object(runner.costs_mod, "seeds", return_value={}), \
                    patch.object(runner.os, "replace", side_effect=fail_second), \
                    patch.object(runner, "execute", side_effect=[first, OSError(errno.ENOSPC, "setup failed")]) as execute:
                with self.assertRaises(runner.ResultsWriteError) as raised:
                    runner.run_suite(planned, {"jouet_bin": Path(sys.executable)}, root, lambda _: None)
            self.assertIn(planned[1].id, str(raised.exception))
            self.assertIn("setup failed", str(raised.exception))
            self.assertIn("No space left on device", str(raised.exception))
            self.assertIn(str(work_root), str(raised.exception))
            self.assertIn(str(tmp_root), str(raised.exception))
            self.assertTrue(work_root.is_dir())
            self.assertTrue(tmp_root.is_dir())
            self.assertEqual(execute.call_count, 2)
            self.assertEqual([json.loads(line) for line in (root / "runs.jsonl").read_text().splitlines()], [first])
            self.assertEqual(list(root.glob(".runs-*")), [])

    def test_result_sync_failure_keeps_the_previous_file(self):
        import errno
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            path = root / "runs.jsonl"
            path.write_text('{"id":"earlier"}\n')
            with patch.object(runner.os, "fsync", side_effect=OSError(errno.ENOSPC, "sync failed")):
                with self.assertRaises(OSError):
                    runner.append_result(path, {"id": "new"})
            self.assertEqual(path.read_text(), '{"id":"earlier"}\n')
            self.assertEqual(list(root.glob(".runs-*")), [])

    def test_disk_guard_stops_the_suite_after_saving_the_record(self):
        import os
        from unittest.mock import patch

        planned = [runner.Planned(1, "first", "codex", "plain"),
                   runner.Planned(1, "second", "codex", "jouet")]
        record = {"id": planned[0].id, "task": "first", "success": False,
                  "error": "disk reserve", "error_kind": "disk_guard", "usage": {}}
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            messages = []
            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root)}), \
                    patch.object(runner, "snapshot", return_value=Path(sys.executable)), \
                    patch.object(runner.costs_mod, "seeds", return_value={}), \
                    patch.object(runner, "execute", return_value=record) as execute:
                records = runner.run_suite(planned, {"jouet_bin": Path(sys.executable)}, root, messages.append)
            self.assertEqual(records, [record])
            self.assertEqual(execute.call_count, 1)
            self.assertEqual(json.loads((root / "runs.jsonl").read_text())["error_kind"], "disk_guard")
            self.assertTrue(any("disk reserve reached" in message for message in messages))

    def test_missing_codex_usage_stops_the_suite(self):
        import os
        from unittest.mock import patch

        planned = [runner.Planned(1, "first", "codex", "plain"),
                   runner.Planned(1, "second", "codex", "jouet")]
        record = {"id": planned[0].id, "task": "first", "success": False,
                  "error_kind": "codex_usage_unavailable", "usage": {}}
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            messages = []
            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root)}), \
                    patch.object(runner, "snapshot", return_value=Path(sys.executable)), \
                    patch.object(runner.costs_mod, "seeds", return_value={}), \
                    patch.object(runner, "execute", return_value=record) as execute:
                records = runner.run_suite(planned, {"jouet_bin": Path(sys.executable)}, root, messages.append)
            self.assertEqual(len(records), 1)
            self.assertEqual(execute.call_count, 1)
            self.assertTrue(any("usage could not be measured" in message for message in messages))

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
                    patch.object(runner, "execute", side_effect=execute), \
                    patch.object(tasks, "index", return_value={"bad": root / "private" / "bad"}):
                records = runner.run_suite(planned, {"jouet_bin": Path(sys.executable)}, root, messages.append)
        self.assertEqual([r["task"] for r in records], ["bad", "good"])
        self.assertIn("EnvironmentSetupError", records[0]["error"])
        # An error record still says which task directory the task came from.
        self.assertEqual(records[0]["task_dir"], str(root / "private"))
        self.assertTrue(records[1]["success"])
        self.assertTrue(any("environment setup already failed" in message for message in messages))

    def test_snapshot_callback_sees_the_binary_the_suite_uses(self):
        import os
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            binary = root / "source"
            binary.write_bytes(b"original")
            seen = []

            def on_snapshot(copy):
                binary.write_bytes(b"rebuilt")
                seen.append(copy.read_bytes())

            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root)}), \
                    patch.object(runner.costs_mod, "seeds", return_value={}):
                runner.run_suite([], {"jouet_bin": binary}, root, lambda _: None, on_snapshot=on_snapshot)
            self.assertEqual(seen, [b"original"])

    def test_a_task_cannot_replace_a_codex_benchmark_tool(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            home = root / "home"
            home.mkdir()
            env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": "/usr/bin:/bin"}
            opts = {"jouet_bin": Path(sys.executable), "harness_bins": {"codex": "codex"},
                    "codex_model": "test", "tmp_root": str(root), "keep_work": True}
            with patch.object(runner.arms_mod, "build", return_value=env), \
                    patch.object(runner.arms_mod, "codex_tools", side_effect=[("old", None), ("new", None)]), \
                    patch.object(runner, "run_agent") as agent:
                with self.assertRaisesRegex(runner.codex_hooks.HookPreflightError, "replaced"):
                    runner.execute(runner.Planned(1, "smoke", "codex", "plain"), opts, root, root)
                agent.assert_not_called()

    def test_dependency_setup_cannot_replace_a_codex_benchmark_tool(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            home = root / "home"
            home.mkdir()
            env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": "/usr/bin:/bin"}
            opts = {"jouet_bin": Path(sys.executable), "harness_bins": {"codex": "codex"},
                    "codex_model": "test", "tmp_root": str(root), "keep_work": True}
            with patch.object(runner.arms_mod, "build", return_value=env), \
                    patch.object(runner.arms_mod, "codex_tools", side_effect=[("old", None), ("old", None), ("new", None)]), \
                    patch.object(runner.tasks_mod, "setup_environment", return_value=None) as setup, \
                    patch.object(runner, "run_agent") as agent:
                with self.assertRaisesRegex(runner.codex_hooks.HookPreflightError, "Setup replaced"):
                    runner.execute(runner.Planned(1, "smoke", "codex", "plain"), opts, root, root)
                setup.assert_called_once()
                agent.assert_not_called()

    def test_agent_exit_is_part_of_success(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            home = root / "home"
            home.mkdir()
            env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": "/usr/bin:/bin"}
            agent = {"lines": [], "session": None, "stopped": None, "exit_code": 1, "wall_s": 1}
            opts = {"jouet_bin": Path(sys.executable), "harness_bins": {"codex": "codex"},
                    "codex_model": "test", "tmp_root": str(root), "keep_work": True}
            with patch.object(runner.arms_mod, "build", return_value=env), \
                    patch.object(runner.arms_mod, "codex_tools", return_value=("test", None)), \
                    patch.object(runner.arms_mod, "verify_codex_path"), \
                    patch.object(runner.credentials, "lend", return_value=[]), \
                    patch.object(runner.credentials, "give_back", return_value=[]), \
                    patch.object(runner, "run_agent", return_value=agent), \
                    patch.object(runner, "sweep", return_value=[]), \
                    patch.object(runner, "audit", return_value={}), \
                    patch.object(runner.tasks_mod, "run_check", return_value={"passed": True, "exit_code": 0, "output": "ok"}):
                failed = runner.execute(runner.Planned(1, "smoke", "codex", "plain"), opts, root, root)
                self.assertTrue(failed["check"]["passed"])
                self.assertFalse(failed["success"])
                self.assertEqual(failed["exit_code"], 1)
                self.assertNotIn("error", failed)

                agent["exit_code"] = 0
                passed = runner.execute(runner.Planned(2, "smoke", "codex", "plain"), opts, root, root)
                self.assertTrue(passed["success"])

    def test_disk_guard_skips_checker_and_is_not_scored(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            home = root / "home"
            home.mkdir()
            env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": "/usr/bin:/bin"}
            agent = {"lines": [], "session": None, "stopped": "disk_guard", "exit_code": None, "wall_s": 0}
            opts = {"jouet_bin": Path(sys.executable), "harness_bins": {"codex": "codex"},
                    "codex_model": "test", "tmp_root": str(root), "keep_work": True}
            with patch.object(runner.arms_mod, "build", return_value=env), \
                    patch.object(runner.arms_mod, "codex_tools", return_value=("test", None)), \
                    patch.object(runner.arms_mod, "verify_codex_path"), \
                    patch.object(runner.credentials, "lend", return_value=[]), \
                    patch.object(runner.credentials, "give_back", return_value=[]), \
                    patch.object(runner, "run_agent", return_value=agent), \
                    patch.object(runner, "sweep", return_value=[]), \
                    patch.object(runner.tasks_mod, "run_check") as check:
                record = runner.execute(runner.Planned(1, "smoke", "codex", "plain"), opts, root, root)
            check.assert_not_called()
            self.assertEqual(record["error_kind"], "disk_guard")
            self.assertIsNone(record["check"]["passed"])
            self.assertFalse(report.scored(record))

    def test_capacity_error_skips_check_and_is_not_a_task_failure(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            home = root / "home"
            home.mkdir()
            env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": "/usr/bin:/bin"}
            event = json.dumps({"type": "turn.failed", "error": {
                "message": "Selected model is at capacity. Please try a different model."}})
            agent = {"lines": [event], "session": None, "stopped": None, "exit_code": 1, "wall_s": 1}
            opts = {"jouet_bin": Path(sys.executable), "harness_bins": {"codex": "codex"},
                    "codex_model": "test", "tmp_root": str(root), "keep_work": True}
            with patch.object(runner.arms_mod, "build", return_value=env), \
                    patch.object(runner.arms_mod, "codex_tools", return_value=("test", None)), \
                    patch.object(runner.arms_mod, "verify_codex_path"), \
                    patch.object(runner.credentials, "lend", return_value=[]), \
                    patch.object(runner.credentials, "give_back", return_value=[]), \
                    patch.object(runner, "run_agent", return_value=agent), \
                    patch.object(runner, "sweep", return_value=[]), \
                    patch.object(runner, "audit", return_value={}), \
                    patch.object(runner.tasks_mod, "run_check") as check:
                record = runner.execute(runner.Planned(1, "smoke", "codex", "plain"), opts, root, root)
                check.assert_not_called()
                self.assertIn("capacity", record["error"])
                self.assertFalse(record["success"])
                self.assertIsNone(record["check"]["exit_code"])
                self.assertTrue(Path(record["workdir"]).is_dir())
                self.assertNotIn("smoke", Path(record["workdir"]).parent.name)
                self.assertNotIn("smoke", Path(record["tmp_dir"]).name)

                # Capacity can interrupt a session after it has spent tokens
                # and run tools; the partial attempt is still not a task result.
                agent["lines"] = [json.dumps({"type": "item.completed", "item": {
                    "type": "command_execution", "command": "git status"}}), event]
                with patch.object(HARNESSES["codex"], "_session_usage", return_value=({"input_tokens": 123}, None)):
                    interrupted = runner.execute(runner.Planned(2, "smoke", "codex", "plain"), opts, root, root)
                check.assert_not_called()
                self.assertIn("capacity", interrupted["error"])
                self.assertEqual(interrupted["usage"]["input_tokens"], 123)
                self.assertNotEqual(record["workdir"], interrupted["workdir"])

                # An ephemeral capacity failure before usage is observable
                # retains the diagnosis but stops the suite on unknown cost.
                agent["lines"] = [json.dumps({"type": "jouet.ephemeral"}), event]
                no_usage = runner.execute(runner.Planned(4, "smoke", "codex", "plain"), opts, root, root)
                check.assert_not_called()
                self.assertIn("capacity", no_usage["error"])
                self.assertEqual(no_usage["error_kind"], "codex_usage_unavailable")

                # A stopped run keeps its timeout classification even if
                # the stream also reported capacity.
                agent["lines"] = [event]
                agent["stopped"] = "timeout"
                check.return_value = {"passed": True, "exit_code": 0, "output": "ok"}
                stopped = runner.execute(runner.Planned(3, "smoke", "codex", "plain"), opts, root, root)
                check.assert_called_once()
                self.assertNotIn("error", stopped)
                self.assertEqual(stopped["stopped"], "timeout")
                self.assertFalse(stopped["success"])
                agent["lines"] = [json.dumps({"type": "jouet.ephemeral"})]
                unknown = runner.execute(runner.Planned(5, "smoke", "codex", "plain"), opts, root, root)
                self.assertEqual(unknown["stopped"], "timeout")
                self.assertEqual(unknown["error_kind"], "codex_usage_unavailable")
                self.assertIsNone(unknown["check"]["exit_code"])


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

    def test_disk_reserve_refuses_start_and_stops_an_active_agent(self):
        from types import SimpleNamespace
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            args = (HARNESSES["claude"], [sys.executable, "-c", "import time; time.sleep(30)"],
                    {"PATH": "/usr/bin:/bin"}, root, root, 20, None, root / "agent.jsonl")
            with patch.object(runner.shutil, "disk_usage", return_value=SimpleNamespace(free=99)), \
                    patch.object(runner.subprocess, "Popen") as spawn:
                refused = runner.run_agent(*args, min_free_bytes=100)
            spawn.assert_not_called()
            self.assertEqual(refused["stopped"], "disk_guard")
            self.assertIsNone(refused["exit_code"])

            readings = [SimpleNamespace(free=200)] * 3 + [SimpleNamespace(free=99)] * 6
            with patch.object(runner.shutil, "disk_usage", side_effect=readings):
                stopped = runner.run_agent(*args, min_free_bytes=100)
            self.assertEqual(stopped["stopped"], "disk_guard")
            self.assertLess(stopped["wall_s"], 15)
            self.assertNotEqual(stopped["exit_code"], 0)

    def test_token_cap(self):
        result = self.run_agent(responses=3, timeout_s=20, max_tokens=1000)
        self.assertEqual(result["stopped"], "token_cap")
        self.assertLess(result["wall_s"], 15)

    def test_timeout(self):
        result = self.run_agent(responses=0, timeout_s=1, max_tokens=None)
        self.assertEqual(result["stopped"], "timeout")
        self.assertLess(result["wall_s"], 15)

    def test_stream_receipt_times_stay_aligned_without_copying_content(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            script = "print('private-first'); print('private-second')"
            result = runner.run_agent(HARNESSES["codex"], [sys.executable, "-c", script],
                                      {"PATH": "/usr/bin:/bin"}, root, root, 5, None,
                                      root / "agent.jsonl")
            self.assertEqual(result["lines"], ["private-first\n", "private-second\n"])
            raw = (root / "agent-timing.json").read_text()
            self.assertNotIn("private", raw)
            timing = json.loads(raw)
            self.assertTrue(timing["stream_complete"])
            self.assertEqual(len(timing["elapsed_s"]), 2)
            self.assertEqual(timing["elapsed_s"], sorted(timing["elapsed_s"]))
            self.assertEqual(timing["elapsed_s"], result["elapsed"])
            self.assertEqual(set(result["disk_free"]), {"work", "tmp", "results"})
            for space in result["disk_free"].values():
                self.assertGreaterEqual(space["samples"], 2)
                self.assertLessEqual(space["min_bytes"], space["start_bytes"])
                self.assertLessEqual(space["min_bytes"], space["end_bytes"])

    def test_disk_samples_resolve_relative_temp_directory_in_agent_workdir(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "relative-tmp").mkdir()
            result = runner.run_agent(HARNESSES["codex"], [sys.executable, "-c", "pass"],
                                      {"PATH": "/usr/bin:/bin", "TMPDIR": "relative-tmp"},
                                      root, root, 5, None, root / "agent.jsonl")
            self.assertEqual(result["disk_free"]["tmp"]["errors"], 0)

    def test_disk_samples_preserve_missing_readings(self):
        from types import SimpleNamespace
        from unittest.mock import patch

        samples = runner.DiskSamples({"work": Path("/example")})
        with patch.object(runner.shutil, "disk_usage", side_effect=[OSError("unavailable"),
                                                                   SimpleNamespace(free=30),
                                                                   SimpleNamespace(free=10),
                                                                   OSError("unavailable")]):
            for _ in range(4):
                samples.sample()
        self.assertEqual(samples.values["work"], {"start_bytes": None, "end_bytes": None,
                                                  "min_bytes": 10, "samples": 2, "errors": 2})

    def test_late_descendant_cannot_change_a_finalized_stream(self):
        import threading
        import time
        from unittest.mock import patch

        original_join = threading.Thread.join

        def short_join(thread, timeout=None):
            return original_join(thread, timeout=0.05)

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            child = "import time; time.sleep(0.5); print('late', flush=True)"
            script = f"import subprocess, sys; subprocess.Popen([sys.executable, '-c', {child!r}]); print('early', flush=True)"
            with patch.object(threading.Thread, "join", short_join):
                result = runner.run_agent(HARNESSES["codex"], [sys.executable, "-c", script],
                                          {"PATH": "/usr/bin:/bin"}, root, root, 5, None,
                                          root / "agent.jsonl")
            self.assertFalse(result["stream_complete"])
            self.assertEqual(result["lines"], ["early\n"])
            time.sleep(0.7)
            self.assertEqual((root / "agent.jsonl").read_text(), "early\n")
            timing = json.loads((root / "agent-timing.json").read_text())
            self.assertEqual(len(timing["elapsed_s"]), 1)
            self.assertFalse(timing["stream_complete"])


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

    def test_codex_capacity_failure_is_distinguished_from_a_failed_task(self):
        lines = [json.dumps({"type": "turn.failed", "error": {
            "message": "Selected model is at capacity. Please try a different model."}})]
        u = HARNESSES["codex"].usage(lines, Path("/nonexistent"))
        self.assertTrue(u.is_error)
        self.assertTrue(u.extra["capacity_error"])
        self.assertEqual(u.input_tokens, 0)
        after_work = [json.dumps({"type": "item.completed", "item": {
            "type": "command_execution", "command": "git status"}}), *lines]
        u = HARNESSES["codex"].usage(after_work, Path("/nonexistent"))
        self.assertTrue(u.extra["capacity_error"])
        recovered = [*lines, json.dumps({"type": "turn.completed", "usage": {}})]
        u = HARNESSES["codex"].usage(recovered, Path("/nonexistent"))
        self.assertFalse(u.extra["capacity_error"])


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
    def test_history_configuration_requires_a_local_source_and_no_forge_remote(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            directory = Path(d) / "task"
            directory.mkdir()
            (directory / "check.py").touch()
            (directory / "solve.py").touch()
            base = 'description = "test"\nprompt = "test"\nchore = "bug-fix"\n'
            with patch.object(tasks, "TASKS_DIR", Path(d)):
                cases = [
                    ('[source]\ngit = ""\ncommit = "abc"\n', "nonempty git"),
                    ('[source]\ngit = "/tmp/source"\ncommit = "abc"\nhistory = "yes"\n', "boolean"),
                    ('[source]\ngit = "/tmp/source"\ncommit = "abc"\nhistory = true\n', "remote = false"),
                ]
                for config, message in cases:
                    (directory / "task.toml").write_text(base + config)
                    with self.subTest(config=config), self.assertRaisesRegex(ValueError, message):
                        tasks.load("task")

    def test_rubocop_rejects_hidden_checkout_paths_before_running(self):
        import subprocess

        command = tasks.load("rubocop-redundant-self").isolated_setup[0]
        def check(path):
            return subprocess.run([sys.executable, *command[1:-1], path], capture_output=True, text=True, check=False)

        self.assertEqual(check("/work/benchmark/repo").returncode, 0)
        hidden = check("/work/.cache/benchmark/repo")
        self.assertNotEqual(hidden.returncode, 0)
        self.assertIn("XDG_CACHE_HOME", hidden.stderr)

    def test_isolated_setup_rejects_shell_strings_and_empty_commands(self):
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            directory = Path(d) / "bad"
            (directory / "repo").mkdir(parents=True)
            (directory / "check.py").touch()
            (directory / "solve.py").touch()
            base = 'description = "test"\nprompt = "test"\nchore = "bug-fix"\n'
            with patch.object(tasks, "TASKS_DIR", Path(d)):
                for value in ('["echo hello"]', '[[]]', '[["echo", ""]]'):
                    (directory / "task.toml").write_text(base + f"isolated_setup = {value}\n")
                    with self.subTest(value=value), self.assertRaisesRegex(ValueError, "argument lists"):
                        tasks.load("bad")

    def test_isolated_setup_uses_run_environment_and_refuses_checkout_changes(self):
        import dataclasses
        import os
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            work = root / "work"
            home = root / "home"
            work.mkdir()
            home.mkdir()
            tasks.git(work, "init", "-q", "-b", "main")
            (work / "tracked").write_text("base\n")
            tasks.git(work, "add", "tracked")
            tasks.git(work, "commit", "-q", "-m", "base")
            env = {"HOME": str(home), "PATH": os.defpath}
            script = "import os,pathlib,sys; pathlib.Path(os.environ['HOME'], 'prepared').write_text(sys.argv[1])"
            task = dataclasses.replace(tasks.load("smoke"), isolated_setup=[[sys.executable, "-c", script, "a; b"]])
            result = tasks.setup_environment(task, env, work, root / "cache", root / "setup.log")
            self.assertEqual(result, {"setup_commands": "1"})
            self.assertEqual((home / "prepared").read_text(), "a; b")

            task = dataclasses.replace(task, dir=root, isolated_setup=[[sys.executable, "-c", script, "{taskdir}"]])
            tasks.setup_environment(task, env, work, root / "cache", root / "setup.log")
            self.assertEqual((home / "prepared").read_text(), str(root.resolve()))

            task = dataclasses.replace(task, isolated_setup=[[sys.executable, "-c", "raise SystemExit(7)"]])
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "isolated setup failed"):
                tasks.setup_environment(task, env, work, root / "cache", root / "setup.log")

            (work / "existing").write_text("before\n")
            task = dataclasses.replace(task, isolated_setup=[[sys.executable, "-c", "open('existing', 'w').write('after')"]])
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "changed files"):
                tasks.setup_environment(task, env, work, root / "cache", root / "setup.log")

            task = dataclasses.replace(task, isolated_setup=[[sys.executable, "-c", "open('new-file', 'w').close()"]])
            excludes = root / "host-ignore"
            excludes.write_text("new-file\n")
            hostile = {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.excludesFile",
                       "GIT_CONFIG_VALUE_0": str(excludes)}
            with patch.dict(os.environ, hostile), self.assertRaisesRegex(tasks.EnvironmentSetupError, "changed files"):
                tasks.setup_environment(task, env, work, root / "cache", root / "setup.log")

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
            self.assertFalse((workdir / ".git" / "FETCH_HEAD").exists())
            self.assertEqual((workdir / "hidden.txt").read_text(), "included in the commit\n")
            self.assertEqual(tasks.git(workdir, "rev-parse", "HEAD^{tree}"), tasks.git(source, "rev-parse", "HEAD^{tree}"))
            modified = dataclasses.replace(task, setup=["sh -c 'printf altered > crlf.txt'"])
            with self.assertRaisesRegex(RuntimeError, "crlf.txt"):
                tasks.prepare(modified, root / "modified")

    def test_history_source_keeps_only_the_pinned_ancestry_and_local_upstream(self):
        import dataclasses
        import os
        import subprocess
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source = root / "source"
            source.mkdir()
            tasks.git(source, "init", "-q", "-b", "main")
            for value in ("first", "pinned", "later"):
                (source / "file").write_text(value)
                tasks.git(source, "add", "file")
                tasks.git(source, "commit", "-q", "-m", value)
                if value == "pinned":
                    pinned = tasks.git(source, "rev-parse", "HEAD")
            later = tasks.git(source, "rev-parse", "HEAD")
            task = dataclasses.replace(tasks.load("smoke"),
                source={"git": str(source), "commit": pinned, "history": True}, remote=False, upstream=True)
            work = root / "work"
            with patch.dict(os.environ, {"GIT_DIR": str(source / ".git")}):
                shas = tasks.prepare(task, work)
            self.assertEqual(shas["base_sha"], pinned)
            self.assertEqual(tasks.git(work, "log", "--format=%s"), "pinned\nfirst")
            missing = subprocess.run(["git", "cat-file", "-e", f"{later}^{{commit}}"], cwd=work, capture_output=True, check=False)
            self.assertNotEqual(missing.returncode, 0)
            self.assertEqual((work / "file").read_text(), "pinned")
            self.assertEqual(tasks.git(work, "remote", "get-url", "origin"), str(root / "origin.git"))
            self.assertNotIn(str(source.resolve()), (work / ".git" / "FETCH_HEAD").read_text())
            self.assertEqual(tasks.git(work, "remote", "get-url", "--push", "origin"), os.devnull)
            pushed = subprocess.run(["git", "push", "origin", "HEAD:refs/heads/other"], cwd=work, capture_output=True, check=False)
            self.assertNotEqual(pushed.returncode, 0)
            self.assertEqual(tasks.git(source, "log", "-1", "--format=%s"), "later")
            no_upstream = dataclasses.replace(task, upstream=False)
            no_origin = root / "no-origin"
            tasks.prepare(no_upstream, no_origin)
            self.assertFalse((no_origin / ".git" / "FETCH_HEAD").exists())

    def test_history_source_rejects_partial_and_shallow_clones(self):
        import dataclasses
        import subprocess

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source = root / "source"
            source.mkdir()
            tasks.git(source, "init", "-q", "-b", "main")
            (source / "file").write_text("base")
            tasks.git(source, "add", "file")
            tasks.git(source, "commit", "-q", "-m", "base")
            revision = tasks.git(source, "rev-parse", "HEAD")
            shallow = root / "shallow"
            subprocess.run(["git", "clone", "--quiet", "--depth=1", source.as_uri(), str(shallow)], check=True)
            task = dataclasses.replace(tasks.load("smoke"), remote=False,
                source={"git": str(shallow), "commit": revision, "history": True})
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "non-shallow"):
                tasks.prepare(task, root / "shallow-work")
            task.source["git"] = str(source)
            task.source["commit"] = "main"
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "full pinned commit"):
                tasks.prepare(task, root / "moving-ref-work")
            tasks.git(source, "config", "remote.origin.promisor", "true")
            task.source["commit"] = revision
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "partial clone"):
                tasks.prepare(task, root / "partial-work")

    def test_history_source_refuses_network_remotes_and_submodules(self):
        import dataclasses

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source = root / "source"
            source.mkdir()
            tasks.git(source, "init", "-q", "-b", "main")
            (source / ".gitmodules").write_text('[submodule "child"]\npath = child\nurl = https://example.invalid/child\n')
            tasks.git(source, "add", ".gitmodules")
            tasks.git(source, "commit", "-q", "-m", "base")
            revision = tasks.git(source, "rev-parse", "HEAD")
            task = dataclasses.replace(tasks.load("smoke"), remote=True,
                source={"git": str(source), "commit": revision, "history": True})
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "remote = false"):
                tasks.prepare(task, root / "remote-work")
            task.remote = False
            with self.assertRaisesRegex(tasks.EnvironmentSetupError, "does not fetch submodules"):
                tasks.prepare(task, root / "submodule-work")

    def test_external_tasks_require_an_explicit_outside_result_directory(self):
        import contextlib
        import io
        import os
        from unittest.mock import patch

        import agents

        with tempfile.TemporaryDirectory() as d, patch.object(tasks, "TASKS_DIR", tasks.TASKS_DIR):
            directory = Path(d)
            other_checkout = directory / "another worktree"
            other_checkout.mkdir()
            (directory / "local-task").mkdir()
            (directory / "local-task" / "task.toml").write_text('description = "local"\n')
            with contextlib.redirect_stderr(io.StringIO()), patch.object(agents, "cmd_run", return_value=0) as run, \
                 patch.object(tasks, "checkout_roots", return_value=[runner.REPO_ROOT, other_checkout.resolve()]):
                self.assertEqual(agents.main(["run", "--tasks-dir", str(directory)]), 2)
                self.assertEqual(agents.main(["run", "--tasks-dir", str(directory), "--out", str(runner.REPO_ROOT / "results")]), 2)
                self.assertEqual(agents.main(["run", "--tasks-dir", str(directory), "--out", str(other_checkout / "results")]), 2)
                other_spelling = other_checkout.with_name(other_checkout.name.upper())
                if other_spelling.exists() and other_spelling != other_checkout:
                    self.assertEqual(agents.main(["run", "--tasks-dir", str(directory), "--out", str(other_spelling / "results")]), 2)
                with patch.dict(os.environ, {"XDG_CACHE_HOME": str(runner.REPO_ROOT / "cache")}):
                    self.assertEqual(agents.main(["run", "--tasks-dir", str(directory), "--out", str(directory / "results")]), 2)
                with patch.object(agents.init, "CACHE", other_checkout / "init-cache"):
                    self.assertEqual(
                        agents.main(["run", "--tasks-dir", str(directory), "--arms", "jouet-init",
                                     "--out", str(directory / "results")]), 2,
                    )
                with patch.dict(os.environ, {"JOUET_BENCH_TASKS": str(directory)}):
                    self.assertEqual(agents.main(["run"]), 2)
                    self.assertEqual(agents.main(["run", "--out", str(other_checkout / "results")]), 2)
                self.assertEqual(agents.main(["run", "--tasks-dir", str(directory), "--tasks-dir", str(other_checkout), "--out", str(directory / "results")]), 2)
                with patch.object(agents.tempfile, "gettempdir", return_value=str(other_checkout)):
                    self.assertEqual(agents.main(["check-tasks", "--tasks-dir", str(directory)]), 2)
                self.assertEqual(agents.main(["run", "--tasks-dir", str(directory), "--out", str(directory / "results")]), 0)
                run.assert_called_once()
                self.assertEqual(run.call_args.args[0].tasks_dir, [directory])

    def test_ghost_checker_does_not_count_a_deleted_test(self):
        import os
        import subprocess

        checker = tasks.TASKS_DIR / "ghost-tier-refresh" / "check.py"
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "repo"
            root.mkdir()
            source = root / "apps/admin/src/automations"
            source.mkdir(parents=True)
            (source / "code.ts").write_text("before\n")
            (source / "old.test.ts").write_text("old test\n")
            tasks.git(root, "init", "-q", "-b", "main")
            tasks.git(root, "add", "apps")
            tasks.git(root, "commit", "-q", "-m", "base")
            base = tasks.git(root, "rev-parse", "HEAD")
            (source / "code.ts").write_text("fixed\n")
            tasks.git(root, "rm", "-q", "apps/admin/src/automations/old.test.ts")
            tasks.git(root, "add", "apps/admin/src/automations/code.ts")
            tasks.git(root, "commit", "-q", "-m", "delete test")
            bin_dir = Path(d) / "bin"
            bin_dir.mkdir()
            pnpm = bin_dir / "pnpm"
            marker = Path(d) / "pnpm-ran"
            pnpm.write_text('#!/bin/sh\nprintf "called" > "$PNPM_STUB_MARKER"\n')
            pnpm.chmod(0o755)
            env = tasks.git_env() | {"BENCH_BASE_SHA": base, "PNPM_STUB_MARKER": str(marker),
                                     "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}"}

            def check():
                return subprocess.run([sys.executable, str(checker)], cwd=root, env=env,
                                      capture_output=True, text=True, check=False)

            deleted = check()
            self.assertNotEqual(deleted.returncode, 0)
            self.assertIn("no committed regression test", deleted.stderr)
            self.assertEqual(marker.read_text(), "called")
            (source / "new.test.ts").write_text("new test\n")
            tasks.git(root, "add", "apps/admin/src/automations/new.test.ts")
            tasks.git(root, "commit", "-q", "-m", "add test")
            added = check()
            self.assertEqual(added.returncode, 0, added.stderr)

    def test_checkout_roots_include_the_current_worktree(self):
        self.assertIn(runner.REPO_ROOT, tasks.checkout_roots(runner.REPO_ROOT))

    def test_report_does_not_inspect_external_task_directories(self):
        from unittest.mock import patch

        import agents

        with patch.dict("os.environ", {"JOUET_BENCH_TASKS": "/missing-tasks"}), \
             patch.object(tasks, "checkout_roots", side_effect=AssertionError("not needed")), \
             patch.object(agents, "cmd_report", return_value=0) as report:
            self.assertEqual(agents.main(["report", "/results"]), 0)
            report.assert_called_once()

    def test_check_tasks_reports_history_preparation_errors(self):
        import subprocess

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task = root / "task"
            task.mkdir()
            (task / "task.toml").write_text(
                'description = "test"\nprompt = "test"\nchore = "bug-fix"\nremote = false\n'
                '[source]\ngit = "/missing-source"\ncommit = "' + "a" * 40 + '"\nhistory = true\n'
            )
            (task / "check.py").touch()
            (task / "solve.py").touch()
            result = subprocess.run([sys.executable, str(runner.REPO_ROOT / "bench/agents.py"),
                                     "check-tasks", "--tasks", "task", "--tasks-dir", str(root)], capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 1)
            self.assertIn("preparation failed", result.stderr)
            self.assertNotIn("Traceback", result.stderr)

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
            with patch.object(tasks, "load", return_value=task), patch.object(tasks, "all_names", return_value=["sample"]), \
                    contextlib.redirect_stdout(io.StringIO()):
                result = agents.cmd_check_tasks(argparse.Namespace(tasks=["sample"], tasks_dir=[], jouet=root / "unused"))
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

    def test_relative_submodules_use_explicit_base_only_during_setup(self):
        import dataclasses
        import os
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d, patch.dict(os.environ, {"GIT_ALLOW_PROTOCOL": "file"}):
            root = Path(d)
            sub = root / "sub"
            nested = root / "nested"
            source = root / "source"
            sub.mkdir()
            nested.mkdir()
            source.mkdir()
            tasks.git(nested, "init", "-q", "-b", "main")
            (nested / "nested.txt").write_text("nested\n")
            tasks.git(nested, "add", "nested.txt")
            tasks.git(nested, "commit", "-q", "-m", "nested")
            tasks.git(sub, "init", "-q", "-b", "main")
            (sub / "fixture.txt").write_text("from submodule\n")
            tasks.git(sub, "add", "fixture.txt")
            tasks.git(sub, "submodule", "add", "-q", "../nested", "vendor/nested")
            tasks.git(sub, "commit", "-q", "-m", "submodule")
            tasks.git(source, "init", "-q", "-b", "main")
            tasks.git(source, "submodule", "add", "-q", "../sub", "vendor/sub")
            tasks.git(source, "commit", "-q", "-m", "source")
            revision = tasks.git(source, "rev-parse", "HEAD")
            task = dataclasses.replace(tasks.load("smoke"), remote=False, upstream=True,
                source={"git": str(source), "commit": revision, "submodule_base_url": str(source)})
            work = root / "work"
            tasks.prepare(task, work)
            self.assertEqual((work / "vendor/sub/fixture.txt").read_text(), "from submodule\n")
            self.assertEqual(tasks.git(work, "rev-parse", "HEAD^{tree}"), tasks.git(source, "rev-parse", "HEAD^{tree}"))
            self.assertEqual((work / "vendor/sub/vendor/nested/nested.txt").read_text(), "nested\n")
            for repo in (work, work / "vendor/sub", work / "vendor/sub/vendor/nested"):
                config = Path(tasks.git(repo, "rev-parse", "--git-path", "config"))
                self.assertNotIn(str(root / "source"), (repo / config).read_text())
                self.assertNotIn(str(root / "sub"), (repo / config).read_text())
                self.assertNotIn(str(root / "nested"), (repo / config).read_text())
                if repo != work:
                    self.assertEqual(tasks.git(repo, "remote"), "")
                    fetch_head = Path(tasks.git(repo, "rev-parse", "--git-path", "FETCH_HEAD"))
                    self.assertFalse((repo / fetch_head).exists())
                    reflog = Path(tasks.git(repo, "rev-parse", "--git-path", "logs/HEAD"))
                    if (repo / reflog).exists():
                        self.assertNotIn(str(root), (repo / reflog).read_text())
            self.assertEqual(tasks.git(work, "remote", "get-url", "origin"), str(root / "origin.git"))

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

    def test_unverified_check_requires_marker_and_preserves_stderr(self):
        import dataclasses

        with tempfile.TemporaryDirectory() as d:
            task_dir = Path(d) / "task"
            task_dir.mkdir()
            task = dataclasses.replace(tasks.load("smoke"), dir=task_dir)
            script = task_dir / "check.py"
            base = {"PATH": "/usr/bin:/bin", "HOME": d}
            script.write_text("import sys\nprint('missing test')\nprint('JOUET_BENCH_UNVERIFIED_V1')\n"
                              "print('warning', file=sys.stderr)\nsys.exit(3)\n")
            result = tasks.run_check(task, Path(d), base, {})
            self.assertIsNone(result["passed"])
            self.assertIn("missing test", result["output"])
            self.assertIn("warning", result["output"])
            script.write_text("import sys\nsys.exit(3)\n")
            self.assertIs(tasks.run_check(task, Path(d), base, {})["passed"], False)

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

    def test_an_upstream_url_is_origins_url_and_resolves_to_the_local_repository(self):
        import dataclasses

        url = "https://git.example.invalid/owner/repo.git"
        with tempfile.TemporaryDirectory() as d:
            task = dataclasses.replace(tasks.load("smoke"), remote=False, upstream=True, upstream_url=url)
            workdir = Path(d) / "run" / "work"
            workdir.parent.mkdir()
            shas = tasks.prepare(task, workdir)
            self.assertEqual(tasks.git(workdir, "config", "--get", "remote.origin.url"), url)
            self.assertEqual(tasks.git(workdir, "rev-parse", "origin/main"), shas["base_sha"])
            # A push goes to the local repository, not the host.
            tasks.git(workdir, "push", "-q", "origin", "HEAD:refs/heads/check")
            self.assertEqual(tasks.git(Path(d) / "run" / "origin.git", "rev-parse", "check"), shas["head_sha"])


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

    def test_a_task_tool_runs_from_its_original_installation(self):
        import dataclasses
        import os
        import subprocess
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            source = root / "install"
            source.mkdir()
            (source / "marker").write_text("original installation\n")
            tool = source / "tool"
            tool.write_text('#!/bin/sh\ncat "$(dirname "$0")/marker"\nprintf "%s\\n" "$1"\n')
            tool.chmod(0o755)
            task = dataclasses.replace(tasks.load("smoke"), tools={"tool": "tool"})
            env = {"PATH": os.pathsep.join((str(bin_dir), os.defpath))}
            with patch.dict(os.environ, {"PATH": os.pathsep.join((str(source), os.defpath))}):
                task.agent_env(env, bin_dir, root)
            result = subprocess.run([str(bin_dir / "tool"), "an argument"], env=env,
                                    capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout, "original installation\nan argument\n")
            missing = dataclasses.replace(task, tools={"nope": "no-such-program-here"})
            with self.assertRaisesRegex(RuntimeError, "no-such-program-here"):
                missing.agent_env(env, bin_dir, root)

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


class ExternalTasksTest(unittest.TestCase):
    """Tasks in directories outside bench/tasks/, from --tasks-dir or JOUET_BENCH_TASKS."""

    def setUp(self):
        from unittest.mock import patch

        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        env = patch.dict("os.environ")
        env.start()
        self.addCleanup(env.stop)
        import os

        os.environ.pop("JOUET_BENCH_TASKS", None)
        self.addCleanup(tasks.EXTRA_DIRS.clear)

    def task(self, parent: str, name: str) -> Path:
        d = self.root / parent / name
        (d / "repo").mkdir(parents=True)
        (d / "repo" / "README").write_text("an example project\n")
        (d / "task.toml").write_text(
            'description = "write the answer"\nprompt = "Write 42 to answer."\nchore = "bug-fix"\nopt_in = true\n'
        )
        # Uses the shared helpers, as the tasks in bench/tasks/ do.
        (d / "check.py").write_text(
            "import os, sys\nsys.path.insert(0, os.environ['BENCH_CHECK_HELPERS'])\n"
            "from pathlib import Path\nfrom _check import expect, finish\n"
            "expect(Path('answer').exists() and Path('answer').read_text() == '42', 'no answer')\nfinish()\n"
        )
        (d / "solve.py").write_text("from pathlib import Path\nPath('answer').write_text('42')\n")
        return d

    def test_tasks_dir_adds_its_tasks(self):
        d = self.task("private", "example-fix")
        tasks.EXTRA_DIRS[:] = [self.root / "private"]
        self.assertIn("example-fix", tasks.all_names())
        self.assertIn("smoke", tasks.all_names())
        self.assertEqual(tasks.load("example-fix").dir, d.resolve())

    def test_environment_lists_directories(self):
        import os

        self.task("one", "first-fix")
        self.task("two", "second-fix")
        os.environ["JOUET_BENCH_TASKS"] = os.pathsep.join([str(self.root / "one"), "", str(self.root / "two")])
        names = tasks.all_names()
        self.assertIn("first-fix", names)
        self.assertIn("second-fix", names)
        self.assertEqual(tasks.directories()[1:], [(self.root / "one").resolve(), (self.root / "two").resolve()])

    def test_a_name_in_two_directories_is_an_error(self):
        self.task("one", "example-fix")
        self.task("two", "example-fix")
        tasks.EXTRA_DIRS[:] = [self.root / "one", self.root / "two"]
        with self.assertRaisesRegex(ValueError, "example-fix is in both"):
            tasks.all_names()
        tasks.EXTRA_DIRS[:] = [self.root / "one"]
        self.task("three", "smoke")
        tasks.EXTRA_DIRS.append(self.root / "three")
        with self.assertRaisesRegex(ValueError, "smoke is in both"):
            tasks.load("smoke")

    def test_the_same_directory_twice_is_read_once(self):
        self.task("one", "example-fix")
        tasks.EXTRA_DIRS[:] = [self.root / "one", self.root / "one" / ".." / "one"]
        self.assertIn("example-fix", tasks.all_names())

    def test_check_tasks_proves_an_external_task(self):
        import argparse
        import contextlib
        import io

        import agents

        self.task("private", "example-fix")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            result = agents.cmd_check_tasks(
                argparse.Namespace(tasks=["example-fix"], tasks_dir=[self.root / "private"], jouet=self.root / "unused")
            )
        self.assertEqual(result, 0)
        self.assertIn("ok   example-fix: check passes when solved", out.getvalue())

    def test_check_tasks_refuses_a_missing_directory_or_an_unknown_task(self):
        import argparse
        import contextlib
        import io

        import agents

        self.task("private", "example-fix")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            missing = agents.cmd_check_tasks(
                argparse.Namespace(tasks=None, tasks_dir=[self.root / "absent"], jouet=self.root / "unused")
            )
            unknown = agents.cmd_check_tasks(
                argparse.Namespace(tasks=["other"], tasks_dir=[self.root / "private"], jouet=self.root / "unused")
            )
        self.assertEqual((missing, unknown), (2, 2))
        self.assertIn("no task directory at", err.getvalue())
        self.assertIn("unknown task: other", err.getvalue())

    def test_run_records_the_task_directory(self):
        import contextlib
        import io
        import json
        from unittest.mock import patch

        import agents

        self.task("private", "example-fix")
        args = agents.parse_args(
            ["run", "--tasks-dir", str(self.root / "private"), "--tasks", "example-fix", "--harness", "claude",
             "--arms", "plain", "--rounds", "1", "--seed", "1", "--out", str(self.root / "out")]
        )

        def suite(planned, opts, out, log, on_snapshot=None):
            task = tasks.load(planned[0].task)
            return [{"id": planned[0].id, "task": task.name, "task_dir": str(task.dir.parent), "harness": "claude",
                     "arm": "plain", "round": 1, "success": True, "usage": {}}]

        with patch.object(arms, "resolve", return_value=Path("/bin/true")), \
                patch.object(arms, "auth_status", return_value="test"), \
                patch.object(runner, "version", return_value="test"), \
                patch.object(runner, "run_suite", side_effect=suite), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(agents.cmd_run(args), 0)
        meta = json.loads((self.root / "out" / "meta.json").read_text())
        self.assertIn(str((self.root / "private").resolve()), meta["task_dirs"])


class ReportTest(unittest.TestCase):
    def test_spend_per_solved_task_counts_failed_runs_and_weights_cache_reads(self):
        def record(arm, number, success, tokens, cached):
            return {"task": "web", "harness": "codex", "arm": arm, "round": number, "success": success,
                    "wall_s": 10, "usage": {"input_tokens": tokens, "cached_input_tokens": cached}}

        records = [record("plain", 1, True, 1000, 900), record("plain", 2, True, 1000, 900),
                   record("jouet", 1, True, 600, 500), record("jouet", 2, False, 600, 500)]
        cell = report.summarize(records, {})["cells"][0]
        plain, jouet = cell["arms"]["plain"]["per_solved"], cell["arms"]["jouet"]["per_solved"]
        self.assertEqual(plain["input_tokens"], 1000)
        # The failed run's spend counts against the one that passed.
        self.assertEqual(jouet["input_tokens"], 1200)
        self.assertAlmostEqual(plain["weighted_input_tokens"], 190)
        self.assertAlmostEqual(jouet["weighted_input_tokens"], 300)
        self.assertIsNone(plain["cost_usd"])
        text = report.markdown({"meta": {}, "cells": [cell]})
        self.assertIn("Per solved task (all scored runs' spend over the runs that passed): input: plain 1.0k, jouet 1.2k (+20%)", text)
        self.assertNotIn("cost: ", text)

        nothing = [record("plain", 1, False, 1000, 0)]
        self.assertIsNone(report.summarize(nothing, {})["cells"][0]["arms"]["plain"]["per_solved"]["input_tokens"])

    def test_primary_comparison_prefers_initialized_jouet(self):
        def record(arm, tokens, success=True):
            return {"task": "web", "harness": "codex", "arm": arm, "round": 1,
                    "success": success, "wall_s": 10, "usage": {"input_tokens": tokens, "cached_input_tokens": 0}}

        records = [record("plain", 100), record("jouet", 80), record("jouet-init", 60)]
        cell = report.summarize(records, {})["cells"][0]
        self.assertEqual(cell["treatment"], "jouet-init")
        self.assertEqual(cell["paired_when_both_pass"]["input_tokens"]["median_diff"], -40)
        self.assertEqual(cell["init_comparison"]["paired_when_both_pass"]["input_tokens"]["median_diff"], -20)
        self.assertIn("| web (jouet-init) | codex | 1/1 | 1/1 | 1 | -40", report.markdown({"meta": {}, "cells": [cell]}))

        cell = report.summarize(records[:2], {})["cells"][0]
        self.assertEqual(cell["treatment"], "jouet")
        self.assertEqual(cell["paired_when_both_pass"]["input_tokens"]["median_diff"], -20)

    def test_initialized_comparison_includes_only_both_passed_pairs(self):
        def record(arm, number, success, tokens, **extra):
            return {"task": "web", "harness": "codex", "arm": arm, "round": number,
                    "success": success, "wall_s": 10 if arm == "jouet" else 15,
                    "usage": {"input_tokens": tokens, "cached_input_tokens": tokens // 2,
                              "output_tokens": 10}, **extra}

        init = {"files": [".jouet/project.toml"], "wall_s": 12.5,
                "usage": {"input_tokens": 200, "cached_input_tokens": 150, "output_tokens": 20}}
        records = [record("jouet", 1, True, 100), record("jouet-init", 1, True, 120, init=init),
                   record("jouet", 2, True, 100), record("jouet-init", 2, False, 500),
                   record("jouet", 3, True, 100), record("jouet-init", 3, True, 300, error="auth")]
        cell = report.summarize(records, {})["cells"][0]
        comparison = cell["init_comparison"]
        self.assertEqual((comparison["pairs"], comparison["both_passed"]), (2, 1))
        self.assertEqual(comparison["paired_when_both_pass"]["input_tokens"]["median_diff"], 20)
        text = report.markdown({"meta": {}, "cells": [cell]})
        self.assertIn("Initialized versus ordinary jouet", text)
        self.assertIn("| input tokens (all context read) | 100 | 310 | +20 |", text)
        self.assertNotIn("12.5s, 200 input tokens", text)

    def test_report_backfills_only_complete_local_codex_streams(self):
        import argparse
        import contextlib
        import io
        from unittest.mock import patch

        import agents

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            out = root / "results"
            run = out / "runs" / "one"
            run.mkdir(parents=True)
            events = [{"type": "jouet.ephemeral"}, {"type": "thread.started"},
                      {"type": "item.started", "item": {"type": "commandExecution", "id": "c"}},
                      {"type": "item.completed", "item": {"type": "commandExecution", "id": "c", "exitCode": 1}},
                      {"type": "turn.completed"}]
            (run / "agent.jsonl").write_text("\n".join(json.dumps(event) for event in events))
            outside = root / "external"
            outside.mkdir()
            (outside / "agent.jsonl").write_text((run / "agent.jsonl").read_text())
            (out / "runs" / "linked").symlink_to(outside, target_is_directory=True)
            records = [{"id": ident, "harness": "codex", "stopped": False, "exit_code": 0}
                       for ident in ("one", "missing", "../external", "linked")]
            records.append({"id": "one", "harness": "codex", "stopped": True, "exit_code": 0})
            (out / "meta.json").write_text("{}")
            (out / "runs.jsonl").write_text("\n".join(json.dumps(record) for record in records))
            (out / "summary.md").write_text("")
            with patch.object(agents, "write_summary") as write, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(agents.cmd_report(argparse.Namespace(dir=out)), 0)
            actual = write.call_args.args[1]
            self.assertEqual(actual[0]["shell"], {"completed": 1, "failed": 1})
            self.assertTrue(all("shell" not in record for record in actual[1:]))

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
        self.assertIn("**Benchmark checkout:** `unavailable`", text)
        self.assertIn("binary SHA-256 `unavailable`", text)
        identified = report.markdown(report.summarize([], {
            "commit": "harness-revision", "jouet_version": "jouet 0.1", "jouet_sha256": "abc123"
        }))
        self.assertIn("**Benchmark checkout:** `harness-revision`", identified)
        self.assertIn("**jouet:** jouet 0.1; binary SHA-256 `abc123`", identified)
        self.assertIn("| smoke | claude | 2/2 | 1/2 | 1 | -200 (-20%)", text)
        self.assertIn("diagnostic metrics include finished runs even when a task failed or stopped early", text)
        self.assertIn("`python-edit:deny` ×2", text)
        self.assertIn("Paired outcomes: 1 both passed; 1 plain only; 0 jouet only; 0 both failed.", text)
        self.assertIn("Both-pass pairs (1): median jouet minus plain: input -200", text)
        cell = summary["cells"][0]
        self.assertEqual(cell["paired"]["input_tokens"]["median_diff"], -50)
        self.assertEqual(cell["paired_when_both_pass"]["input_tokens"]["median_diff"], -200)

    def test_codex_jouet_reports_unverified_stop_gate(self):
        def record(harness, arm):
            return {"task": "smoke", "harness": harness, "arm": arm,
                    "round": 1, "success": True, "wall_s": 1.0}

        for arm in ("jouet", "jouet-init"):
            summary = report.summarize([record("codex", "plain"), record("codex", arm)], {})
            self.assertIn("codex_stop_gate_unverified", summary["cells"][0]["limitations"])
            self.assertIn("Codex stop-gate coverage is not independently verified", report.markdown(summary))

        for records in ([record("codex", "plain")], [record("claude", "jouet")]):
            summary = report.summarize(records, {})
            self.assertNotIn("limitations", summary["cells"][0])
            self.assertNotIn("Codex stop-gate coverage", report.markdown(summary))

    def test_markdown_shows_change_evidence_without_silent_truncation(self):
        paths = ["name`with\nnewline"] + [f"file-{i}.txt" for i in range(11)]
        records = [
            {"task": "smoke", "harness": "codex", "arm": "plain", "round": 1,
             "success": True, "changed_paths": []},
            {"task": "smoke", "harness": "codex", "arm": "plain", "round": 2,
             "success": True},
            {"task": "smoke", "harness": "codex", "arm": "jouet", "round": 1,
             "success": True, "changed_paths": paths},
            {"task": "smoke", "harness": "codex", "arm": "jouet", "round": 2,
             "success": False, "changed_paths": None},
            {"task": "smoke", "harness": "codex", "arm": "jouet", "round": 3,
             "error": "model unavailable", "changed_paths": ["partial.txt"]},
        ]
        summary = report.summarize(records, {})
        text = report.markdown(summary)
        self.assertIn("**plain changed paths (round 1):** none observed", text)
        self.assertIn("**plain changed paths (round 2):** unavailable", text)
        self.assertIn('"file-0.txt"', text)
        self.assertIn('``["name`with\\nnewline"', text)
        self.assertIn("2 more (full list: runs.jsonl)", text)
        self.assertIn("**jouet changed paths (round 2):** unavailable", text)
        self.assertIn('**jouet changed paths (round 3):** `["partial.txt"]`', text)
        self.assertEqual(summary["cells"][0]["arms"]["jouet"]["changes"][0]["paths"], paths)

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
        self.assertEqual((cell["arms"]["jouet"]["successes"], cell["arms"]["jouet"]["runs"]), (1, 3))
        self.assertIsNone(cell["paired_when_both_pass"]["input_tokens"])
        text = report.markdown({"meta": {}, "cells": [cell]})
        self.assertIn("| t | codex | 3/5 | 1/3 | 0 | – | – | – | – | – |", text)
        self.assertIn("Both-pass pairs (0): median jouet minus plain: no comparable metrics.", text)

    def test_unverified_runs_are_excluded_from_all_comparisons(self):
        def record(arm, round_number, success, tokens):
            return {"task": "t", "harness": "codex", "arm": arm, "round": round_number,
                    "success": success, "usage": {"input_tokens": tokens, "cached_input_tokens": 0}}

        records = [record("plain", 1, True, 100), record("jouet", 1, True, 90),
                   record("plain", 2, True, 200), record("jouet", 2, None, 10000)]
        records += [record("plain", 3, True, 300),
                    {**record("jouet", 3, False, 20000), "check": {"passed": None}}]
        cell = report.summarize(records, {})["cells"][0]
        self.assertEqual(cell["arms"]["jouet"]["runs"], 1)
        self.assertEqual(len(cell["arms"]["jouet"]["unverified"]), 2)
        self.assertEqual(cell["arms"]["jouet"]["metrics"]["input_tokens"]["median"], 90)
        self.assertEqual(cell["paired"]["input_tokens"]["pairs"], 1)
        self.assertIn("jouet check unverified (round 2)", report.markdown({"meta": {}, "cells": [cell]}))

    def test_unverified_checks_do_not_hide_agent_failures(self):
        def record(arm, round_number, success, **extra):
            return {"task": "t", "harness": "codex", "arm": arm, "round": round_number,
                    "success": success, "usage": {"input_tokens": 100, "cached_input_tokens": 0}, **extra}

        records = [record("plain", n, True, exit_code=0) for n in range(1, 5)]
        records += [
            record("jouet", 1, False, exit_code=1, check={"passed": None}),
            record("jouet", 2, False, exit_code=None, stopped="token budget", check={"passed": None}),
            record("jouet", 3, None, exit_code=0, check={"passed": None}),
            record("jouet", 4, False, check={"passed": None}),
        ]
        cell = report.summarize(records, {})["cells"][0]
        self.assertEqual((cell["arms"]["jouet"]["successes"], cell["arms"]["jouet"]["runs"]), (0, 2))
        self.assertEqual(len(cell["arms"]["jouet"]["unverified"]), 4)
        self.assertEqual(cell["arms"]["jouet"]["stopped"], ["token budget"])
        self.assertEqual(cell["paired_outcomes"]["plain_only"], 2)
        self.assertIsNone(cell["arms"]["jouet"]["metrics"]["input_tokens"])
        self.assertIsNone(cell["paired"]["input_tokens"])
        self.assertIsNone(cell["paired_when_both_pass"]["input_tokens"])

    def test_init_comparison_counts_known_failures_with_unverified_checks(self):
        records = [
            {"task": "t", "harness": "codex", "arm": "jouet", "round": 1,
             "success": False, "exit_code": 1, "check": {"passed": None}},
            {"task": "t", "harness": "codex", "arm": "jouet-init", "round": 1,
             "success": True, "exit_code": 0},
            {"task": "t", "harness": "codex", "arm": "jouet", "round": 2,
             "success": None, "exit_code": 0, "check": {"passed": None}},
            {"task": "t", "harness": "codex", "arm": "jouet-init", "round": 2,
             "success": True, "exit_code": 0},
        ]
        comparison = report.summarize(records, {})["cells"][0]["init_comparison"]
        self.assertEqual((comparison["pairs"], comparison["both_passed"]), (1, 0))
        self.assertIsNone(comparison["paired_when_both_pass"]["input_tokens"])


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
                    patch.object(tasks, "setup_environment", side_effect=fail_setup), \
                    self.assertRaises(tasks.EnvironmentSetupError):
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

    def test_incomplete_cost_seed_is_reported_and_not_cached(self):
        import dataclasses
        import os
        from unittest.mock import patch

        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task_dir = root / "task"
            (task_dir / "repo").mkdir(parents=True)
            (task_dir / "repo" / "a.txt").write_text("a\n")
            task = tasks.Task(name="t", dir=task_dir, description="", prompt="", chore="", rules=[],
                              timeout_s=60, cost_seed=["cargo build", "pnpm vitest run test.ts"])
            stand_in = root / "jouet"
            stand_in.write_text(
                '#!/bin/sh\n'
                'case "$*" in *"pnpm vitest"*) exit 0;; esac\n'
                'git notes --ref jouet-cost add -f -m \'{"cmd":"cargo build"}\' HEAD\n'
                'exit "${FAIL_RECORDED:-0}"\n'
            )
            stand_in.chmod(0o755)
            messages = []
            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root / "cache")}):
                note = costs.seed(task, stand_in, root / "work", messages.append)
                self.assertFalse(costs.cache_path(task, stand_in).exists())
            self.assertIsNone(note)
            self.assertTrue(any("no timing was recorded for pnpm vitest run test.ts" in m for m in messages))
            self.assertTrue(any("runs start without a cost note" in m for m in messages))
            failed = dataclasses.replace(task, cost_seed=["cargo build"], env={"FAIL_RECORDED": "1"})
            messages.clear()
            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root / "cache")}):
                self.assertIsNone(costs.seed(failed, stand_in, root / "second-work", messages.append))
                self.assertFalse(costs.cache_path(failed, stand_in).exists())
            self.assertTrue(any("cargo build failed" in m for m in messages))

    def test_timed_out_cost_seed_does_not_block_trials_or_leave_temp_files(self):
        import os
        import subprocess
        from unittest.mock import patch

        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            task_dir = root / "task"
            (task_dir / "repo").mkdir(parents=True)
            (task_dir / "repo" / "a.txt").write_text("a\n")
            task = tasks.Task(name="t", dir=task_dir, description="", prompt="", chore="", rules=[],
                              timeout_s=60, check_timeout_s=1, cost_seed=["cargo build"])
            tmp_record = root / "tmp-path"
            marker = root / "child-survived"
            stand_in = root / "jouet"
            stand_in.write_text(f'#!/bin/sh\nprintf "%s" "$TMPDIR" > "{tmp_record}"\n'
                                f'(sleep 2; touch "{marker}") &\nwait\n')
            stand_in.chmod(0o755)
            messages = []
            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(root / "cache")}):
                self.assertIsNone(costs.seed(task, stand_in, root / "work", messages.append))
                self.assertFalse(costs.cache_path(task, stand_in).exists())
            self.assertTrue(any("could not be timed" in message for message in messages))
            self.assertTrue(any("runs start without a cost note" in message for message in messages))
            self.assertFalse(Path(tmp_record.read_text()).exists())
            subprocess.run(["sleep", "2"], check=True)
            self.assertFalse(marker.exists(), "the timed-out check's child kept running")

    def test_cost_cache_changes_with_setup_tools_and_jouet(self):
        import dataclasses
        import os
        from unittest.mock import patch

        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            tool = root / "pnpm"
            tool.write_text("tool one")
            tool.chmod(0o755)
            binary = root / "jouet"
            binary.write_text("jouet one")
            task = dataclasses.replace(tasks.load("smoke"), cost_seed=["pnpm test"],
                                       setup=["pnpm install"], tools={"pnpm": "pnpm"})
            with patch.dict(os.environ, {"PATH": str(root), "XDG_CACHE_HOME": str(root / "cache")}):
                original = costs.cache_path(task, binary)
                self.assertEqual(costs.cache_path(task, binary), original)
                self.assertNotEqual(costs.cache_path(dataclasses.replace(task, setup=["pnpm build"]), binary), original)
                self.assertNotEqual(costs.cache_path(dataclasses.replace(task, env={"MODE": "offline"}), binary), original)
                with patch.object(costs, "harness_hashes", return_value=["different harness"]):
                    self.assertNotEqual(costs.cache_path(task, binary), original)
                tool.write_text("a different tool version")
                self.assertNotEqual(costs.cache_path(task, binary), original)
                tool.write_text("tool one")
                tool.chmod(0o755)
                before_binary_change = costs.cache_path(task, binary)
                binary.write_text("jouet two")
                self.assertNotEqual(costs.cache_path(task, binary), before_binary_change)

    def test_cost_cache_fingerprints_the_tool_behind_a_mise_shim(self):
        import dataclasses
        import os
        from unittest.mock import patch

        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            tool = root / "actual-pnpm"
            tool.write_text("first version")
            tool.chmod(0o755)
            mise = root / "mise"
            mise.write_text(f'#!/bin/sh\nprintf "%s\\n" "{tool}"\n')
            mise.chmod(0o755)
            (root / "pnpm").symlink_to(mise)
            binary = root / "jouet"
            binary.write_text("jouet")
            task = dataclasses.replace(tasks.load("smoke"), cost_seed=["pnpm test"])
            with patch.dict(os.environ, {"PATH": str(root), "XDG_CACHE_HOME": str(root / "cache")}):
                original = costs.cache_path(task, binary)
                tool.write_text("a different version of the tool")
                self.assertNotEqual(costs.cache_path(task, binary), original)

    def test_cost_cache_does_not_resolve_worktree_commands_from_the_host_cwd(self):
        import dataclasses
        import os
        from unittest.mock import patch

        from jbench import costs

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            binary = root / "jouet"
            binary.write_text("jouet")
            for name in ("first", "second"):
                (root / name / "local").mkdir(parents=True)
            (root / "first/local/check").write_text("unrelated host file")
            task = dataclasses.replace(tasks.load("smoke"), cost_seed=["local/check"])
            previous = Path.cwd()
            try:
                with patch.dict(os.environ, {"PATH": str(root), "XDG_CACHE_HOME": str(root / "cache")}):
                    os.chdir(root / "first")
                    first = costs.cache_path(task, binary)
                    os.chdir(root / "second")
                    self.assertEqual(costs.cache_path(task, binary), first)
            finally:
                os.chdir(previous)
