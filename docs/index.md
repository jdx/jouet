---
layout: home
title: Spend fewer tokens. Get there faster.
description: jouet helps coding agents use fewer tokens and finish work faster with a code graph, checked edits, executable workflows and improvements learned across sessions.

hero:
  name: 'jouet <span class="jouet-gloss">pronounced “zhweh” · French for “toy”</span>'
  text: "Spend fewer tokens. Get there faster."
  tagline: "A toolkit that helps your coding agent read less, skip repetitive work and learn from its mistakes across sessions. A code graph, checked edits and reusable workflows, through the shell it already uses."
  image:
    src: /logo.svg
    alt: The jouet logo, a coral wind-up tin robot with a brass key on top
  actions:
    - theme: brand
      text: Get started
      link: /getting-started
    - theme: alt
      text: Why jouet
      link: /why

features:
  - title: Less context, fewer tokens
    details: Budgeted searches and reads show the relevant definitions and remember what the session already saw. Test output puts failures first and saves the full log for when it is needed.
    link: /commands/search
    linkText: Search without flooding context
  - title: Less waiting, fewer turns
    details: One call waits for CI and returns the failed step's log. Batched PR queries replace per-PR loops, and affected-test selection shortens the agent's edit and test cycle.
    link: /commands/test
    linkText: Run the tests a change affects
  - title: A code graph to navigate
    details: Find callers, dependencies and the tests a change reaches. Build a repository map within a token budget so the agent can explore through focused queries. No daemon or language server to run.
    link: /commands/graph
    linkText: Explore the code graph
  - title: Better habits, built in
    details: A simple replacement takes one command instead of a Python heredoc. Hooks catch costly patterns as they happen and point the agent to checked edits, waits and other tools.
    link: /setup
    linkText: Set up your agent
  - title: Like skills with executable workflows
    details: Turn a repeated procedure into a tested Luau routine over jouet's primitives. The agent calls it once, with declared capabilities and previews for writes, instead of reasoning through each step again.
    link: /routines
    linkText: Write a routine
  - title: Improves across sessions
    details: A local ledger spots repeated failures and costly habits. jouet proposes rules and tasks; session review can suggest routines and project guidance. You or your agent choose what to adopt. New rules warn first.
    link: /winding
    linkText: How self-winding works
---
