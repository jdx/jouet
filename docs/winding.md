---
description: How jouet notices costly patterns, in a local ledger of command shapes and in reviews of whole sessions, and proposes rules, mise tasks and notes for an agent to adopt, test, promote or retire.
---

# Self-winding

After `jouet setup`, jouet improves itself the way agents keep memories. It works in two layers:

- **The ledger.** Its hooks keep a [ledger](/setup#the-ledger) of command shapes on this machine, cheaply and all the time. When the same costly pattern keeps showing up there, jouet drafts a [rule](/rules) against it, and the next session start shows a one-line note about it.
- **Session review.** Now and then, an agent reads a digest of a whole session: what the user corrected, approaches tried and abandoned, waits, exploration repeated. It proposes mise tasks, rules, routines, or AGENTS.md and memory notes, which surface the same way. See [Reviewing whole sessions](#reviewing-whole-sessions).

The agent, or you, decides whether to adopt what either layer proposes. Nothing leaves the machine, and every change to a rule is logged.

::: warning Early development
Winding is new. The patterns it notices, and its thresholds, will change between releases.
:::

## The loop

1. **Notice.** At the end of each turn and at session start, `jouet wind` reads the ledger from where it last stopped. It looks for two patterns:
   - **A command that never works:** the same program, subcommands and flags failing at least three times, across at least two sessions, and never succeeding, such as a tool that is not installed. Test, lint and build runs are left out, because they fail while code is being fixed.
   - **Long sleeps:** at least five sleeps of ten seconds or more, across at least two sessions, where `jouet wait` would wait for the thing itself.
2. **Draft.** For each pattern, it writes a rule against the public `jouet` API. The rule is in warn mode. Its `fires` examples are the calls that motivated it. Its `quiet` examples are near misses it must leave alone: what worked instead, and calls in the ledger running the same program. A draft that fails its own examples is dropped. It keeps the draft as a note, and drafts at most two notes a day.
3. **Show.** A session start shows at most two pending notes, one line each, after jouet's primer:

   ```text
   jouet wind noticed: `mytool <arg> --all` failed 4 times in 2 sessions (last exit 127, not found) and never worked. It drafted rule `wind-mytool-fails` (warn mode): `jouet wind show n1` to read it, `jouet rule new wind-mytool-fails --from-note n1 --author s-42` to adopt it, `jouet wind dismiss n1` to drop it.
   ```

   A note shown to three sessions without being adopted lapses.
4. **Adopt.** `jouet rule new NAME --from-note ID` writes the rule to `~/.config/jouet/rules/NAME.toml`, with the session that adopted it as its author. Most drafts are a matcher: the command that kept failing, with exactly the options it failed with. A draft about long sleeps also writes `NAME.luau`, a script that compares durations, which a matcher cannot.
5. **Test.** `jouet rule test NAME` runs its examples. `jouet rule test NAME --replay` runs it over every call in the ledger, lists the calls it fires on and the near misses it leaves alone, and fails if it fires on nothing.
6. **Promote.** A rule stays in warn mode until `jouet rule promote NAME` switches it to deny, which it does only for a rule that passes examples of both kinds. With `auto_promote` on, a rule that has fired `promote_after` times, in at least two sessions, with no overrides, is promoted on its own.
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

### Compound commands

A loop, a conditional, a `case`, a subshell, a `{ }` group or a function keeps its structure in the ledger: its reserved words and brackets, and each command in its bodies, shaped like any other. What belongs to the construct itself is a value, and becomes a placeholder: a loop's list (collapsed to one `<arg>…`, however long), a `case` word and its patterns, a function's name.

| Command | Shape |
| --- | --- |
| `cd /repo && for pr in 101 102; do gh pr checks $pr; done` | `cd <arg> && for <arg> in <arg>…; do gh pr checks <arg>; done` |
| `while true; do sleep 5; done` | `while true; do sleep <arg>; done` |
| `if ! cargo test; then echo failed; fi` | `if ! cargo test; then echo <arg>; fi` |
| `case "$e" in deploy-prod) make ship;; esac` | `case <arg> in <arg>) make <arg>;; esac` |
| `(cd x && make) \|\| echo failed` | `( cd <arg> && make ) \|\| echo <arg>` |

So the same loop over other values has one shape, and a pattern over compound commands means something. A command with no compound syntax (a simple command, a pipeline, an `&&` chain) shapes exactly as it did before compound commands kept their structure, so older ledger entries for those still compare equal to new ones.

## Reviewing whole sessions

The ledger is deliberately myopic: it holds command shapes, for privacy and so it costs nothing to keep. It cannot see what matters most in a session that went badly:

- the user correcting the agent ("no, use X", "don't do that"), or interrupting it;
- an approach taken, then abandoned;
- the same exploration repeated, within a session or across sessions;
- time lost waiting or flailing;
- missing project knowledge that a mise task, a routine, a rule, or an AGENTS.md or memory note would have supplied.

Session review reads the whole transcript instead, through a digest, and only now and then.

### The digest

`jouet wind digest SESSION` reads one Claude Code or Codex transcript and reduces it to a few thousand tokens. `SESSION` is a session ID, a prefix of one, or a transcript's path; `--last` takes the most recently written session (inside a session, that is the session itself), and `--since 1d` every session written since then, the eight most recent, with what they share listed first. `--project PATH` and `--harness` narrow the choice, and `--budget N` sets the tokens per digest (4000 by default).

The digest lists, most telling first:

- **jouet itself:** where jouet crashed, exited outside its exit-code table, printed `--json` that is not JSON, met usage errors, or was dropped for raw `gh` or `git` right after it answered. See [what sessions show about jouet](#what-sessions-show-about-jouet).
- **User corrections:** interruptions, refused tool calls (with the reason the user gave), and prompts that correct the agent, each with the call the agent made just before. A prompt is a correction when it opens with "no", "stop", "wait", "don't", "actually" or "instead", or contains a phrase such as "I said", "that's not what", "why did you", "still failing" or "rather than", outside code spans. Whatever follows an interruption is counted as a redirect. Text the harness writes in the user's place (slash-command expansions, task notifications, resumed-session summaries, AGENTS.md and environment blocks) is not a prompt.
- **Failures:** commands and calls that failed, how often, whether a later attempt of the same shape worked, and the output line that says what went wrong. A search that found nothing once is left out.
- **Long calls:** any call of 30 seconds or more.
- **Where tokens and time went:** `jouet audit`'s categories for this one session, and the costly patterns it knows (sleep polls, cut test output, retries), with its advice.
- **Files read repeatedly:** three reads or more, or two with no edit to the file in between, counting shell reads such as `sed -n`, `cat` and `nl`.
- **Commands typed out again:** the same command three times, or a long one twice: candidates for a mise task.
- **A timeline:** one phase per prompt, with its time, requests and tokens, and its calls collapsed by what they run (`grep ×21`, `cargo test ×3 (2 failed)`, `gh pr view ×2`) with up to three files or example commands each.

Every item names its lines in the transcript (`L120-188`), so a reviewer can read the original. When a digest is over budget, its lists and timeline are cut in steps, each cut ending with a marker saying what was left out; corrections and failures go last.

```text
## User corrections (4)
- L8-10 interrupt — after `cargo test 2>&1 | tail -<n>`
- L8-11 negation (after an interrupt): "no, use mise run test instead of cargo test, and don't pipe it through tail" — after `cargo test 2>&1 | tail -<n>`
- L20-21 refusal: "never force-push; open a PR instead" — after `git push <v>… --force`
- L20-24 repeat: "I said open a PR. GH_TOKEN=<redacted> is already in the env" — after `git push <v>… --force`

## Failures (2)
- L4-8 `cargo test 2>&1 | tail -<n>` failed 2 of 2, never worked: error[E0425]: cannot find value `x` in this scope
```

### The review

jouet calls no model and holds no API keys, so an agent does the review. `jouet wind review SESSION` (with the same choices as `digest`) prints a prompt and the digest, for a fresh subagent or a headless run:

```sh
claude -p "$(jouet wind review --since 1d)" | jouet wind propose
codex exec "$(jouet wind review --last --harness codex)" | jouet wind propose
```

The prompt asks the reviewer for causes, not symptoms, and for at most three proposals, each grounded in spans, of one of these kinds, using the [decision guide](#what-a-proposal-can-be):

```json
{"session": "d1d1…", "proposals": [
  {"kind": "task", "title": "Add a `release:check` task", "why": "The agent tried `cargo xtask release-check` twice before the user named the task (L8-18).", "evidence": ["L8-14", "L18"], "scope": "local", "draft": "[tasks.\"release:check\"]\nrun = \"...\""}
]}
```

`jouet wind propose` reads that reply on stdin (or from a file), checks each proposal and records it as pending. Recording applies nothing: each next session start shows a pending proposal in one line, like a ledger note, and the agent there asks the user before applying it, then runs `jouet wind adopt ID`. `jouet wind show ID` prints the proposal and its draft, and `jouet wind dismiss ID` drops it.

**Why a separate agent, not the session itself.** The stop hook could ask the agent to review its own session before it finishes, but the stop hook fires at the end of every turn, and a review there would spend the session's own, largest context, at the moment the user is waiting for the answer, by the agent least able to see its own mistakes. A fresh subagent or a headless run starts from the digest alone, a few thousand tokens, and can run when nobody is waiting, or on a schedule.

### What a proposal can be

- **`task`:** a mise task. Fixed commands, no decisions, specific to the project, useful to humans too: the default for anything retyped. Its scope is `local` (`mise.local.toml`, the default), `project` (`mise.toml`) or `global`: absolute or home paths, personal flags or a single test mean local; only paths inside the repository, tools already in `mise.toml` and names like the existing tasks mean project. The agent always asks the user before writing to `mise.toml`.
- **`routine`:** a Luau program over jouet's primitives, when it needs logic over structured state. Start with a task, and escalate when conditionals creep in.
- **`rule`:** a rule that steers the agent's own tool calls, such as pointing a long command at the task that replaces it. Always warn mode, in user scope, like a rule the ledger drafts.
- **`agents_md`:** a line or two for the project's AGENTS.md: knowledge every agent in the repository needs. Always asked first.
- **`memory`:** a note for the harness's memory: this user's preference, across projects.
- **`jouet`:** a change to jouet itself, for its maintainers. Offered only with `[share] contribute` on. It is not a pending proposal: `jouet wind propose` records it as an idea for [`jouet share`](#what-sessions-show-about-jouet), which drafts it and posts only with the user's yes.
- **`nothing`:** most sessions need nothing, and the prompt says so.

### Suggesting a review

With `[wind] review` on (it is off by default), the stop hook digests the session it ends, at most every ten minutes per session, from the transcript path the harness passes it. A session with two or more user corrections (an interruption and the prompt after it count once), six or more failed attempts of retried commands, or ten minutes polling CI or a process (as `jouet audit` counts it: a long build is work, not waste) is queued, at most one in every `review_every` sessions (five by default). The next session start in the same repository shows one line:

```text
jouet wind: session d1d1d1d1 (2h ago, this repository) had 3 user corrections, 2 failed retries, 2m21s polling. At a natural pause, review it in a subagent: give it the output of `jouet wind review d1d1d1d1-0000-4000-8000-000000000001`, and it records what it proposes with `jouet wind propose`. `jouet wind dismiss r1` to skip it.
```

### Limits

Proposals share the ledger notes' guardrails:

- At most two lines per session start, the ledger's notes first, then suggested reviews, then proposals. A line shown to three sessions without being acted on lapses.
- At most three proposals per review and three a day. One with the same kind and title as an earlier proposal is skipped.
- A proposal that looks like it holds a secret is refused, as is one without a title, a why and a draft, with an unknown scope, or with evidence that is not a span.
- Nothing is applied by jouet. Rules start in warn mode in your rules; anything for the project (`mise.toml`, AGENTS.md, a repository rule) needs the user's yes, and a repository's rules change through a reviewed PR.

### Privacy and cost

A digest holds more than the ledger, so it is never written to disk by jouet, only printed. It holds:

- the start of each prompt and correction (160 characters), and the line of an error that says what went wrong;
- commands, cut to 80 or 160 characters, and the files they read or edited, relative to the session's directory;
- counts, durations and token totals.

Before anything is shown, the home directory becomes `~`, and secrets are redacted with jouet's patterns (prefixed credentials, JSON web tokens, values named as secrets, credentials in URLs, long key-like strings) and, when a [gitleaks](https://github.com/gitleaks/gitleaks) config is found, its rules too: `[wind] gitleaks = PATH`, or `~/.config/jouet/gitleaks.toml` when it exists. A rule whose regex Rust's `regex` crate cannot compile is skipped, and the digest's first line says which rules redacted it. The session's own directory is kept whole, however key-like a worktree's name looks.

Reading is local, and a digest goes only where you pipe it: to a subagent in the same harness, or a headless run, under the same terms as the session it digests.

Building one reads the transcript once. Over 1,759 Claude Code and 989 Codex transcripts on one machine (a median of 1 MB, the largest 42 MB), a digest took 8 ms at the median and 0.6 s at most, and came to about 1,000 tokens at the median and 3,700 at the 99th percentile, within the default budget of 4,000. That is also what the stop hook spends, with `[wind] review` on, at most every ten minutes per session.

## Settings

```toml
[wind]
enabled = true          # JOUET_WIND: draft rules and show notes
auto_promote = false    # JOUET_WIND_AUTO_PROMOTE: promote clean rules to deny on their own
promote_after = 20      # JOUET_WIND_PROMOTE_AFTER: firings, with no overrides, before that
review = false          # JOUET_WIND_REVIEW: digest sessions at stop, suggest reviewing one
review_every = 5        # JOUET_WIND_REVIEW_EVERY: sessions, at least, between two suggestions
review_corrections = 2  # JOUET_WIND_REVIEW_CORRECTIONS: corrections that make a session worth it
gitleaks = "~/src/gitleaks/config/gitleaks.toml"  # JOUET_WIND_GITLEAKS: also redact with these rules

[share]
contribute = false      # JOUET_SHARE_CONTRIBUTE: suggest sharing what sessions show about jouet
```

`JOUET_WIND=0` turns winding off, reviews included. The ledger itself has its own switch, `[hooks] ledger = false`.

## Sharing rules and ideas

Contributing is opt-in, and a person approves every post. A rule that has proven itself here may help everyone, and a rule jouet's API could not express shows what jouet should build next.

- **`jouet share RULE`** turns a rule of yours into a proposal. It refuses until the rule passes `jouet rule test`, fires on the ledger under `--replay`, has denied for 7 days since `jouet rule promote`, and has fired at least 10 times with at most 10% overridden. It says which of these is missing.
- **`jouet share --idea [KEY]`** turns recorded gaps into an idea: "I kept trying to catch X; there is no primitive for Y". jouet records a gap when a rule reads a `jouet` primitive that does not exist, and `jouet rule gap "TEXT"` records one in your words. It also covers [what sessions show about jouet itself](#what-sessions-show-about-jouet), and the changes to jouet a session review proposed.
- **`jouet share --list`** lists the ideas ready to share, with their keys and where each would go.
- **What it posts.** Without `--yes`, nothing: it prints the exact title and text, and a diff of what anonymizing changed. Paths, repository names, hostnames, emails, your login and this machine's name are replaced; `--redact WORD` replaces more, such as an internal tool's name. It refuses text that still looks like a secret.
- **Where.** With `--yes`, it posts with your GitHub login, ending with an AI disclosure line (your `reply_footer`, if you set one): a bug as an Issue on [jdx/jouet](https://github.com/jdx/jouet/issues), anything else as a Discussion in its "Rules & ideas" category. It first looks for an Issue or Discussion with the same title, and posts at most three a week in all. If the Discussions category does not exist yet, it says so and posts nothing.

`jouet skill --contribute` prints an optional skill that tells an agent when a contribution is worth making, and that it posts only after the user approves the exact text.

### What sessions show about jouet

jouet watches itself in the same ledger and digests. What it notices becomes an idea for `jouet share --idea`, kept beside the missing primitives in `$XDG_STATE_HOME/jouet/gaps.json`, as shapes and counts only:

| Signal | Seen in | Ready to share | Goes to |
| --- | --- | --- | --- |
| jouet crashed: exit 101, or `panicked at` in its output (the location only, never the message) | ledger, digest | at once | an Issue, with the commands' shapes as steps to reproduce |
| An exit code outside the shared table (126 and up belong to the shell) | ledger, digest | at once | an Issue |
| `--json` output that does not start a JSON document, when nothing else printed to it | digest | at once | an Issue |
| A usage error (exit 2), and whether the agent retried with other flags | ledger, digest | 3 times in 2 sessions | a Discussion: a confusing flag, or a gap in `agent-help` |
| An answer dropped: the raw command a jouet command replaces (`gh pr view` after `jouet pr status`, `git commit` after `jouet commit`) run right after it succeeded | ledger, digest | 3 times in 2 sessions | a Discussion: the output may be missing something |
| A built-in rule overridden at least 3 times and 30% of its 10 or more firings | ledger | at once | a Discussion with the overridden shapes; an Issue from 50% |
| A hook over its time budget (`[hooks] budget_ms`, 500 ms for the tool hooks and ten times that for session start and stop), which the hook records itself as a `slow-hook` ledger entry | ledger | 3 times | an Issue, with the budget and the slowest run |
| The same script (a loop, a conditional, or three commands or more chained) in 5 sessions | ledger | at once | a Discussion: a candidate built-in |

The ledger's signals are counted afresh each time `jouet share` reads them; a digest's are added once per session and line, so digesting a session again counts nothing twice. A key names each signal, such as `crash-pr-threads` or `usage-wind-digest`. A shared one is not proposed again, and a second user seeing the same problem finds the first post by its title.

With `[share] contribute = true` (off by default), a session start may add one line, within the same two-line budget as the winding notes, suggesting the most pressing idea (bugs first) to the agent, at most three times per idea:

```text
jouet noticed something for its maintainers: `jouet pr threads` crashed 2 times (panicked at src/cmd/threads.rs:41:9). `jouet share --idea crash-pr-threads` drafts an Issue on jdx/jouet; show the draft to the user, and post it with `--yes` only if they agree.
```

The session review prompt then also offers the [`jouet` kind](#what-a-proposal-can-be).
