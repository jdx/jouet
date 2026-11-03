import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch as patch_env

from jbench import arms, init, report


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


class InitTest(unittest.TestCase):
    def apply(self, work, patch):
        head = init.git(work, "rev-parse", "HEAD").strip()
        shas = {"base_sha": head, "head_sha": head}
        init.apply(work, patch, SimpleNamespace(upstream=False), shas)
        return shas

    def repo(self, d):
        work = Path(d) / "work"
        work.mkdir()
        git(work, "init", "-q", "-b", "main")
        git(work, "config", "user.name", "t")
        git(work, "config", "user.email", "t@example.com")
        (work / "a.txt").write_text("a\n")
        git(work, "add", "a.txt")
        git(work, "commit", "-q", "-m", "base")
        return work

    def test_the_init_patch_is_applied_and_committed_on_a_clean_tree(self):
        with tempfile.TemporaryDirectory() as d:
            work = self.repo(d)
            # What the skill left: a new config file and an edited one.
            (work / ".jouet").mkdir()
            (work / ".jouet" / "test.toml").write_text("[test]\n")
            (work / "a.txt").write_text("a\nb\n")
            git(work, "add", "-A")
            patch = Path(d) / "init.patch"
            patch.write_text(git(work, "diff", "--cached", "--binary", "HEAD"))
            git(work, "reset", "-q", "--hard")
            git(work, "clean", "-qfd")

            shas = self.apply(work, patch)
            self.assertEqual(shas["head_sha"], git(work, "rev-parse", "HEAD").strip())
            self.assertEqual(shas["base_sha"], shas["head_sha"])
            self.assertEqual(git(work, "status", "--porcelain"), "")
            self.assertEqual((work / ".jouet" / "test.toml").read_text(), "[test]\n")

    def test_a_tasks_own_untracked_files_stay_out_of_the_patch(self):
        with tempfile.TemporaryDirectory() as d:
            work = self.repo(d)
            # The task leaves another session's file untracked.
            (work / "notes.md").write_text("theirs\n")
            before = set(init.untracked(work))
            (work / ".jouet").mkdir()
            (work / ".jouet" / "test.toml").write_text("[test]\n")
            # Even committed by the skill, the task's file stays out.
            init.git(work, "add", "-A")
            init.git(work, "commit", "-q", "-m", "skill")
            paths = ["--", ".", *(f":(exclude,literal){p}" for p in sorted(before))]
            patch = Path(d) / "init.patch"
            patch.write_text(init.git(work, "diff", "--cached", "--binary", "HEAD~1", *paths))
            self.assertNotIn("notes.md", patch.read_text())
            # A fresh tree, with that file untracked again, applies it cleanly.
            init.git(work, "reset", "-q", "--hard", "HEAD~1")
            (work / "notes.md").write_text("theirs\n")
            self.apply(work, patch)
            self.assertEqual((work / "notes.md").read_text(), "theirs\n")

    def test_a_skill_that_changed_nothing_leaves_the_tree_as_it_was(self):
        with tempfile.TemporaryDirectory() as d:
            work = self.repo(d)
            before = git(work, "rev-parse", "HEAD")
            patch = Path(d) / "init.patch"
            patch.write_text("")
            self.apply(work, patch)
            self.assertEqual(git(work, "rev-parse", "HEAD"), before)

    def test_configuration_is_in_the_baseline_and_task_changes_remain(self):
        with tempfile.TemporaryDirectory() as d:
            work = self.repo(d)
            base = git(work, "rev-parse", "HEAD").strip()
            origin = work.parent / "origin.git"
            git(work.parent, "init", "--bare", "-q", str(origin))
            git(work, "remote", "add", "origin", str(origin))
            git(work, "push", "-q", "origin", "HEAD:refs/heads/main")
            git(work, "fetch", "-q", "origin")
            (work / "task.txt").write_text("task change\n")
            git(work, "add", "task.txt")
            git(work, "commit", "-q", "-m", "task")
            head = git(work, "rev-parse", "HEAD").strip()
            (work / "config.txt").write_text("configuration\n")
            git(work, "add", "config.txt")
            patch = Path(d) / "init.patch"
            patch.write_text(git(work, "diff", "--cached", "--binary", "HEAD"))
            git(work, "reset", "-q", "--hard")
            shas = {"base_sha": base, "head_sha": head}

            init.apply(work, patch, SimpleNamespace(upstream=True, remote=False), shas)

            self.assertEqual(git(work, "rev-parse", "origin/main").strip(), shas["base_sha"])
            self.assertEqual(git(origin, "rev-parse", "refs/heads/main").strip(), shas["base_sha"])
            self.assertEqual(git(work, "diff", "--name-only", "origin/main", "HEAD"), "task.txt\n")
            self.assertEqual(git(work, "show", "origin/main:config.txt"), "configuration\n")
            git(work, "fetch", "-q", "origin")
            self.assertEqual(git(work, "rev-parse", "origin/main").strip(), shas["base_sha"])

    def test_configuration_that_depends_on_task_changes_fails_before_the_run(self):
        with tempfile.TemporaryDirectory() as d:
            work = self.repo(d)
            base = git(work, "rev-parse", "HEAD").strip()
            (work / "a.txt").write_text("task\n")
            git(work, "add", "a.txt")
            git(work, "commit", "-q", "-m", "task")
            head = git(work, "rev-parse", "HEAD").strip()
            (work / "a.txt").write_text("configured task\n")
            git(work, "add", "a.txt")
            patch = Path(d) / "init.patch"
            patch.write_text(git(work, "diff", "--cached", "--binary", "HEAD"))
            git(work, "reset", "-q", "--hard")
            shas = {"base_sha": base, "head_sha": head}

            with self.assertRaisesRegex(RuntimeError, "init patch cannot be applied to the task baseline"):
                init.apply(work, patch, SimpleNamespace(upstream=False), shas)

            self.assertEqual(shas, {"base_sha": base, "head_sha": head})
            self.assertEqual(git(work, "rev-parse", "HEAD").strip(), head)
            self.assertEqual(git(work, "status", "--porcelain"), "")

    def test_init_does_not_follow_git_environment_into_another_repository(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "trial").mkdir()
            (root / "host").mkdir()
            work = self.repo(root / "trial")
            host = self.repo(root / "host")
            host_head = git(host, "rev-parse", "HEAD").strip()
            (work / "config.txt").write_text("configuration\n")
            git(work, "add", "config.txt")
            init_patch = root / "init.patch"
            init_patch.write_text(git(work, "diff", "--cached", "--binary", "HEAD"))
            git(work, "reset", "-q", "--hard")

            with patch_env.dict(os.environ, {"GIT_DIR": str(host / ".git"), "GIT_WORK_TREE": str(host),
                                            "GIT_INDEX_FILE": str(host / ".git" / "index")}):
                shas = self.apply(work, init_patch)

            self.assertEqual(git(work, "show", "HEAD:config.txt"), "configuration\n")
            self.assertEqual(git(work, "rev-parse", "HEAD").strip(), shas["base_sha"])
            self.assertEqual(git(host, "rev-parse", "HEAD").strip(), host_head)
            self.assertEqual(git(host, "status", "--porcelain"), "")

    def test_the_arm_runs_jouet_after_init(self):
        arm = arms.ARMS["jouet-init"]
        self.assertTrue(arm.jouet)
        self.assertTrue(arm.init)
        self.assertFalse(arms.ARMS["jouet"].init)

    def test_the_report_says_what_init_configured_and_counts_none_of_its_cost(self):
        meta = {"usage": {"input_tokens": 450_000}, "wall_s": 120, "files": [".jouet/test.toml"]}
        line = report.init_line({"arms": {}}, meta)
        self.assertIn(".jouet/test.toml", line)
        self.assertNotIn("450", line)
        self.assertNotIn("pays back", line)
        self.assertIn("changed no files", report.init_line({"arms": {}}, {"files": []}))


if __name__ == "__main__":
    unittest.main()
