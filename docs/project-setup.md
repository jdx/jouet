---
description: Use the jouet-init skill to adapt jouet to a project's workflows, make expensive checks visible and verify useful configuration changes.
---

# Initialize a project

After installing jouet and connecting your agent, use **jouet-init** to adapt
it to your project. Onboarding needs the agent's judgment about your workflows:
which tests matter, which runs are expensive, and which repeated work is worth
turning into a tool.

```sh
jouet setup codex   # or: claude, cursor, gemini, copilot
```

Setup installs the skill and prints what to do next. Start a new agent session
in your project and run `$jouet-init` in Codex or `/jouet-init` in Claude Code.
You can also ask:

```text
Use the jouet-init skill to configure jouet for this project.
```

## What the skill does

The agent reads your project instructions, manifests, task definitions, CI and
existing jouet configuration. It identifies the full and focused check commands
from their definitions, then reads `jouet cost` for timing history. Where useful
Claude Code or Codex history exists, it can audit the project's sessions too.

Those measurements help it plan around slow builds and tests. A new project
may have no timings yet; the skill does not launch a cold rebuild or full suite
just to gather them. Normal work run through `jouet test` builds that history.

The agent can adapt test configuration, add a task or routine, or write a
warning rule for a demonstrated costly habit. It preserves existing settings
and makes only changes supported by the project and requested scope. Finding
that no extra configuration is needed is a useful result too.

It validates what it changes, then reports the full and focused check commands,
known slow checks, what passed, and any gaps that still need configuration or
changes to jouet. A selection dry run is reported as a dry run, not as passing
tests. See [cost analysis](/commands/cost) and [self-winding](/winding) for how
measurements and lessons carry forward.

## Trust and existing files

The skill leaves repository trust for a person to review. If a configuration
cannot be exercised until trusted, it explains that limitation and which files
need review. It does not run `jouet trust` or work around a refusal.

Re-running `jouet setup` updates the installed onboarding skill only while it
is unedited. User-written or edited skills are kept, including on uninstall.
For a harness without skill support, `jouet skill --init --format agents-md`
prints the onboarding instructions to give your agent in that setup session.

::: warning Breaking change before 1.0
The `jouet-init` skill replaces the `jouet init` command. Run `jouet setup` again
to install it, then invoke the skill in your agent.
:::
