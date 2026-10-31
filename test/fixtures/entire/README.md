# Entire CLI fixtures

What `entire` 0.11.4 prints with `--json`, as `test/entire.bats`'s fake
`entire` replays it. The shapes were recorded from repositories on a
development machine; IDs are kept, prompts are shortened, and paths that must
point into the test's repository are written `@WORK@`.

- `status-enabled.json`, `status-disabled.json`: `entire status --json` in an
  enabled repository and in one Entire is not set up in.
- `auth-ok.json`: `entire auth status --json` with a working login. Its
  `token_source` is a stand-in that jouet must never print.
- `auth-logged-out.json`: the same without a login. Not recorded (the
  machine's login was left alone): it keeps the recorded shape with
  `logged_in` false, the one field jouet reads.
- `session-list.json`: `entire session list --json`.
- `checkpoint-list.json`: `entire checkpoint list --json`: committed
  checkpoints (ULIDs, with `agent` and `session_ids`) and ones not condensed
  yet (40 hex digits, whose `message` is the prompt).
- `why-wait_port.sh.json`: `entire why wait_port.sh --json`, the whole-file
  form, with a credential planted in a prompt to prove it is redacted.
