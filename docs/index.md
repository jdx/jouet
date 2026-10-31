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
    details: Blocks until CI finishes, a PR is ready, or a file, log line, port, URL, process or command is, then prints one summary. A CI failure comes with the failed step's log.
    link: /commands/wait
    linkText: Read the guide
  - title: 'jouet pr status <span class="jouet-status is-ready">available</span>'
    details: One line per PR with what needs attention, from one batched query. <code>pr threads</code> lists, replies to and resolves review threads without hand-written GraphQL.
    link: /commands/pr-status
    linkText: Read the guide
  - title: 'jouet setup <span class="jouet-status is-ready">available</span>'
    details: Hooks for Claude Code, Codex, Cursor, Gemini CLI and Copilot. Rules deny an agent's costly habits and name the command to use instead, and it cannot finish while its last test run failed.
    link: /setup
    linkText: Read the guide
---
