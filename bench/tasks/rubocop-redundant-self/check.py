import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import committed_files, dirty, expect, finish, new_commit  # noqa: E402

# Exercise the public CLI independently of the agent's spec changes. A real
# redundant receiver must still be reported, including on the right-hand side.
cases = [
    ("self.a, *self.b = c", 0),
    ("*self.a, self.b = c", 0),
    ("a, (self.b, *self.c) = d", 0),
    ("a, b = *self.c", 1),
    ("self.c", 1),
]
for source, expected in cases:
    run = subprocess.run(
        ["bundle", "exec", "rubocop", "--config", "/dev/null", "--only", "Style/RedundantSelf",
         "--format", "json", "--cache", "false", "--stdin", "case.rb"],
        input=source + "\n", capture_output=True, text=True, timeout=60,
    )
    try:
        doc = json.loads(run.stdout)
        offenses = [o for f in doc["files"] for o in f["offenses"]
                    if o["cop_name"] == "Style/RedundantSelf"]
        expect(len(offenses) == expected and run.returncode in (0, 1),
               f"{source}: expected {expected} offenses, got {len(offenses)} (exit {run.returncode})")
    except (ValueError, KeyError, TypeError):
        expect(False, f"{source}: RuboCop did not produce a usable report (exit {run.returncode}): {run.stderr}")

files = committed_files()
expect(new_commit(), "nothing was committed")
expect("spec/rubocop/cop/style/redundant_self_spec.rb" in files, "no regression spec was committed")
expect(any(p.startswith("changelog/") and p.endswith(".md") for p in files), "no changelog entry was committed")
expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
finish()
