import json
import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
import uuid
from unittest.mock import patch

import agents
from jbench import arms
from jbench.codex_hooks import HookPreflightError, copy_trust, require_reviewed
from jbench.harness import Codex


class CodexHooksTest(unittest.TestCase):
    def setUp(self):
        self.path = Path("/bench/home/.codex/hooks.json")
        self.hooks = [{"eventName": event, "sourcePath": str(self.path), "enabled": True,
                       "trustStatus": "trusted"}
                      for event in ("sessionStart", "preToolUse", "postToolUse", "stop")]

    def check(self):
        require_reviewed({"data": [{"hooks": self.hooks, "warnings": [], "errors": []}]}, self.path)

    def test_requires_every_expected_enabled_trusted_hook(self):
        self.check()
        for key, value in (("enabled", False), ("trustStatus", "untrusted"),
                           ("trustStatus", "modified"), ("sourcePath", "/other/hooks.json")):
            with self.subTest(key=key, value=value):
                original = self.hooks[0][key]
                self.hooks[0][key] = value
                with self.assertRaises(HookPreflightError):
                    self.check()
                self.hooks[0][key] = original

    def test_rejects_duplicate_and_unknown_enabled_hooks(self):
        self.hooks.append(dict(self.hooks[0]))
        with self.assertRaises(HookPreflightError):
            self.check()
        self.hooks[-1]["eventName"] = "userPromptSubmit"
        with self.assertRaises(HookPreflightError):
            self.check()

    def test_rejects_missing_inventory_or_errors(self):
        for result in ({}, {"data": []}, {"data": [{"hooks": self.hooks, "errors": ["broken"]}]}):
            with self.subTest(result=result), self.assertRaises(HookPreflightError):
                require_reviewed(result, self.path)

    def test_requires_reviewed_configuration_to_match_the_run_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = root / "run"
            reviewed = (root / "reviewed/.codex").resolve()
            expected.mkdir()
            reviewed.mkdir(parents=True)
            (reviewed.parent / ".jouet-bench-codex-home").write_text("test\n")
            (expected / "hooks.json").write_text(json.dumps({"hooks": {"version": 1}}))
            (reviewed / "hooks.json").write_text(json.dumps({"hooks": {"version": 2}}))
            env = {"CODEX_HOME": str(expected)}
            opts = {"codex_reviewed_home": str(reviewed), "harness_bins": {"codex": "codex"}}
            with patch("jbench.arms.codex_hooks.list_hooks") as lookup:
                with self.assertRaisesRegex(HookPreflightError, "differ"):
                    arms.reviewed_codex(Codex(), env, root, opts)
                lookup.assert_not_called()
            (reviewed / "hooks.json").write_text((expected / "hooks.json").read_text())
            with patch("jbench.arms.codex_hooks.list_hooks") as lookup:
                with self.assertRaisesRegex(HookPreflightError, "not been reviewed"):
                    arms.reviewed_codex(Codex(), env, root, opts)
                lookup.assert_not_called()
            source = reviewed / "hooks.json"
            (reviewed / "config.toml").write_text(
                'cli_auth_credentials_store = "file"\n\n'
                f'[hooks.state."{source}:stop:0:0"]\ntrusted_hash = "sha256:aa"\n\n'
                '[hooks.state."/elsewhere/.codex/hooks.json:stop:0:0"]\ntrusted_hash = "sha256:bb"\n')
            (expected / "config.toml").write_text('[projects."/w"]\ntrust_level = "trusted"\n')
            hooks = [{**hook, "sourcePath": str(expected / "hooks.json")} for hook in self.hooks]
            with patch("jbench.arms.codex_hooks.list_hooks", return_value={"data": [{"hooks": hooks}]}) as lookup:
                arms.reviewed_codex(Codex(), env, root, opts)
                self.assertEqual(env["CODEX_HOME"], str(expected))
                lookup.assert_called_once()
            copied = tomllib.loads((expected / "config.toml").read_text())
            self.assertEqual(copied["projects"], {"/w": {"trust_level": "trusted"}})
            self.assertEqual(copied["hooks"]["state"],
                             {f"{expected / 'hooks.json'}:stop:0:0": {"trusted_hash": "sha256:aa"}})
            with self.assertRaisesRegex(HookPreflightError, "already has hook settings"):
                arms.reviewed_codex(Codex(), env, root, opts)

    def test_copied_trust_keeps_a_non_ascii_run_path_valid(self):
        # json.dumps escapes U+1F600 as a surrogate pair, which TOML rejects.
        with tempfile.TemporaryDirectory() as directory:
            reviewed, run = Path(directory) / "reviewed", Path(directory) / "run 😀"
            reviewed.mkdir()
            run.mkdir()
            key = json.dumps(f"{reviewed / 'hooks.json'}:stop:0:0")
            (reviewed / "config.toml").write_text(f"[hooks.state.{key}]\ntrusted_hash = \"sha256:aa\"\n")
            copy_trust(reviewed, run)
            copied = tomllib.loads((run / "config.toml").read_text(encoding="utf-8"))
            self.assertEqual(copied["hooks"]["state"], {f"{run / 'hooks.json'}:stop:0:0": {"trusted_hash": "sha256:aa"}})

    def test_session_start_requires_an_observed_ledger_event(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"XDG_STATE_HOME": directory}
            self.assertFalse(arms.codex_started(env))
            ledger = Path(directory) / "jouet/ledger/session.jsonl"
            ledger.parent.mkdir(parents=True)
            ledger.write_text('{"event":"pre-tool-use"}\n')
            self.assertFalse(arms.codex_started(env))
            ledger.write_text('{"event":"session-start"}\n')
            self.assertTrue(arms.codex_started(env))

    def test_codex_tool_identity_detects_replaced_wrappers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "codex").symlink_to(sys.executable)
            (root / "jouet").write_text("original wrapper")
            original = arms.codex_tools(root, True)
            (root / "jouet").write_text("different wrapper")
            self.assertNotEqual(arms.codex_tools(root, True), original)
            (root / "jouet").unlink()
            (root / "jouet").symlink_to(sys.executable)
            with self.assertRaises(HookPreflightError):
                arms.codex_tools(root, True)
            (root / "codex").unlink()
            (root / "codex").write_text("replacement")
            with self.assertRaises(HookPreflightError):
                arms.codex_tools(root, False)

    def test_path_rejects_shadowed_tools_and_jouet_in_the_plain_arm(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bin_dir = root / "bin"
            other = root / "other"
            bin_dir.mkdir()
            other.mkdir()
            (bin_dir / "codex").symlink_to(sys.executable)
            (bin_dir / "jouet").symlink_to(sys.executable)
            env = {"PATH": str(bin_dir)}
            arms.verify_codex_path(bin_dir, env, True)
            with self.assertRaisesRegex(HookPreflightError, "plain"):
                arms.verify_codex_path(bin_dir, env, False)
            (bin_dir / "jouet").unlink()
            (other / "codex").symlink_to(sys.executable)
            (other / "jouet").symlink_to(sys.executable)
            env["PATH"] = os.pathsep.join((str(other), str(bin_dir)))
            with self.assertRaisesRegex(HookPreflightError, "codex"):
                arms.verify_codex_path(bin_dir, env, False)
            env["PATH"] = os.pathsep.join((str(bin_dir), str(other)))
            with self.assertRaisesRegex(HookPreflightError, "plain"):
                arms.verify_codex_path(bin_dir, env, False)

    def test_preparation_creates_a_dedicated_home_without_overwriting_one(self):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory) / "home"
            command = [sys.executable, str(root / "bench/agents.py"), "prepare-codex",
                       "--home", str(home), "--jouet", str(root / "target/debug/jouet")]
            first = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(first.returncode, 0, first.stderr)
            self.assertTrue((home / ".codex/hooks.json").is_file())
            self.assertEqual((home / ".codex/config.toml").read_text(), 'cli_auth_credentials_store = "file"\n')
            self.assertTrue((home / ".jouet-bench-codex-home").is_file())
            self.assertFalse((home / ".codex/auth.json").exists())
            second = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(second.returncode, 0)
            inside = root / f".bench-test-{uuid.uuid4().hex}"
            unsafe = subprocess.run([*command[:3], "--home", str(inside), *command[5:]],
                                    capture_output=True, text=True)
            self.assertNotEqual(unsafe.returncode, 0)
            self.assertFalse(inside.exists())

    def test_suite_preflight_stops_before_any_arm_runs(self):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory) / "reviewed"
            config = home / ".codex"
            config.mkdir(parents=True)
            (home / ".jouet-bench-codex-home").write_text("test\n")
            (config / "hooks.json").write_text("{}")
            args = agents.parse_args(["run", "--harness", "codex", "--tasks", "smoke",
                                      "--arms", "plain,jouet", "--rounds", "1", "--out", str(home / "out"),
                                      "--jouet", str(root / "target/debug/jouet"),
                                      "--codex-reviewed-home", str(config)])
            with patch.object(agents.arms, "resolve", return_value=Path(sys.executable)), \
                    patch.object(agents.arms, "auth_status", return_value="test"), \
                    patch.object(agents.arms, "build", return_value={}), \
                    patch.object(agents.arms, "setup_jouet"), \
                    patch.object(agents.arms, "reviewed_codex", side_effect=HookPreflightError("untrusted")), \
                    patch.object(agents.runner, "run_suite") as suite, \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(agents.cmd_run(args), 4)
                suite.assert_not_called()

    def test_codex_home_cannot_be_changed_by_an_environment_override(self):
        for name in ("CODEX_HOME", "HOME", "PATH", "XDG_STATE_HOME"):
            with self.subTest(name=name):
                args = agents.parse_args(["run", "--harness", "codex", "--tasks", "smoke",
                                          "--jouet-env", f"{name}=/elsewhere"])
                with patch.object(agents.runner, "run_suite") as suite, contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(agents.cmd_run(args), 2)
                    suite.assert_not_called()
