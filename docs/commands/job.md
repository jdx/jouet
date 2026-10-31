---
description: Run a build, a test suite, a long script or a server for one check in the background, with a ready check, a log and a name to stop it by.
---

# jouet job

`jouet job` runs one-off work in the background: a build, a test suite, a long script, or a server started for one check. It replaces `cmd &`, `nohup cmd > log 2>&1 &`, the `sleep 5 && curl ...` loop that follows, `tail -f` on the log, and `kill $(lsof -ti :3000)` at the end.

```text
jouet job start ID [--port N | --http URL | --output REGEX] [--timeout DUR] [--replace] -- CMD...
jouet job ls
jouet job logs ID [-n N]
jouet job stop ID... [--grace DUR]
jouet wait job ID [--until exited|ready] [--port N | --http URL] [--log-lines N] [--no-logs]
```

No daemon runs. `start` spawns the command and returns; each job is a small JSON file under jouet's state directory, beside its log. Jobs belong to the work tree they were started in: `jouet job ls` in another worktree does not see them. For a project's long-running services, the ones meant to keep running between sessions, use [pitchfork](https://pitchfork.jdx.dev).

## Before and after

An agent starts a dev server, guesses how long it takes, then checks:

```sh
python3 -m http.server 28961 > server.log 2>&1 &
sleep 5 && curl -s localhost:28961
```

If the server comes up in one second, four are wasted; if it takes ten, the check fails and the agent loops. If it never comes up, the loop never ends, and the reason is in a log the agent then has to find. `jouet job start` returns as soon as the ready check passes:

```output
$ jouet job start web --port 28961 -- python3 -u -m http.server 28961 --bind 127.0.0.1
[0s] job web: running, port localhost:28961 closed
[0s] job web: running, port localhost:28961 open
STARTED job web (pid 2304182): running, port localhost:28961 open (0s)
  log: ~/.local/state/jouet/jobs/43c333e456351478/web.log
  next: `jouet job logs web`, `jouet wait job web`, `jouet job stop web`
```

And when the job exits before it is ready, it fails at once with the end of its log:

```output
$ jouet job start web --port 28961 -- python3 -m http.server 28961
[0s] job web: running, port localhost:28961 closed
[0s] job web: exited 1
FAILED job web: exited 1 after 0s
  Traceback: OSError: [Errno 98] Address already in use
  full log: ~/.local/state/jouet/jobs/43c333e456351478/web.log
```

## `job start`

`job start ID -- CMD...` runs CMD directly (not through a shell) in the current directory, detached in a session and process group of its own, with stdin closed and stdout and stderr appended to the job's log. `ID` is letters, digits, `_`, `.` and `-`.

- **Ready checks.** `--port N` waits until localhost's port N accepts connections, `--http URL` until the URL answers with a 2xx status, and `--output REGEX` until a line of the job's log matches. It waits up to `--timeout` (default `60s`). A job that exits first fails the start with exit `1` and the end of its log. A job still not ready at the timeout keeps running, and the start exits `124`; `jouet wait job ID --until ready` keeps waiting.
- **Without a ready check**, `start` returns at once, unless the command fails within a moment of starting, such as a program that does not exist (exit `127`).
- **One job per ID.** While a job of the same ID is running, `start` refuses with exit `4`. `--replace` stops it first.
- **Exit codes are kept.** The command runs under `sh`, which writes its exit status beside the log when it ends, so `ls` and `wait job` know how a job ended after jouet has gone.

## `wait job`

`wait job ID` blocks until the job ends: exit `0` when it exited 0 or `job stop` stopped it, and exit `1`, with the end of its log, when it exited with another code. It takes `wait`'s `--timeout`, `--interval`, `--json`, `--jsonl` and `--quiet`.

```output
$ jouet wait job test --interval 1s --log-lines 4
[0s] job test: running
[1s] job test: exited 101
FAILED job test: exited 101 after 1s
  ... 38 lines skipped
  test case_39 ... ok
  test case_40 ... ok
  test case_41 ... FAILED
  thread case_41 panicked at src/lib.rs:12:5
  full log: ~/.local/state/jouet/jobs/43c333e456351478/test.log
```

The log is cut like `wait ci`'s excerpts: the last `--log-lines` lines (default `60`; `0` keeps all of it), plus earlier lines matching `error|FAILED|panicked|assert` with two lines of context, with every gap marked. `--until ready` waits for the job's ready check instead (`--port` or `--http`, else the one it was started with); a job that ends first fails. A work tree without such a job is exit `3`.

## `job ls`, `job logs` and `job stop`

- **`job ls`** lists the work tree's jobs, oldest first, as `running`, `exited N`, `stopped`, or `gone` (no longer running, with nothing recorded about how it ended). Jobs that ended more than a week ago are removed. `--json` prints each job's `id`, `state`, `exit_code`, `pid`, `command`, `cwd`, `log`, `started_at` and `ready`.
- **`job logs ID`** prints the last 50 lines of the job's log (`-n N`; `0` prints all of it), after a line saying how many earlier lines there are and where the full log is.
- **`job stop ID...`** sends SIGTERM to each job's whole process group, so the children it started stop too, and SIGKILL after `--grace` (default `5s`). A job that has already ended is left as it is. Every ID is checked before anything is signalled; an unknown one is exit `3`.

## Which process is the job

A PID can be reused once its process has exited. jouet trusts a job to still be running, and signals it, only while its PID leads the process group it was started in and has the start time recorded when it was spawned. A process that only shares the PID is reported `gone` and left alone.

## Handoff, pickup and the stop hook

[`jouet handoff`](/commands/handoff) records the work tree's jobs, and `jouet pickup` says which have ended since. When an agent stops, [the stop hook](/setup#finishing-on-a-failing-check) names the jobs that session started that are still running, as a note: it never blocks on them.

## Platforms

Jobs run on Linux and macOS. On Windows, `start` and `stop` refuse with exit `4`; use the harness's own background tasks there.
