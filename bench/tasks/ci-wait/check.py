import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, new_commit, run_python, tests_pass, unchanged  # noqa: E402

expect(unchanged("tests"), "the tests were changed")
ok, out = tests_pass("discover", "-s", "tests", env={"CI": "true"})
expect(ok, f"the tests fail as CI runs them (CI=true):\n{out}")
ok, out = run_python(
    "from amounts import parse_amount as p\n"
    "assert p('(1,234.50)') == -1234.5 and p('2,000') == 2000.0 and p('-0.5') == -0.5\n"
)
expect(ok, f"parse_amount is wrong on inputs the tests do not cover:\n{out}")
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"changes were left uncommitted: {dirty()}")
finish()
