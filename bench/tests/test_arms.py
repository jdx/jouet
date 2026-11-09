import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from jbench import arms, harness, tasks


class ResolveTest(unittest.TestCase):
    def test_project_tools_are_available_in_the_isolated_environment(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("parallel", "uv", "uvx"):
                executable = root / name
                executable.write_text("#!/bin/sh\nexit 0\n")
                executable.chmod(0o755)
            with patch.dict(os.environ, {"PATH": str(root)}):
                env = arms.base_env(root / "run", None, "jdx/demo")
            for name in ("parallel", "uv", "uvx"):
                self.assertEqual((root / "run/bin" / name).resolve(), (root / name).resolve())
            self.assertTrue(env["PATH"].startswith(str(root / "run/bin")))

    def test_claude_runs_as_the_desktop_app_like_real_sessions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            claude = root / "claude-bin"
            claude.write_text("#!/bin/sh\nexit 0\n")
            with patch.dict(os.environ, {"PATH": tmp}, clear=True):
                env = arms.build(root / "run", harness.Claude(), arms.ARMS["plain"], claude, claude, None, "jdx/demo")
        self.assertEqual(env["CLAUDE_CODE_ENTRYPOINT"], "claude-desktop")

    def test_docker_path_is_passed_only_to_the_checker_environment(self):
        with patch.object(tasks.shutil, "which") as lookup:
            self.assertNotIn("BENCH_DOCKER", tasks.check_env({"base_sha": "base", "head_sha": "head"}, None))
            lookup.assert_not_called()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "docker-tools").write_text("#!/bin/sh\nexit 0\n")
            (root / "docker-tools").chmod(0o755)
            (root / "docker").symlink_to(root / "docker-tools")
            with patch.dict(os.environ, {"PATH": tmp}):
                check = tasks.check_env({"base_sha": "base", "head_sha": "head"}, None, True)
            self.assertEqual(check["BENCH_DOCKER"], str(root / "docker"))
        with patch.object(tasks.shutil, "which", return_value=None):
            self.assertNotIn("BENCH_DOCKER", tasks.check_env({"base_sha": "base", "head_sha": "head"}, None, True))

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
