import subprocess
import tempfile
import unittest
from pathlib import Path

from jbench import arms, init, report


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


class InitTest(unittest.TestCase):
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

            head = init.apply(work, patch)
            self.assertEqual(head, git(work, "rev-parse", "HEAD").strip())
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
            self.assertIsNotNone(init.apply(work, patch))
            self.assertEqual((work / "notes.md").read_text(), "theirs\n")

    def test_a_skill_that_changed_nothing_leaves_the_tree_as_it_was(self):
        with tempfile.TemporaryDirectory() as d:
            work = self.repo(d)
            before = git(work, "rev-parse", "HEAD")
            patch = Path(d) / "init.patch"
            patch.write_text("")
            self.assertIsNone(init.apply(work, patch))
            self.assertEqual(git(work, "rev-parse", "HEAD"), before)

    def test_the_arm_runs_jouet_after_init(self):
        arm = arms.ARMS["jouet-init"]
        self.assertTrue(arm.jouet)
        self.assertTrue(arm.init)
        self.assertFalse(arms.ARMS["jouet"].init)

    def test_the_report_says_what_init_configured_and_shows_its_separate_cost(self):
        meta = {"usage": {"input_tokens": 450_000}, "wall_s": 120, "files": [".jouet/test.toml"]}
        line = report.init_line({"arms": {}}, meta)
        self.assertIn(".jouet/test.toml", line)
        self.assertIn("120.0s, 450,000 input tokens", line)
        self.assertNotIn("pays back", line)
        self.assertIn("changed no files", report.init_line({"arms": {}}, {"files": []}))


if __name__ == "__main__":
    unittest.main()
