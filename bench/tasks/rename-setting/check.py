import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import committed_files, dirty, expect, finish, git, new_commit, run_python, tests_pass, unchanged  # noqa: E402

left = subprocess.run(
    ["git", "grep", "-n", "-e", "retry_limit", "-e", "retry-limit", "HEAD", "--", ".", ":!CHANGELOG.md"],
    capture_output=True,
    text=True,
).stdout.strip()
expect(not left, f"the old name is still committed:\n{left}")
for path in ("shipit/config.py", "shipit/client.py", "shipit/cli.py", "shipit/upload.py", "README.md"):
    expect("max_attempts" in git("show", f"HEAD:{path}"), f"{path} does not use max_attempts")
expect("--max-attempts" in git("show", "HEAD:README.md"), "README.md does not document --max-attempts")
expect(unchanged("CHANGELOG.md"), "CHANGELOG.md was changed")
ok, out = tests_pass("discover", "-s", "tests")
expect(ok, f"the tests fail:\n{out}")
ok, out = run_python(
    "import io, contextlib\n"
    "from shipit import cli, config\n"
    "assert config.load(overrides={'max_attempts': 4})['max_attempts'] == 4\n"
    "out = io.StringIO()\n"
    "with contextlib.redirect_stdout(out):\n"
    "    cli.main(['--max-attempts', '6', '--print-config'])\n"
    "assert out.getvalue().strip() == 'max_attempts=6 timeout_s=30', out.getvalue()\n"
)
expect(ok, f"the renamed setting or flag does not work:\n{out}")
expect(new_commit(), "nothing was committed")
stray = sorted(f for f in committed_files() if f.startswith(".scratch/") or "__pycache__" in f)
expect(not stray, f"the commit includes files that are not part of the change: {stray}")
expect(not dirty(), f"changes were left uncommitted: {dirty()}")
finish()
