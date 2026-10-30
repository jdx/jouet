import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, git, github_state, new_commit, run_python, tests_pass  # noqa: E402

source = git("show", "HEAD:invoice.py")
expect("subtotal" in source and "tmp" not in source, "invoice.py still calls it tmp, or never says subtotal")
ok, out = tests_pass("discover", "-s", "tests")
expect(ok, f"the tests fail:\n{out}")
ok, out = run_python(
    "from decimal import Decimal\n"
    "from invoice import average_line\n"
    "assert average_line([]) == Decimal('0.00'), average_line([])\n"
    "assert average_line([('a', 1, '1.005'), ('b', 1, '2')]) == Decimal('1.51')\n"
)
expect(ok, f"average_line is wrong:\n{out}")
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"changes were left uncommitted: {dirty()}")

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 12)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    comments = threads[thread_id]["comments"]["nodes"]
    return any(c["author"]["login"] == viewer for c in comments[1:])


for thread_id, what in (("PRRT_kwDOdemo1", "the rename"), ("PRRT_kwDOdemo2", "the empty-invoice fix")):
    expect(replied(thread_id), f"no reply on the thread asking for {what}")
    expect(threads[thread_id]["isResolved"], f"the thread asking for {what} was not resolved")
expect(replied("PRRT_kwDOdemo3"), "the question about rounding got no answer")
expect(not threads["PRRT_kwDOdemo3"]["isResolved"], "the question about rounding was resolved")
old = threads["PRRT_kwDOdemo0"]
expect(old["isResolved"] and len(old["comments"]["nodes"]) == 1, "the thread that was already resolved was changed")
finish()
