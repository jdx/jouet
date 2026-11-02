---
description: Use the jouet-init skill to have an agent fit jouet to a project's checks and timings, and check what it changed.
---

# Initialize a project

`jouet setup` connects jouet to your agent. Fitting it to one project takes judgment about that project: which tests matter, which runs are expensive, and which repeated work is worth turning into a task or a rule. That part is a skill, **jouet-init**, which an agent runs in the project.

::: warning Breaking change before 1.0
The `jouet-init` skill replaces the `jouet init` command. Run `jouet setup` again to install it, then invoke the skill in your agent.
:::

## Running it

`jouet setup` installs the skill along with the hooks, and says what to do next:

```sh
jouet setup codex   # or: claude, cursor, gemini, copilot
```

Start a new agent session in the project and run `$jouet-init` in Codex or `/jouet-init` in Claude Code, or ask:

```text
Use the jouet-init skill to configure jouet for this project.
```

For a harness without skills, `jouet skill --init --format agents-md` prints the same instructions, to give the agent in that session.

## What the skill does

The agent reads the project's instructions, manifests, task definitions, CI and any existing jouet configuration. From the task definitions it works out the full check commands and the focused ones, then reads [`jouet cost`](/commands/cost) for how long each has taken. Where there is Claude Code or Codex history for the project, it can [audit](/commands/audit) those sessions too.

The timings let it plan around slow builds and tests. A new project may have none yet, and the skill does not start a cold rebuild or the full suite just to get them: normal work run through `jouet test` builds the history.

Where the project needs it, the agent adapts the test configuration, adds a task or a [routine](/routines), or writes a warning [rule](/rules) for a costly habit it has seen. It keeps existing settings, and changes only what the project supports and you asked for. Sometimes nothing needs to change, and the agent says so.

It then checks what it changed, and reports the full and focused check commands, the slow checks it knows of, what passed, and what is still missing, whether that needs more configuration or a change to jouet. A dry run of test selection is reported as a dry run, not as tests passing. [Self-winding](/winding) carries on from there, proposing rules and tasks as sessions go by.

## The project's checks

The skill records the checks it established in `.jouet/project.toml`, which you commit with the project:

```toml
[checks]
full = "mise run test"
lint = "jouet diag check --changed"
before_push = ["cargo clippy --workspace --all-targets -- -D warnings"]

[checks.focused]
unit = "jouet test --affected"
bats = "jouet test --cmd -- mise run test:bats -- test/NAME.bats"
```

Once the repository's jouet files are trusted, the session-start primer names them in one line: `jouet: this project's checks (.jouet/project.toml): bats …; unit …; full suite …; before pushing …`. Every later session then starts knowing how this project runs its focused tests and what CI enforces, instead of looking for it. In the benchmark, agents that did not know a project's focused bats command spent several requests finding a way to run it. The keys are `full`, `lint`, `before_push` (a list) and `focused` (a table, by what each check covers). An unknown key makes the file unreadable, so a typo is not silently dropped.

## Trust and existing files

Trusting a repository's rules stays with you. When something the agent added cannot run until the repository is trusted, it says which files need your review. It does not run `jouet trust` or work around a refusal.

Running `jouet setup` again updates the installed skill only while it is unedited. A skill you wrote or edited is kept, also by `--uninstall`.
