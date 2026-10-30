---
layout: home
title: Wind-up tools for coding agents
description: jouet turns the shell chores coding agents keep scripting by hand into single commands that never prompt and exit with a code an agent can act on.

hero:
  name: 'jouet <span class="jouet-gloss">pronounced “zhweh” · French for “toy”</span>'
  text: "Wind-up tools for coding agents"
  tagline: "Agents keep rebuilding the same shell chores by hand, and get them subtly wrong. jouet turns each one into a single command that never prompts, never cuts output silently, and exits with a code an agent can act on."
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
  - title: 'jouet edit <span class="jouet-status is-ready">available</span>'
    details: Exact, all-or-nothing edits across files, from search/replace blocks, a Codex-style patch or JSON. A failed edit writes nothing and shows the closest match.
    link: /commands/edit
    linkText: Read the guide
  - title: 'jouet wait <span class="jouet-status is-ready">available</span>'
    details: Blocks until a file, log line, port, URL, process or command is ready, then prints one summary. Waiting on CI, with the failed step's log, is coming soon.
    link: /commands/wait
    linkText: Read the guide
  - title: 'jouet pr <span class="jouet-status is-soon">coming soon</span>'
    details: <code>pr status</code> gives one line per PR with what needs attention. <code>pr threads</code> lists, replies to and resolves review threads without hand-written GraphQL.
    link: /commands/pr-status
    linkText: What it will do
  - title: 'Agent steering <span class="jouet-status is-later">later</span>'
    details: Hooks that catch an agent's costly patterns and point it to the command that replaces them, and rules you write yourself.
    link: /why#what-comes-next
    linkText: The plan
---
