---
description: The display modes, JSON output, exit codes and logging rules every jouet command shares.
---

# Shared conventions

Every jouet command follows these rules, so an agent that has learned one command already knows how the others behave.

## No prompts, no TTY

No command ever asks a question or needs a terminal. jouet runs the same way under an agent harness, in CI, in a pipe, or in your shell.

## Display modes

Each run writes for one of three readers:

| Mode | For | Text output | `--json` output |
|---|---|---|---|
| `agent` | A coding agent | Compact, one line per item, no colour | Minified |
| `human` | A person at a terminal | Aligned and readable | Pretty-printed |
| `plain` | Pipes, redirects and CI | Like `agent` | Minified |

jouet picks the mode from the first rule that applies:

1. The `--display agent|human|plain` flag.
2. The `JOUET_DISPLAY` environment variable.
3. `agent` when a coding agent is detected.
4. `human` when stdout is a terminal.
5. `plain` otherwise.

Agent detection is mise's [`mise-agent-env`](https://crates.io/crates/mise-agent-env) crate, so mise, [mr boxington](https://mr-boxington.jdx.dev) and jouet agree on who counts as an agent. It reads the vendor-neutral `AI_AGENT` variable and the variables individual agents set, such as `CLAUDECODE` and `CODEX_THREAD_ID`. `AI_AGENT=0` turns detection off.

Detection comes before the terminal check on purpose: a terminal does not prove a person is reading. Codex can run commands under a PTY, and so can a terminal pane an agent drives.

## JSON output

`--json` prints exactly one JSON document to stdout, errors included, so `jouet … --json | jq` always has something to parse:

```json
{"error":{"code":"usage","message":"--revert-on-fail needs --check"}}
```

- Field names are `snake_case`.
- Fields that are `null`, `false` or empty are left out, to keep documents short. A missing field means its zero value.
- Documents carry no schema version. Fields are added, not renamed or removed, except in a new major version of jouet, so the jouet version is the schema version.
- Only `human` mode pretty-prints. Every mode emits the same fields.

The `code` of an error is always one of the names in the [exit code table](#exit-codes).

## stdout and stderr

Only the result goes to stdout. Without `--json`, an error that stops a command is printed to stderr as `jouet: <message>`.

## Exit codes

One table covers every command:

| Code | Name | Meaning |
|---|---|---|
| 0 | `ok` | The command succeeded, or the condition was met |
| 1 | `failed` | The work was done and the answer is no: a check failed, or a condition can no longer be met |
| 2 | `usage` | Bad arguments or a malformed spec |
| 3 | `not_found` | The file, PR, thread or run does not exist |
| 4 | `precondition` | Refused before doing anything: an edit did not match, or a file changed since it was read |
| 5 | `remote` | Authentication, network or rate-limit failure talking to the host |
| 124 | `timeout` | Still pending when the timeout expired |
| 130 | `interrupted` | Stopped by a signal |

`1` and `4` are deliberately separate. After a `1`, fix the code: jouet did what was asked, and the result is a failure. After a `4`, fix the request: jouet refused, and nothing changed.

## Nothing truncated silently

When jouet shortens a list or a log, it says so and says where the rest is. For example, `jouet edit --check` keeps the last 30 lines of the check's output, marks the cut, and prints the path to the full log:

```output
check failed (exit 101): cargo check -q
  ...
  error: could not compile `demo` (bin "demo") due to 1 previous error
  full log: /home/me/.local/state/jouet/logs/edit-check-1793373365910-3807454.log
```

With `--json` the same report sets `"output_truncated":true` and carries `log_path`.

## Full logs are kept

Output that jouet summarizes is saved in full under the first of these that is set:

1. `$JOUET_LOG_DIR`
2. `$XDG_STATE_HOME/jouet/logs`
3. `~/.local/state/jouet/logs`
4. `jouet/logs` under the system temporary directory

The path appears in both text and JSON output.
