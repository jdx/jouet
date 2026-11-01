import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from jbench import arms


class ResolveTest(unittest.TestCase):
    def test_bench_environment_prefers_mise_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            mise = root / "mise"
            mise.write_text("#!/bin/sh\nprintf '%s\\n' /pinned/python3\n")
            mise.chmod(0o755)
            python = root / "python3"
            python.write_text("#!/bin/sh\nexit 0\n")
            python.chmod(0o755)
            with patch.dict(os.environ, {"PATH": tmp, "MISE_ENV": "bench"}):
                self.assertEqual(arms.resolve("python3"), Path("/pinned/python3"))

    def test_resolve_mise_itself(self):
        with tempfile.TemporaryDirectory() as tmp:
            mise = Path(tmp) / "mise"
            mise.write_text("#!/bin/sh\nexit 1\n")
            mise.chmod(0o755)
            with patch.dict(os.environ, {"PATH": tmp}):
                self.assertEqual(arms.resolve("mise"), mise.resolve())

    def test_inactive_mise_shim_falls_back_to_next_path_entry(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            shim_dir = root / "shims"
            bin_dir = root / "bin"
            shim_dir.mkdir()
            bin_dir.mkdir()
            mise = root / "mise"
            mise.write_text("#!/bin/sh\nexit 1\n")
            mise.chmod(0o755)
            (shim_dir / "python3").symlink_to(mise)
            python = bin_dir / "python3"
            python.write_text("#!/bin/sh\nexit 0\n")
            python.chmod(0o755)
            with patch.dict(os.environ, {"PATH": os.pathsep.join((str(shim_dir), str(bin_dir)))}):
                self.assertEqual(arms.resolve("python3"), python.resolve())

    def test_login_shell_preserves_isolated_path(self):
        shell = shutil.which("zsh")
        if not shell:
            self.skipTest("zsh is not installed")
        with tempfile.TemporaryDirectory() as tmp:
            env = {"HOME": tmp, "PATH": "/isolated/bin:/usr/bin:/bin"}
            arms.login_shell_path(env)
            result = subprocess.run([shell, "-lc", "printf %s \"$PATH\""], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, env["PATH"])
