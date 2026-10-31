---
description: What jouet promises not to break, what it may change in any release, how agents should read its output, and how something is deprecated before it goes.
---

# Stability and deprecations

jouet is pre-1.0. Until 1.0, any release may break anything on this page, and some will. What changes at 1.0 is that the promise below starts to hold. Until then, jouet still follows it where that costs little, and says so plainly when it does not.

## What jouet promises

The promise covers what a script or an agent can depend on:

- commands and subcommands, and their flags and positional arguments;
- exit codes;
- `--json` and `--jsonl` documents;
- the keys in `~/.config/jouet/config.toml` and the `JOUET_*` environment variables;
- the names of the built-in [rules](/rules), which `[rules] disable` lists.

### Breaking changes

After 1.0 these happen only in a new major version, after a [deprecation](#deprecations):

- Removing or renaming a command, subcommand, flag, config key, environment variable or built-in rule.
- Removing or renaming a JSON field, or changing its type: a string that becomes an object, a number that becomes a string, a single value that becomes a list.
- Changing what an exit code means, or which exit code an outcome gets, such as a refusal moving from `1` to `4`.
- Removing a value from a field that takes one of a fixed set, such as a `status` that can no longer be `flaky`.
- Making an optional flag required, or changing a default so the same command line does something else.
- Writing anything but the result to stdout.

### Not breaking

These can happen in any release:

- A new command, a new optional flag, a new JSON field, a new config key or environment variable.
- A new value in a field that takes one of a fixed set.
- A field that used to be left out when empty now being present. A missing field already means its zero value.
- Any change to text output, in every [display mode](/conventions#display-modes): wording, layout, order, colour, and where a list is cut short.
- Anything on stderr, including warnings and deprecation notes.
- Hidden flags, which `--help` does not show, and `jouet hook`, which only a harness calls.
- The ledger and the other state files under `~/.local/state/jouet`.
- The session primer, `jouet agent-help` and the skill.
- New rules, and what a rule's message says.

## Reading jouet's output

To read jouet's output from a script or an agent:

- Pass `--json`, and read fields, not text. Text output is for reading, and changes in any release.
- Ignore fields you do not know. New ones appear without notice.
- Treat a missing field as its zero value: `null`, `false`, `0` or empty.
- Handle a value you do not know in a field that takes one of a fixed set.
- Decide on the [exit code](/conventions#exit-codes) first, then on the document.

```sh
jouet pr status 123 --json | jq -r '.prs[0].state // "unknown"'
```

## Deprecations

When something covered by the promise has to change, the old form keeps working for a while and warns each time it is used. Nothing is deprecated yet; a note would look like this:

```output
jouet: deprecated [wait-poll]: `--poll` is now `--interval` (removed in jouet 2.0.0)
```

- The note is one line on stderr, so it never breaks a `--json` pipe. A person at a terminal sees it as `jouet: warning: …`.
- An agent session is told once, not on every command: the [ledger](/setup#the-ledger) records each deprecation it has seen, and later commands in that session stay quiet. Outside an agent session, each run says it once.
- The note names the release it is removed in.
- The [changelog](https://github.com/jdx/jouet/blob/main/CHANGELOG.md) lists the deprecation when it starts, and lists the removal as a breaking change when it happens.

After 1.0, something deprecated is removed in the first major release at least 12 months after the release that started warning. Before 1.0 the warning still comes first where keeping the old form is cheap, and 12 months is the aim rather than a rule.
