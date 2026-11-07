import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, new_commit
from bats_check import check_bats

build = subprocess.run(["cargo", "build", "--bin", "fnox"], capture_output=True, text=True, check=False)
expect(build.returncode == 0, f"build fails:\n{build.stderr[-3000:]}")
if build.returncode == 0:
    binary = Path.cwd() / "target/debug/fnox"
    with tempfile.TemporaryDirectory(prefix="fnox bench ") as tmp:
        directory = Path(tmp)
        (directory / "fnox.toml").write_text("[secrets]\n")
        editor = directory / "editor"
        editor.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$EDITOR_ARGS"\n')
        editor.chmod(0o755)
        args_file = directory / "args"
        env = {**os.environ, "HOME": tmp, "FNOX_CONFIG_DIR": str(directory / "config"),
               "EDITOR_ARGS": str(args_file)}
        # The executable path and an argument both contain spaces.
        env.update(EDITOR=f"'{editor}' --flag 'two words'", VISUAL="")
        run = subprocess.run([binary, "edit"], cwd=directory, env=env, capture_output=True, text=True, check=False)
        args = args_file.read_text().splitlines() if args_file.exists() else []
        expect(run.returncode == 0 and len(args) == 3 and args[:2] == ["--flag", "two words"],
               f"editor arguments were not preserved: {run.stderr}")
        args_file.unlink(missing_ok=True)
        env.update(EDITOR="", VISUAL=f"'{editor}' --visual")
        run = subprocess.run([binary, "edit"], cwd=directory, env=env, capture_output=True, text=True, check=False)
        args = args_file.read_text().splitlines() if args_file.exists() else []
        expect(run.returncode == 0 and len(args) == 2 and args[0] == "--visual",
               f"empty EDITOR did not fall back to VISUAL: {run.stderr}")
print(f"committed: {new_commit()}")
expect(new_commit(), "nothing was committed")
expect(not dirty(), "tracked changes remain uncommitted")
if build.returncode == 0 and new_commit() and not dirty():
    check_bats()
    expect(not dirty(), "editor tests left tracked changes uncommitted")
finish()
