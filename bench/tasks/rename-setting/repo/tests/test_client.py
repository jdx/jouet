import io
import unittest
from contextlib import redirect_stdout

from shipit import cli
from shipit.upload import UploadError, upload_with_retries


class Flaky:
    def __init__(self, failures):
        self.failures = failures
        self.calls = 0

    def __call__(self, artifact, timeout_s):
        self.calls += 1
        if self.calls <= self.failures:
            raise OSError("connection reset")


class ClientTest(unittest.TestCase):
    def test_retries_until_success(self):
        self.assertEqual(upload_with_retries(Flaky(2), "a", retry_limit=3, timeout_s=1), 3)

    def test_gives_up(self):
        with self.assertRaisesRegex(UploadError, "failed after 2 attempts"):
            upload_with_retries(Flaky(5), "a", retry_limit=2, timeout_s=1)

    def test_cli_flag(self):
        out = io.StringIO()
        with redirect_stdout(out):
            cli.main(["--retry-limit", "5", "--print-config"])
        self.assertEqual(out.getvalue().strip(), "retry_limit=5 timeout_s=30")
