---
description: How jouet notices costly patterns in a local ledger of command shapes, in reviews of whole sessions and in agents' saved memories, and proposes rules, mise tasks and notes for an agent to adopt, test, promote or retire.
---

# Self-winding

Self-winding turns what keeps going wrong in your agents' sessions into changes you can adopt: [rules](/rules) that steer the agent, mise tasks, routines, and notes for AGENTS.md or the agent's memory. It runs from the hooks [`jouet setup`](/setup) installs, on this machine, and calls no model. Each thing it drafts waits as a one-line note at the next session start, and nothing takes effect until the agent adopts it, asking you first when it would change the project or a memory.

The notes come from three sources:

- **The ledger.** The hooks record the shape of every command an agent runs in a [ledger](/setup#the-ledger), cheaply and all the time. When the same costly pattern shows up there in at least two sessions, such as a command that never works or long `sleep`s, jouet drafts a rule in warn mode. When the same command sequence keeps succeeding in a repository, it suggests a mise task. See [Rules from the ledger](#the-loop) and [Winding into mise tasks](#winding-into-mise-tasks).
- **Session review.** Off by default. An agent reads a digest of a whole session (what the user corrected, approaches tried and abandoned, waits, exploration repeated) and proposes mise tasks, rules, routines, or AGENTS.md and memory notes. See [Reviewing whole sessions](#reviewing-whole-sessions).
- **Memory upkeep.** Once a day, jouet reads the memories agents keep and checks the claims they depend on, such as a PR a workaround waits on. When one has changed, the next session start in that project says the memory may be stale. See [Keeping memories current](#keeping-memories-current).

The ledger, the notes and the drafts stay in local files. Memory upkeep asks GitHub about the PRs and issues memories name, a digest goes only where you pipe it, and [`jouet share`](#sharing-rules-and-ideas) posts only with `--yes`. Every change to a rule is logged. Winding is on after `jouet setup`, and `JOUET_WIND=0` turns it off.

::: warning Early development
Winding is new. The patterns it notices, and its thresholds, will change between releases.
:::

## What you see at session start

jouet's session-start primer ends with at most two winding lines. Each line says what was noticed and gives the commands to read, adopt or drop it. A rule drafted from the ledger looks like this:

```text
jouet wind noticed: `mytool <arg> --all` failed 4 times in 2 sessions (last exit 127, not found) and never worked. It drafted rule `wind-mytool-fails` (warn mode): `jouet wind show n1` to read it, `jouet rule new wind-mytool-fails --from-note n1 --author s-42` to adopt it, `jouet wind dismiss n1` to drop it.
```

The lines are written for the agent. A line that would change the project or a memory tells the agent to ask you at a natural pause before it acts. The letter in a note's ID says what it is:

- **`n1`, a rule drafted from the ledger.** `jouet wind show n1` prints the rule, and `jouet rule new NAME --from-note n1` adopts it. See [Rules from the ledger](#the-loop).
- **`t1`, a command sequence a mise task could replace.** `jouet task propose` drafts the task from the session transcripts, and `jouet task apply t1 --file local|shared` adds it to the file you choose. See [Winding into mise tasks](#winding-into-mise-tasks).
- **`m1`, a memory whose workaround may be fixed.** `jouet wind show m1` gives the memory's path. Once the agent has checked the bug and you have agreed to update or delete the memory, `jouet wind adopt m1` marks it done. See [Notes at session start](#notes-at-session-start).
- **`r1`, a session worth reviewing.** The line gives the `jouet wind review` command to hand a subagent. See [Suggesting a review](#suggesting-a-review).
- **`p1`, a proposal from a review.** `jouet wind show p1` prints why and the draft. The agent applies it as the line says, asking you where it must, then runs `jouet wind adopt p1`. See [What a proposal can be](#what-a-proposal-can-be).

`jouet wind dismiss ID` drops any of these, and it is never shown again. `jouet wind adopt` takes only proposals and memory notes: given a drafted rule or a mise task, it exits 2 and names the command that adopts it. A line shown to three sessions without being acted on lapses.

When more than two are pending, the ledger's rule notes come first, then mise tasks for this repository, then notes about this project's memories, then suggested reviews, then proposals. With `[share] contribute` on, a last line may suggest [reporting something to jouet's maintainers](#what-sessions-show-about-jouet), within the same two lines.

## Guardrails

- **Warn first.** A rule jouet drafts starts in warn mode, and so does a rule a review proposes. Promotion to deny takes `jouet rule promote` or, when you turn `auto_promote` on, a clean record.
- **Your rules only.** Winding writes, promotes and retires rules in `~/.config/jouet/rules` and nothing else. Built-in rules are never proposed for retirement, and none of yours can replace or overrule one (see [the floor](/rules#where-rules-come-from)). A repository's rules run only once trusted, and change through a reviewed PR.
- **Your choice for the project.** jouet applies nothing itself. Anything for the project (`mise.toml`, AGENTS.md, a repository rule) or a change to a memory needs your yes. `jouet task apply` refuses until it is told which file.
- **Few lines.** At most two lines per session start. At most two new rule drafts, two new mise task notes and two new memory notes a day, and three proposals per review and three a day.
- **The sandbox.** Every rule fails open and runs within its budget, whoever wrote it.
- **Plain files.** The notes, proposals and their states are in `$XDG_STATE_HOME/jouet/wind.json`, and the rule log is in `$XDG_STATE_HOME/jouet/rule-log.jsonl`. `jouet rule log` lists every change (created, promoted, retired), when, by whom and why.

## Rules from the ledger {#the-loop}

At session start and at the end of each turn, at most once a minute, `jouet wind` reads the ledger from where it last stopped. Running `jouet wind` by hand reads it straight away. These are the steps from noticing a pattern to retiring the rule drafted against it:

1. **Notice.** `jouet wind` looks for three patterns:
   - **A command that never works:** the same program, subcommands and flags failing at least three times, across at least two sessions, and never succeeding, such as a tool that is not installed. Test, lint and build runs are left out, because they fail while code is being fixed.
   - **Long sleeps:** at least five sleeps of ten seconds or more, across at least two sessions, where `jouet wait` would wait for the thing itself.
   - **A repeated sequence:** the same sequence of commands succeeding at least three times, across at least two sessions, in one repository. This one becomes a mise task, not a rule. See [Winding into mise tasks](#winding-into-mise-tasks).
2. **Draft.** For each pattern, it writes a rule against the public `jouet` API, in warn mode. Its `fires` examples are the calls that motivated it. Its `quiet` examples are near misses it must leave alone: what worked instead, and calls in the ledger running the same program. A draft that fails its own examples is dropped. The draft is kept as a note, and at most two notes are drafted a day.
3. **Show.** The next session start shows the note in one line, as in [What you see at session start](#what-you-see-at-session-start).
4. **Adopt.** `jouet rule new NAME --from-note ID` writes the rule to `~/.config/jouet/rules/NAME.toml`, with the session that adopted it as its author. Most drafts are a matcher: the command that kept failing, with exactly the options it failed with. A draft about long sleeps also writes `NAME.luau`, a script that compares durations, which a matcher cannot.
5. **Test.** `jouet rule test NAME` runs its examples. `jouet rule test NAME --replay` runs it over every call in the ledger, lists the calls it fires on and the near misses it leaves alone, and fails if it fires on nothing.
6. **Promote.** A rule stays in warn mode until `jouet rule promote NAME` switches it to deny, which it does only for a rule that passes examples of both kinds. With `auto_promote` on, a rule that has fired `promote_after` times, in at least two sessions, with no overrides, is promoted on its own.
7. **Unwind.** `jouet unwind` proposes retiring rules whose firings are mostly overridden (half or more, once a rule has fired five times), that fire on a large share of calls (10% or more of all shell calls, and at least 20 times), or that no longer load. It lists rules that are not yours too, such as a trusted repository's, and `jouet unwind --apply` leaves those alone: a repository's rules change through a PR. For your rules, `--apply` moves them to `~/.config/jouet/rules/retired/` and logs it. `jouet unwind` also proposes removing mise tasks jouet added that no session has run in 21 days (see [Unwinding](/commands/task#unwinding)).

### Replaying shapes

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

The same loop over other values therefore has one shape, and a pattern over compound commands means something. A command with no compound syntax (a simple command, a pipeline, an `&&` chain) shapes exactly as it did before compound commands kept their structure, so older ledger entries for those still compare equal to new ones.

## Winding into mise tasks

Many commands an agent retypes belong in the project's task runner, where people can run them too. The ledger notices a repeated sequence by its shape, and the next session start in that repository names it:

```text
jouet wind noticed `cargo build && cargo test -p <arg>` run 3 times in 2 sessions in this repository; a mise task may replace it. At a natural pause, `jouet task propose` drafts one from the session transcripts (`jouet wind dismiss t1` drops it).
```

The ledger never keeps the command itself, so [`jouet task propose`](/commands/task) reads the repository's session transcripts and drafts the task: its name, `run`, description, `depends` and `sources` where they can be inferred, and the evidence. It also suggests `mise.local.toml` (personal, not committed) or `mise.toml` (shared, committed), and says why.

Which file is always the user's choice. A pending task's line at session start tells the agent to ask the user at a natural pause, giving jouet's suggestion. `jouet task apply ID` refuses until it is given `--file local` or `--file shared`, then writes the task through `mise tasks add`. jouet never edits a mise config file itself, and applying needs mise 2026.10.25 or later. `jouet unwind` later proposes removing a task jouet added that nobody runs. [`jouet task`](/commands/task) has the details: which commands count, how the file is suggested, and how a task is applied and removed.

## Reviewing whole sessions

The ledger is deliberately myopic: it holds command shapes, for privacy and so it costs nothing to keep. It cannot see what matters most in a session that went badly:

- the user correcting the agent ("no, use X", "don't do that"), or interrupting it;
- an approach taken, then abandoned;
- the same exploration repeated, within a session or across sessions;
- time lost waiting or flailing;
- missing project knowledge that a mise task, a routine, a rule, or an AGENTS.md or memory note would have supplied.

Session review reads the whole transcript instead, through a [digest](#the-digest), and only now and then. jouet calls no model and holds no API keys, so an agent does the review, and jouet records what it proposes.

### Suggesting a review

With `[wind] review` on (it is off by default), the stop hook digests the session it ends, at most every ten minutes per session, from the transcript path the harness passes it. A session is queued for review when it has `review_corrections` or more user corrections (two by default; an interruption and the prompt after it count once), six or more failed attempts of retried commands, or ten minutes polling CI or a process (as `jouet audit` counts it: a long build is work, not waste). At most one in every `review_every` sessions (five by default) is queued. The next session start in the same repository shows one line:

```text
jouet wind: session d1d1d1d1 (2h ago, this repository) had 3 user corrections, 2 failed retries, 2m21s polling. At a natural pause, review it in a subagent: give it the output of `jouet wind review d1d1d1d1-0000-4000-8000-000000000001`, and it records what it proposes with `jouet wind propose`. `jouet wind dismiss r1` to skip it.
```

### Running a review {#the-review}

`jouet wind review` prints a prompt and the digests of the sessions it picks, for a fresh subagent or a headless run:

```sh
claude -p "$(jouet wind review --since 1d)" | jouet wind propose
codex exec "$(jouet wind review --last --harness codex)" | jouet wind propose
```

`jouet wind review` and `jouet wind digest` choose sessions the same way. `SESSION` is a session ID, a prefix of one, or a transcript's path. `--last` takes the most recently written session (inside a session, that is the session itself), and `--since TIME` every session written since then (`1d`, `12h`, a date or RFC 3339), the eight most recent, with what they share listed first. `--project PATH` keeps sessions that ran in `PATH` or below it, `--harness` picks `claude`, `codex` or `all` (the default), and `--budget N` sets the tokens per digest (4000 by default).

The prompt asks the reviewer for causes, not symptoms, and for at most three proposals, each grounded in spans, of one of [these kinds](#what-a-proposal-can-be):

```json
{"session": "d1d1…", "proposals": [
  {"kind": "task", "title": "Add a `release:check` task", "why": "The agent tried `cargo xtask release-check` twice before the user named the task (L8-18).", "evidence": ["L8-14", "L18"], "scope": "local", "draft": "[tasks.\"release:check\"]\nrun = \"...\""}
]}
```

`jouet wind propose` reads that reply on stdin (or from a file), [checks each proposal](#checks-on-proposals) and records it as pending. Recording a reply also clears that session's suggested review. Recording applies nothing: each next session start shows a pending proposal in one line, like a ledger note, and the agent there asks the user before applying it, then runs `jouet wind adopt ID`. `jouet wind show ID` prints the proposal and its draft, and `jouet wind dismiss ID` drops it.

The review runs in a separate agent rather than in the session itself. The stop hook could ask the agent to review its own session before it finishes, but the stop hook fires at the end of every turn, and a review there would spend the session's own context, its largest, at the moment the user is waiting for the answer, by the agent least able to see its own mistakes. A fresh subagent or a headless run starts from the digest alone, a few thousand tokens, and can run when nobody is waiting, or on a schedule.

### What a proposal can be

- **`task`:** a mise task. Fixed commands, no decisions, specific to the project, useful to humans too: the default for anything retyped. Its scope is `local` (`mise.local.toml`, the default), `project` (`mise.toml`) or `global`: absolute or home paths, personal flags or a single test mean local; only paths inside the repository, tools already in `mise.toml` and names like the existing tasks mean project. The agent always asks the user before writing to `mise.toml`.
- **`routine`:** a Luau [routine](/routines) over jouet's primitives, when it needs logic over structured state. Start with a task, and escalate when conditionals creep in.
- **`rule`:** a rule that steers the agent's own tool calls, such as pointing a long command at the task that replaces it. Always warn mode, like a rule the ledger drafts. The agent writes it to your rules with `jouet rule new NAME` and checks it with `jouet rule test NAME`.
- **`agents_md`:** a line or two for the project's AGENTS.md: knowledge every agent in the repository needs. Always asked first.
- **`memory`:** a note for the harness's memory: this user's preference, across projects.
- **`memory_update`:** a new text for a memory the agent already keeps, when the session contradicts it: the bug it works around did not happen, or the user corrected a step it prescribes. `target` names the memory by its path, as the review lists it.
- **`memory_delete`:** deleting a memory that is wrong or no longer needed, with `target` and the reason as its draft. A memory that works around jouet itself is a jouet bug, not a memory: the review proposes deleting it, and with `contribute` on, the bug as a `jouet` proposal too.
- **`jouet`:** a change to jouet itself, for its maintainers. Offered only with `[share] contribute` on. It is not a pending proposal: `jouet wind propose` records it as an idea for [`jouet share`](#what-sessions-show-about-jouet), which drafts it and posts only with the user's yes.
- **`nothing`:** most sessions need nothing, and the prompt says so.

A `rule`, `routine`, `memory`, `memory_update` or `memory_delete` proposal takes the scope `user` (the default) or `project`, and an `agents_md` proposal takes `project`.

### Checks on proposals

`jouet wind propose` refuses a proposal:

- without a title, a why and a draft, or with a title over 120 characters;
- with a scope its kind does not take, or evidence that is not a span such as `L120-188`;
- for `memory_update` or `memory_delete`, with a `target` that is not one of the memories the review listed;
- that looks like it holds a secret.

It skips one with the same kind and title as an earlier proposal (or, for a memory, the same target), and records at most three per review and three a day. What it records is applied only as the [guardrails](#guardrails) allow.

### The digest

`jouet wind digest SESSION` reads one Claude Code or Codex transcript, or several with `--since`, and reduces each to a few thousand tokens. It takes the same choices as [`jouet wind review`](#the-review). The digest lists, most telling first:

- **jouet itself:** where jouet crashed, exited outside its exit-code table, printed `--json` that is not JSON, met usage errors, or was dropped for raw `gh` or `git` right after it answered. See [what sessions show about jouet](#what-sessions-show-about-jouet).
- **User corrections:** interruptions, refused tool calls (with the reason the user gave), and prompts that correct the agent, each with the call the agent made right before it. A prompt is a correction when it opens with "no", "stop", "wait", "don't", "actually" or "instead", or contains a phrase such as "I said", "that's not what", "why did you", "still failing" or "rather than", outside code spans. Whatever follows an interruption is counted as a redirect. Text the harness writes in the user's place (slash-command expansions, task notifications, resumed-session summaries, AGENTS.md and environment blocks) is not a prompt.
- **Failures:** commands and calls that failed, how often, whether a later attempt of the same shape worked, and the output line that says what went wrong. A search that found nothing once is left out.
- **Long calls:** any call of 30 seconds or more.
- **Where tokens and time went:** `jouet audit`'s categories for this one session, and the costly patterns it knows (sleep polls, cut test output, retries), with its advice.
- **Files read repeatedly:** three reads or more, or two with no edit to the file in between, counting shell reads such as `sed -n`, `cat` and `nl`.
- **Commands typed out again:** the same command three times, or a long one (80 characters or more) twice: candidates for a mise task. Commands under 24 characters are not counted.
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

### Privacy and cost

A digest holds more than the ledger, so jouet never writes it to disk, only prints it. It holds:

- the start of each prompt and correction (160 characters), and the line of an error that says what went wrong;
- commands, cut to 80 or 160 characters, and the files they read or edited, relative to the session's directory;
- counts, durations and token totals.

Before anything is shown, the home directory becomes `~`, and secrets are redacted with jouet's patterns (prefixed credentials, JSON web tokens, values named as secrets, credentials in URLs, long key-like strings) and, when a [gitleaks](https://github.com/gitleaks/gitleaks) config is found, its rules too: `[wind] gitleaks = PATH`, or `~/.config/jouet/gitleaks.toml` when it exists. A rule whose regex Rust's `regex` crate cannot compile is skipped, and the digest's first line says which rules redacted it. The session's own directory is kept whole, however key-like a worktree's name looks.

Reading is local, and a digest goes only where you pipe it: to a subagent in the same harness, or a headless run, under the same terms as the session it digests.

Building one reads the transcript once. Over 1,759 Claude Code and 989 Codex transcripts on one machine (a median of 1 MB, the largest 42 MB), a digest took 8 ms at the median and 0.6 s at most, and came to about 1,000 tokens at the median and 3,700 at the 99th percentile, within the default budget of 4,000. That is also what the stop hook spends, with `[wind] review` on, at most every ten minutes per session.

## Keeping memories current

Agents save memories, and many of them work around a bug: "until #886 is released, run cargo through `mise x`". The bug gets fixed, and later sessions keep applying the workaround. Wind reads those memories and checks, without a model, whether what each one depends on still holds.

### Notes at session start

The stop hook starts `jouet wind memories --record` in the background, at most once a day, since it waits on GitHub. What it finds becomes a memory note (`m1`), under the same limits as the ledger's notes: two new a day, two lines per session start, shown to three sessions at most. A finding already noted, whatever became of the note, is not noted again.

```text
jouet wind: memory `tool-cache-workaround` waits for a release with acme/tool#886, and v1.30.0 (2026-11-02) has it: the bug it works around may be fixed. At a natural pause, check whether it still happens, then ask the user before updating or deleting the memory. `jouet wind show m1` for its path; `jouet wind adopt m1` once done, `jouet wind dismiss m1` to drop it.
```

jouet never changes a memory. The agent that sees the note checks whether the bug is gone, and asks the user before it updates or deletes anything. A memory that works around a built-in rule is reported as a jouet bug: the note says to tell the user, and with `[share] contribute` on it also names the idea `jouet share --idea` drafts.

### Which memories

- Claude Code's: each project's notes in `~/.claude/projects/*/memory/*.md` (not the `MEMORY.md` index), and `~/.claude/CLAUDE.md`.
- Codex's: `~/.codex/AGENTS.md`, and the memories Codex consolidates in `~/.codex/memories/` (`MEMORY.md` and `memory_summary.md`). `memories_1.sqlite` is left out: it holds the extractions Codex itself consolidates and prunes.
- `$CLAUDE_CONFIG_DIR` and `$CODEX_HOME` move them, as they do for the harnesses.

A Claude Code memory belongs to a project. A session in that project's checkout, a directory in it, or a worktree added from it sees notes about the project's memories. The global files' notes go to every session.

### What is checked

A memory's text is read in sentences. Its claims are the ones a workaround depends on:

| Claim | Read from | Stale when |
| --- | --- | --- |
| A PR or issue it waits on | `owner/name#12`, a GitHub URL, or `#12` beside "pending", "waits on", "blocked on", "until … merges", "when to drop this", or "#12 is still open" | it is merged or closed |
| A release it waits for | a merged PR beside "isn't in a release yet", "not released", "predates it", "once … ships" or "waits for the next release" | the repository's latest release has the merge |
| A jouet command or flag | `` `jouet …` `` in a code span | this jouet has no such command or flag |
| A built-in rule to get past | a rule's name in a code span, in a sentence that says to override or bypass it, or that it fires wrongly | always: that is a jouet bug |

Only a difference between what a memory says and what is true counts. "Fixed in jdx/mise#13193" is history, and is never raised. A memory that records a PR as merged anywhere is taken at its word. A bare `#12` belongs to the repository named immediately before it, else to the only one the memory names, else (when it names none) to the project's `origin`; when that is unclear, it is left out.

```text
$ jouet wind memories
read 42 memories (3 projects); asked GitHub about 5 references
tool-cache-workaround  ~/.claude/projects/-home-me-src-app/memory/tool-cache-workaround.md
  waits for a release with acme/tool#886, and v1.30.0 (2026-11-02) has it
```

GitHub is asked through GraphQL, with the token `jouet pr` uses, in batches of up to 80 references, and a release through one comparison of its tag with the merge. Answers that cannot change (merged, closed, released) are kept in the winding state and never asked again; an open PR is asked again after 20 hours. Without a token or a network, the GitHub claims are skipped and the rest is still checked.

### In a review

`jouet wind review` lists the memories the reviewed sessions' agent reads: their projects' and the global ones, after the digests. Each line gives a memory's path, name and description; the memories a session read, and those with a pending note, are shown whole (three at most, redacted like the digest). The reviewer can propose a [`memory_update` or `memory_delete`](#what-a-proposal-can-be) naming one, and `jouet wind propose` refuses a `target` that is not one of them.

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

## Settings

```toml
[wind]
enabled = true          # JOUET_WIND: draft rules and show notes
auto_promote = false    # JOUET_WIND_AUTO_PROMOTE: promote clean rules to deny on their own
promote_after = 20      # JOUET_WIND_PROMOTE_AFTER: firings, with no overrides, before that
review = false          # JOUET_WIND_REVIEW: digest sessions at stop, suggest reviewing one
review_every = 5        # JOUET_WIND_REVIEW_EVERY: sessions, at least, between two suggestions
review_corrections = 2  # JOUET_WIND_REVIEW_CORRECTIONS: corrections that make a session worth it
memories = true         # JOUET_WIND_MEMORIES: check saved memories once a day
gitleaks = "~/src/gitleaks/config/gitleaks.toml"  # JOUET_WIND_GITLEAKS: also redact with these rules

[share]
contribute = false      # JOUET_SHARE_CONTRIBUTE: suggest sharing what sessions show about jouet
```

`JOUET_WIND=0` turns winding off: the ledger's notes, mise task notes, memory checks, reviews, and the `jouet share` line at session start. The ledger itself has its own switch, `[hooks] ledger = false`.
