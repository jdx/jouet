import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# Test the public Command API without replacing the agent's tests.
program = r'''
import click

cases = [
    ("Weigh apples vs. pears.", 30, "Weigh apples vs. pears."),
    ("Choose e.g. apples and pears.", 40, "Choose e.g. apples and pears."),
    ("Continue here. another word follows.", 50, "Continue here. another word follows."),
    ("First sentence. Second sentence.", 40, "First sentence."),
    ("Pick a fruit. 3 remain.", 40, "Pick a fruit."),
    ("First line\ncontinues.\n\nSecond paragraph.", 45, "First line continues."),
    ("Weigh apples vs. pears and plums.", 20, "Weigh apples vs...."),
    ("ab cd", 2, "..."),
]
for text, limit, expected in cases:
    actual = click.Command("example", help=text).get_short_help_str(limit)
    if actual != expected:
        raise AssertionError(f"{text!r}: got {actual!r}, expected {expected!r}")
assert click.Command("example", help="Long text", short_help="Exact.").get_short_help_str() == "Exact."
'''
run = subprocess.run(["python3", "-c", program], capture_output=True, text=True,
                     env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")}, timeout=60)
expect(run.returncode == 0, f"short-help cases failed:\n{run.stdout}\n{run.stderr}")
print(f"committed: {new_commit()}")
finish()
