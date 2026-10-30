---
description: Block until a file, log line, port, URL, process or command is ready, instead of sleep-polling.
---

# jouet wait

`jouet wait` blocks until a condition holds, then prints one summary. It replaces `sleep N; check; repeat` loops, which waste a model turn on every check and are often blocked by the agent's harness.

```text
jouet wait file PATH     [--contains REGEX] [--gone]
jouet wait log PATH      --match REGEX [--from-start]
jouet wait port ADDR     [--closed]
jouet wait http URL      [--status CODE] [--contains REGEX]
jouet wait pid PID
jouet wait cmd [--expect-exit N] -- CMD...

common: --timeout DUR (default 30m)  --interval DUR (default 10s, backs off to 60s)
        --json | --jsonl  --quiet
```

The flags are listed in the [CLI reference](/cli/wait/). Waiting on CI, Actions runs and pull requests (`wait ci`, `wait run` and `wait pr`) is coming next.

## Subjects

- **`wait file PATH`** waits until the path exists. With `--contains REGEX`, it waits until a line of the file matches and prints that line. With `--gone`, it waits until nothing exists at the path.
- **`wait log PATH --match REGEX`** follows the file like `tail -F`, and prints the first appended line that matches. It starts at the current end of the file, so lines already there do not count unless `--from-start` is given. A file that does not exist yet is waited for and then read from its start. When the file is rotated (the path now names a different file) or truncated, it is read again from its start. A line still being written is matched too.
- **`wait port ADDR`** waits until a TCP connection to `ADDR` succeeds, or with `--closed` until every address it resolves to refuses connections. A timeout or an unreachable network says nothing about the port, so it keeps the wait going. `ADDR` is `host:port`, `[v6]:port`, or just a port, which means localhost.
- **`wait http URL`** waits until a GET of the URL answers with a 2xx status, or exactly `--status CODE`. With `--contains REGEX`, a line of the body must match too, and that line is printed. Redirects are followed unless `--status` asks for a 3xx.
- **`wait pid PID`** waits until the process exits. The process does not have to be jouet's child.
- **`wait cmd -- CMD...`** runs the command, directly rather than through a shell, every interval until it exits with `--expect-exit` (default `0`). It prints the last run's exit code and its last 20 output lines, and saves the full output of the last run under the log directory. A run still going at the timeout is stopped, though every run gets at least one interval, so `--timeout 0` runs the command once.

These subjects stay deliberately small. Starting and supervising long-running services, with their ready checks, is [pitchfork](https://pitchfork.jdx.dev)'s job.

## Timing

The first check is immediate. After that, the time between checks starts at `--interval` and grows by half again after each check that saw no change, up to 60 seconds (or `--interval`, if that is longer). It drops back to `--interval` when the state changes. No wait runs past `--timeout`, and there is one last check at it, so `--timeout 0` checks exactly once.

On timeout, jouet exits `124`. Running the same command again keeps waiting.

## Output

```console
$ jouet wait log app.log --match 'Listening on'
[0s] log app.log: missing
[4s] log app.log: matched
MET log app.log: matched (4s)
  Listening on http://localhost:3000
```

- **Progress.** For a person at a terminal, one status line on stderr is redrawn every second. For agents, pipes and CI, each change of state is one line on stderr, so a backgrounded wait's log grows only when something happens.
- **Result.** The first word is `MET` or `TIMEOUT`. A timeout says how many checks ran.
- **`--json`** prints one document: `subject`, `target`, `result` (`met` or `timeout`), the last `state`, `elapsed_s` and `checks`, plus the subject's own fields: `line`, `status`, `exit_code`, `output`, `output_truncated` and `log_path`. Empty fields are left out.
- **`--jsonl`** prints one `{"event":"state",...}` line for each change of state, then the result document tagged `"event":"result"`. Under Claude Code's Monitor tool, each line becomes a notification.
- **`--quiet`** prints no progress and no text result, only the exit code. JSON is still printed when it is asked for.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The condition was met |
| `2` | Bad arguments, such as an invalid duration or regex |
| `3` | `wait cmd`'s program does not exist |
| `124` | Still waiting at `--timeout`; run the same command again to keep waiting |
