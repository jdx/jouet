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
            init.git(work, "add", "-A")
            init.git(work, "reset", "-q", "--", *sorted(before))
            patch = Path(d) / "init.patch"
            patch.write_text(init.git(work, "diff", "--cached", "--binary", "HEAD"))
            self.assertNotIn("notes.md", patch.read_text())
            # A fresh tree with that file applies it cleanly.
            init.git(work, "reset", "-q", "--hard")
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

    def test_the_report_says_what_init_cost_and_when_it_pays_back(self):
        cell = {
            "arms": {
                "plain": {"metrics": {"input_tokens": {"median": 300_000}}},
                "jouet-init": {"metrics": {"input_tokens": {"median": 200_000}}},
            }
        }
        meta = {"usage": {"input_tokens": 450_000}, "wall_s": 120, "files": [".jouet/test.toml"]}
        line = report.init_line(cell, meta)
        self.assertIn("450.0k input tokens in 120s", line)
        self.assertIn(".jouet/test.toml", line)
        self.assertIn("pays back after 4.5 runs", line)
        cell["arms"]["jouet-init"]["metrics"]["input_tokens"]["median"] = 310_000
        self.assertIn("does not pay back", report.init_line(cell, meta))


if __name__ == "__main__":
    unittest.main()
