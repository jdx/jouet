import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, new_commit, run_python, tests_pass, unchanged  # noqa: E402

expect(unchanged("tests"), "the tests were changed")
ok, out = tests_pass("discover", "-s", "tests")
expect(ok, f"the tests fail:\n{out}")
# Cases the tests do not spell out, so hard-coding their answers fails.
ok, out = run_python(
    "import textstats as ts\n"
    "assert ts.word_frequencies('Dog dog DOG') == {'dog': 3}\n"
    "assert ts.top_words('z y x y z', 3) == ['y', 'z', 'x'], ts.top_words('z y x y z', 3)\n"
    "assert ts.reading_time('w ' * 400) == 2 and ts.reading_time('w ' * 401) == 3\n"
    "assert ts.sentence_count('Hm?! Ok.. Fine') == 2\n"
)
expect(ok, f"textstats is wrong on inputs the tests do not cover:\n{out}")
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"changes were left uncommitted: {dirty()}")
finish()
