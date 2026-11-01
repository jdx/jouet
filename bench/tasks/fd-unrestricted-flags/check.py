import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

build = subprocess.run(["cargo", "build", "--bin", "fd"], capture_output=True, text=True)
expect(build.returncode == 0, f"build fails:\n{build.stderr[-3000:]}")
if build.returncode == 0:
    binary = Path.cwd() / "target/debug/fd"
    with tempfile.TemporaryDirectory(prefix="fd-bench-") as tmp:
        directory = Path(tmp)
        for name in ("a.txt", ".hidden.txt", "ignored.txt"):
            (directory / name).touch()
        (directory / ".ignore").write_text("ignored.txt\n")
        env = {**os.environ, "HOME": tmp, "XDG_CONFIG_HOME": str(directory / "config")}
        cases = [
            (["-u", "--ignore"], {"a.txt", ".hidden.txt", ".ignore"}),
            (["-u", "--no-hidden"], {"a.txt", "ignored.txt"}),
            (["--ignore", "-u"], {"a.txt", ".hidden.txt", ".ignore", "ignored.txt"}),
            (["--no-hidden", "-u"], {"a.txt", ".hidden.txt", ".ignore", "ignored.txt"}),
        ]
        for flags, want in cases:
            run = subprocess.run([binary, "--color=never", "--type", "f", *flags], cwd=directory,
                                 env=env, capture_output=True, text=True)
            got = set(run.stdout.splitlines())
            expect(run.returncode == 0 and got == want,
                   f"{flags}: expected {sorted(want)}, got {sorted(got)}; stderr: {run.stderr}")
print(f"committed: {new_commit()}")
finish()
