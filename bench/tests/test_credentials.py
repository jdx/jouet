import os
import tempfile
import time
import unittest
from pathlib import Path

from jbench import credentials, runner


class CredentialsTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        self.user = root / "user"
        self.run = root / "run"
        self.user.mkdir()
        self.run.mkdir()
        self.source = self.user / ".credentials.json"
        self.source.write_text('{"token": "old"}')
        os.chmod(self.source, 0o600)

    def tearDown(self):
        self.dir.cleanup()

    def refresh_in_run(self, text):
        """What a refreshing CLI does: write a new file and rename it over
        the old one, which is what turned the old symlink into a file."""
        tmp = self.run / ".credentials.json.tmp"
        tmp.write_text(text)
        os.replace(tmp, self.run / ".credentials.json")

    def test_a_run_gets_a_private_copy_not_a_link(self):
        [lent] = credentials.lend([self.source, self.user / "missing.json"], self.run)
        copy = self.run / ".credentials.json"
        self.assertFalse(copy.is_symlink())
        self.assertEqual(copy.read_text(), '{"token": "old"}')
        self.assertEqual(copy.stat().st_mode & 0o777, 0o600)
        self.assertEqual(lent.source, self.source.resolve())

    def test_a_refresh_in_the_run_is_written_back(self):
        lent = credentials.lend([self.source], self.run)
        self.refresh_in_run('{"token": "new"}')
        notes = credentials.give_back(lent)
        self.assertEqual(self.source.read_text(), '{"token": "new"}')
        self.assertEqual(self.source.stat().st_mode & 0o777, 0o600)
        self.assertEqual(len(notes), 1)
        self.assertIn("wrote the run's refreshed credentials back", notes[0])

    def test_an_unchanged_copy_leaves_the_users_file_alone(self):
        lent = credentials.lend([self.source], self.run)
        before = self.source.stat().st_mtime_ns
        self.assertEqual(credentials.give_back(lent), [])
        self.assertEqual(self.source.stat().st_mtime_ns, before)

    def test_a_deleted_or_emptied_copy_is_not_written_back(self):
        lent = credentials.lend([self.source], self.run)
        (self.run / ".credentials.json").write_text("")
        self.assertEqual(credentials.give_back(lent), [])
        (self.run / ".credentials.json").unlink()
        self.assertEqual(credentials.give_back(lent), [])
        self.assertEqual(self.source.read_text(), '{"token": "old"}')

    def test_when_both_refreshed_the_newer_one_is_kept(self):
        lent = credentials.lend([self.source], self.run)
        self.refresh_in_run('{"token": "run"}')
        time.sleep(0.01)
        self.source.write_text('{"token": "user"}')
        notes = credentials.give_back(lent)
        self.assertEqual(self.source.read_text(), '{"token": "user"}')
        self.assertIn("newer than the run's copy", notes[0])

        lent = credentials.lend([self.source], self.run)
        self.source.write_text('{"token": "user again"}')
        time.sleep(0.01)
        self.refresh_in_run('{"token": "run again"}')
        notes = credentials.give_back(lent)
        self.assertEqual(self.source.read_text(), '{"token": "run again"}')
        self.assertIn("changed during the run", notes[0])

    def test_a_linked_source_is_written_through_its_link(self):
        target = self.user / "real.json"
        self.source.rename(target)
        self.source.symlink_to(target)
        lent = credentials.lend([self.source], self.run)
        self.refresh_in_run('{"token": "new"}')
        credentials.give_back(lent)
        self.assertTrue(self.source.is_symlink())
        self.assertEqual(target.read_text(), '{"token": "new"}')


class SnapshotTest(unittest.TestCase):
    def test_the_suite_keeps_running_its_copy_when_the_build_goes_away(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            cache = root / "cache" / "debug"
            cache.mkdir(parents=True)
            built = cache / "jouet"
            built.write_text("#!/bin/sh\necho jouet\n")
            link = root / "target-debug-jouet"
            link.symlink_to(built)
            copy = runner.snapshot(link, root / "work")
            built.unlink()
            self.assertFalse(copy.is_symlink())
            self.assertEqual(copy.read_text(), "#!/bin/sh\necho jouet\n")
            self.assertTrue(os.access(copy, os.X_OK))


class AuthFailureTest(unittest.TestCase):
    def test_a_login_failure_stops_the_suite_and_nothing_else_does(self):
        failed = {
            "usage": {
                "is_error": True,
                "input_tokens": 0,
                "result": "Failed to authenticate: OAuth session expired and could not be refreshed",
            }
        }
        self.assertTrue(runner.auth_failed(failed))
        self.assertFalse(runner.auth_failed({"usage": {"is_error": True, "input_tokens": 900, "result": "x"}}))
        self.assertFalse(runner.auth_failed({"usage": {"is_error": False, "input_tokens": 0}}))
        self.assertFalse(runner.auth_failed({"error": "RuntimeError: boom"}))


if __name__ == "__main__":
    unittest.main()
