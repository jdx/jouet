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

It sets the project up in [mise](https://mise.jdx.dev): the tools the checks need, at the versions the project already requires, any environment a check needs to run, and a [task](https://mise.jdx.dev/tasks/) for each check, named by its kind (`test`, `test:unit`, `lint`), with what it covers and the files it reads. If the project has no mise config, the agent creates `mise.toml` and says so. You and every later session then run `mise run test:unit` rather than working out the command again, and mise can skip a task whose files have not changed. The agent does not change mise's `[settings]`, add hooks or put secrets in `[env]`, and mise runs a new or changed config only after you trust it.

Where the project needs it, the agent adapts the test configuration, adds a mise task for a project-specific routine (or a jouet [routine](/routines) when a step needs what jouet knows, such as a PR's status or CI results), or writes a warning [rule](/rules) for a costly habit it has seen. It keeps existing settings, and changes only what the project supports and you asked for. Sometimes nothing needs to change, and the agent says so.

It then checks what it changed, and reports the full and focused check commands, the slow checks it knows of, what passed, and what is still missing, whether that needs more configuration or a change to jouet. A dry run of test selection is reported as a dry run, not as tests passing. [Self-winding](/winding) carries on from there, proposing rules and tasks as sessions go by.

## The project's checks

The skill records the checks it established in `.jouet/project.toml`, which you commit with the project:

```toml
[checks]
full = "mise run test"
affected = "env -u NO_COLOR jouet test --affected" # when this project needs it
lint = "jouet diag check --changed"
before_push = ["cargo clippy --workspace --all-targets -- -D warnings"]

[checks.focused]
unit = "jouet test --affected"
bats = "jouet test --cmd -- mise run test:bats -- test/NAME.bats"

[checks.ci]
e2e = "mise run test:e2e"

[checks.broken]
"mise run lint" = "golangci-lint is built with Go 1.26; go.mod needs 1.27"
```

Once the repository's jouet files are trusted, the session-start primer names them in one line: `jouet: prefer these project-specific checks over generic examples when applicable (.jouet/project.toml): bats …; unit …; full suite …; before pushing …`. Every later session then starts knowing how this project runs its focused tests and what CI enforces, instead of looking for it. In the benchmark, agents that did not know a project's focused bats command spent several requests finding a way to run it. The keys are `full`, `lint`, `before_push` (a list), `focused` (a table, by what each check covers), `ci` and `broken`.

When the check list exceeds the primer budget, jouet omits whole entries and gives their count and the configuration path; it never prints part of a command. Known-broken checks come first, so a later recommendation is not shown while its warning is hidden by the budget. If the warnings themselves do not fit, read the configuration for the omitted entries.

When a project needs an environment adjustment for affected tests, `checks.affected` supplies the verified command in the primer in place of the generic example. It is used only for a trusted project with an available affected-test adapter. If the selected affected command is listed in `checks.broken`, the primer gives the general test-command form instead. The command is guidance for the agent; jouet does not execute it at session start. It must be a nonempty single line of at most 200 characters without backticks. Verify that it selects the intended tests with a representative change before recording it; on a clean repository, a suitable historical change can be selected with `--since` without modifying the working tree.

The skill runs each check once before it records it. For a focused command with a test placeholder, it verifies representative tests for the packages and test types the command claims to cover; configurations can include one kind of test and exclude another. A file placeholder accepts a complete path unless the check is explicitly scoped to a subset. The skill carries required environment settings into every check that may run the affected test. If a run selects no tests or affected projects, that does not establish coverage, and an unverified scope is reported instead of recorded as working. A check that is slow, or needs services or tools only CI has, goes in `ci`: the primer tells agents to push and [`jouet wait ci`](/commands/wait) instead of running it. A check that already fails before any change, for a reason outside the code, goes in `broken` with that reason. The primer tells agents it is not theirs to fix, and a failed run of it does not hold up finishing, as a failure that [blames the machine](/setup#what-clears-a-failure) does not. Remove the entry once the environment is fixed. An unknown key makes the file unreadable, so a typo is not silently dropped.

## A map of the code

The skill also writes `.jouet/map.md`, which you commit with the project: a short guide (under 80 lines) to where each subsystem and its tests live, the entry points where new commands or routes are registered, the generated files not to edit by hand, and, when verified, the existing command to format selected files. Once the repository's jouet files are trusted, the primer points to it in one line, `jouet: .jouet/map.md maps where this project's code, tests and generated files live (N lines): read it before searching for where something is.`, without inlining it, so a session pays for it only when it reads it. Reads and searches are the largest share of agents' input tokens in jouet's session audits; the map replaces the searches a session would make to find its way around. Changing the map, like any of the repository's jouet files, needs trusting again.

## Trust and existing files

Trusting a repository's rules stays with you. When something the agent added cannot run until the repository is trusted, it says which files need your review. It does not run `jouet trust` or work around a refusal.

Running `jouet setup` again updates the installed skill only while it is unedited. A skill you wrote or edited is kept, also by `--uninstall`.
