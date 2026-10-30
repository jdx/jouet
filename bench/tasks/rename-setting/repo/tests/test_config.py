import tempfile
import unittest
from pathlib import Path

from shipit import config


class ConfigTest(unittest.TestCase):
    def test_defaults(self):
        self.assertEqual(config.load()["retry_limit"], 3)

    def test_file_then_overrides(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "shipit.toml"
            path.write_text("retry_limit = 7\ntimeout_s = 5\n")
            self.assertEqual(config.load(path)["retry_limit"], 7)
            self.assertEqual(config.load(path, {"retry_limit": 2})["retry_limit"], 2)

    def test_rejects_zero(self):
        with self.assertRaisesRegex(ValueError, "retry_limit must be at least 1"):
            config.load(overrides={"retry_limit": 0})
