# jouet

A toolkit CLI for coding agents.

> [!CAUTION]
> **jouet is in early development.** Only `jouet edit` and `jouet wait`'s local subjects are available so far. Their
> flags, exit codes and JSON output are not finalized and may change incompatibly between releases.

Coding agents spend a large share of their shell calls rebuilding the same few chores by hand,
and they get them subtly wrong: sleep-polling CI, hand-rolled GraphQL for PR review threads,
`for` loops over `gh pr view`, and Python heredocs that patch files with `str.replace`. jouet's
first four commands turn each of those into one command (`edit` and `wait` for local conditions have landed; the others are being built):

| Command | Replaces |
|---|---|
| `jouet wait` | `sleep`/`until` loops around `gh pr checks` and `gh run view`; reports conflicted PRs that will never get CI. Also waits on files, logs, ports, URLs, processes and commands |
| `jouet pr threads` | raw `reviewThreads` queries, replies and resolves |
| `jouet pr status` | per-PR loops over `gh pr view`, including `mergeable: UNKNOWN` handling |
| `jouet edit` | heredoc edits with hand-written `assert s.count(old) == 1`; all-or-nothing, never partially written |

Each command is designed to work without a TTY and never prompt. Output adapts to who is
reading (an agent, a human at a terminal, or a pipe), `--json` prints exactly one token-lean
document, lists are never truncated silently, and every command shares one exit-code table.

## Building

```sh
cargo install --path .
jouet --help
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup.

## License

[MIT](LICENSE)
