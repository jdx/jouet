import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, new_commit, run_python, tests_pass, unchanged  # noqa: E402

expect(unchanged("test_wordcount.py"), "test_wordcount.py was changed")
ok, out = tests_pass()
expect(ok, f"the tests fail:\n{out}")
ok, out = run_python("from wordcount import count_words as c; assert c(' a  b ') == 2 and c('\\n') == 0")
expect(ok, f"count_words is wrong on inputs the tests do not cover:\n{out}")
expect(new_commit(), "nothing was committed")
expect(not dirty("wordcount.py"), "the fix was not committed")
finish()
