---
description: How jouet's hooks notice costly patterns in a local ledger and draft rules against them, in warn mode, for an agent to adopt, test, promote or retire.
---

# Self-winding

After `jouet setup`, jouet improves itself the way agents keep memories. Its hooks keep a [ledger](/setup#the-ledger) of command shapes on this machine. When the same costly pattern keeps showing up there, jouet drafts a [rule](/rules) against it, and the next session start shows a one-line note about it. The agent, or you, decides whether to adopt the rule. Nothing leaves the machine, and every change to a rule is logged.

::: warning Early development
Winding is new. The patterns it notices, and its thresholds, will change between releases.
:::

## The loop

1. **Notice.** At the end of each turn and at session start, `jouet wind` reads the ledger from where it last stopped. It looks for two patterns:
   - **A command that never works:** the same program, subcommands and flags failing at least three times, across at least two sessions, and never succeeding, such as a tool that is not installed. Test, lint and build runs are left out, because they fail while code is being fixed.
   - **Long sleeps:** at least five sleeps of ten seconds or more, across at least two sessions, where `jouet wait` would wait for the thing itself.
2. **Draft.** For each pattern, it writes a rule against the public `jouet` API. The rule is in warn mode, and its examples are the shapes that motivated it plus the near misses it must allow. It keeps the draft as a note, and drafts at most two notes a day.
3. **Show.** A session start shows at most two pending notes, one line each, after jouet's primer:

   ```text
   jouet wind noticed: `mytool sync --all` failed 4 times in 2 sessions (last exit 127, not found) and never worked. It drafted rule `wind-mytool-sync-fails` (warn mode): `jouet wind show n1` to read it, `jouet rule new wind-mytool-sync-fails --from-note n1 --author s-42` to adopt it, `jouet wind dismiss n1` to drop it.
   ```

   A note shown to three sessions without being adopted lapses.
4. **Adopt.** `jouet rule new NAME --from-note ID` writes the rule to `~/.config/jouet/rules/NAME.luau`, with the session that adopted it as its author.
5. **Test.** `jouet rule test NAME` runs its examples. `jouet rule test NAME --replay` runs it over every call in the ledger, lists the calls it fires on and the near misses it leaves alone, and fails if it fires on nothing.
6. **Promote.** A rule stays in warn mode until `jouet rule promote NAME` switches it to deny. With `auto_promote` on, a rule that has fired `promote_after` times, in at least two sessions, with no overrides, is promoted on its own.
7. **Unwind.** `jouet unwind` proposes retiring rules that are nearly always overridden, fire on a large share of calls, or no longer load. `jouet unwind --apply` moves your rules to `~/.config/jouet/rules/retired/`.

`jouet rule log` lists every change (created, promoted, retired), when, by whom and why.

## Guardrails

- **Warn first.** A rule jouet writes starts in warn mode. Promotion to deny takes `jouet rule promote` or, when you turn it on, a clean record.
- **Your rules only.** Winding writes, promotes and retires rules in `~/.config/jouet/rules` and nothing else. Built-in rules are never proposed for retirement, and none of yours can replace or overrule one (see [the floor](/rules#where-rules-come-from)). A repository's rules run only once trusted, and change through a reviewed PR.
- **Few notes.** At most two notes per session start and two new drafts a day.
- **The sandbox.** Every rule fails open and runs within its budget, whoever wrote it.
- **Plain files.** The notes are in `$XDG_STATE_HOME/jouet/wind.json` and the log is in `$XDG_STATE_HOME/jouet/rule-log.jsonl`.

## Replaying shapes

The ledger keeps shapes, not commands, so `--replay` shows a rule each call with its values as placeholder words: `git commit -m "fix it"` is replayed as `git commit -m ARG`. A rule that matches on programs, subcommands and flags replays exactly. A rule that reads a value sees `ARG` (the drafted sleep rule treats a length it cannot read as long, so it fires on every replayed sleep).

## Settings

```toml
[wind]
enabled = true        # JOUET_WIND: draft rules and show notes
auto_promote = false  # JOUET_WIND_AUTO_PROMOTE: promote clean rules to deny on their own
promote_after = 20    # JOUET_WIND_PROMOTE_AFTER: firings, with no overrides, before that
```

`JOUET_WIND=0` turns winding off. The ledger itself has its own switch, `[hooks] ledger = false`.
