import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import (
    UNVERIFIED,
    dirty,
    expect,
    finish,
    git,
    github_state,
    head_sha,
    new_commit,
)

# What the agent committed, before checking an isolated copy.
expect(new_commit(), "nothing was committed")
left = [path for path in dirty() if path != "mise.lock"]
expect(not left, f"changes were left uncommitted: {left}")

# The docs thread: the namespace page now says an explicit top-level
# `namespace` overrides the directory-derived one.
added = [line[1:] for line in git("diff", head_sha(), "HEAD", "--", "docs/concepts/namespaces.md").splitlines()
         if line.startswith("+") and not line.startswith("+++")]
expect(any(re.search(r"explicit|top-level|overrid", line, re.IGNORECASE) and "namespace" in line for line in added),
       "docs/concepts/namespaces.md does not say that an explicit `namespace` overrides worktree isolation")

def check_code() -> None:
    # A killed checker must never leave hidden tests in the agent's checkout.
    # CI's failure is reproduced with a symlinked TMPDIR, in a separate clone
    # and build target of the exact commit the agent produced.
    with tempfile.TemporaryDirectory(prefix="pf-check-") as directory:
        scratch = Path(directory)
        checkout = scratch / "checkout"
        try:
            subprocess.run(["git", "clone", "--quiet", "--shared", "--no-checkout", "--", str(Path.cwd()), str(checkout)],
                           check=True, capture_output=True, text=True, timeout=60)
            subprocess.run(["git", "-C", str(checkout), "checkout", "--quiet", "--detach", git("rev-parse", "HEAD").strip()],
                           check=True, capture_output=True, text=True, timeout=60)
            for path in (checkout / "ui", checkout / "ui/dist"):
                if path.is_symlink():
                    UNVERIFIED.append(f"checker setup path is a symlink: {path.relative_to(checkout)}")
                    return
            (checkout / "ui/dist").mkdir(parents=True, exist_ok=True)
            placeholder = checkout / "ui/dist/index.html"
            if placeholder.is_symlink():
                UNVERIFIED.append("checker setup file is a symlink: ui/dist/index.html")
                return
            placeholder.write_text("<html><body>placeholder</body></html>\n")
            (scratch / "real").mkdir()
            (scratch / "tmp").symlink_to("real")
            env = {**os.environ, "TMPDIR": str(scratch / "tmp"), "CARGO_TERM_COLOR": "never",
                   "CARGO_TARGET_DIR": str(scratch / "target")}

            def cargo_test(*args: str) -> tuple[bool, str, int]:
                try:
                    proc = subprocess.run(["cargo", "test", "--lib", *args], cwd=checkout, check=False,
                                          capture_output=True, text=True, env=env, timeout=1500)
                except subprocess.TimeoutExpired as error:
                    # TimeoutExpired carries bytes even when text=True. A timeout
                    # alone is inconclusive; a reported test failure is not.
                    output = b"\n".join(part for part in (error.stdout, error.stderr) if part).decode(errors="replace")
                    if re.search(r"(?m)^test .+ \.\.\. FAILED$|^test result: FAILED\.|^thread .+ panicked at", output):
                        return False, f"test failed before timing out:\n{output[-3000:]}", 0
                    raise
                out = proc.stdout + proc.stderr
                failed = [line for line in out.splitlines() if "FAILED" in line or "panicked" in line or line.startswith("error")]
                passed = sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", proc.stdout))
                return proc.returncode == 0, "\n".join(failed[:20]) + "\n" + proc.stdout[-2000:] + proc.stderr[-1000:], passed

            ok, out, _ = cargo_test()
            expect(ok, f"the lib tests fail with a symlinked TMPDIR, as CI's Windows runner failed them:\n{out}")
            hidden = Path(__file__).parent / "hidden"
            for name, target in (("pitchfork_toml.rs", "src/pitchfork_toml.rs"), ("worktree.rs", "src/proxy/worktree.rs")):
                relative = Path(target)
                path = checkout / relative
                if (not path.is_file() or path.is_symlink()
                        or any((checkout / parent).is_symlink() for parent in relative.parents if parent != Path("."))):
                    expect(False, f"hidden-test target is missing or a symlink: {target}")
                    return
                with path.open("a") as f:
                    f.write("\n" + (hidden / name).read_text())
            ok, out, ran = cargo_test("bench_hidden")
            expect(ok, f"the hidden tests fail (a symlinked start dir, and canonical paths under a symlinked TMPDIR):\n{out}")
            expect(not ok or ran == 4, f"expected the 4 hidden tests to run, {ran} passed")
        except (OSError, subprocess.SubprocessError) as error:
            UNVERIFIED.append(f"could not check the isolated pitchfork checkout: {error}")


check_code()

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 737)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    return any(c["author"]["login"] == viewer for c in threads[thread_id]["comments"]["nodes"][1:])


for thread_id, what in (("PRRT_kwDOdemo1", "canonicalizing the start dir"), ("PRRT_kwDOdemo2", "the namespace docs")):
    expect(replied(thread_id), f"no reply on the thread asking for {what}")
    expect(threads[thread_id]["isResolved"], f"the thread asking for {what} was not resolved")
# Greptile's suggestion to move discovery onto a blocking worker: upstream
# declined it, as the existing callers are synchronous. Either a reply saying
# why or the change will do, so only the reply is required.
expect(replied("PRRT_kwDOdemo3"), "the thread about blocking filesystem calls got no reply")
finish()
