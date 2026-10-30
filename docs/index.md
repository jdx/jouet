---
layout: home
title: A toolkit CLI for coding agents
description: One command for each chore coding agents keep rebuilding by hand, with output written for whoever is reading it.

hero:
  name: "jouet"
  text: "A toolkit CLI for coding agents"
  tagline: "Agents rebuild the same few chores in shell, and get them subtly wrong. jouet turns each one into a single command that never prompts, never truncates silently, and exits with a code an agent can act on."
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
  - title: jouet edit
    details: Exact, all-or-nothing edits across files from search/replace blocks, a Codex-style patch or JSON. A failed edit writes nothing and shows the closest match.
    link: /commands/edit
    linkText: Available now
  - title: jouet wait
    details: Wait for CI or another condition without sleep-polling, and hear straight away when a conflicted PR will never get CI.
    link: /commands/wait
    linkText: Coming soon
  - title: jouet pr status
    details: One line per PR with the reasons it needs attention, in place of a for loop over gh pr view.
    link: /commands/pr-status
    linkText: Available now
  - title: jouet pr threads
    details: List, reply to and resolve review threads without hand-written reviewThreads GraphQL.
    link: /commands/pr-threads
    linkText: Coming soon
---
