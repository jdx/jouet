---
layout: home
title: Wind-up tools for coding agents
description: jouet tells a coding agent which checks are slow before it runs them, and turns what went wrong in past sessions into rules, tasks and routines for the next one.

hero:
  name: 'jouet <span class="jouet-gloss">pronounced “zhweh” · French for “toy”</span>'
  text: "Wind-up tools for coding agents"
  tagline: "jouet tells an agent which builds and tests are slow before it runs them, and turns what keeps going wrong across sessions into rules, tasks and routines for the next one. It also has a command for each shell chore agents keep scripting by hand."
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
  - title: Know what a run costs
    details: jouet's hooks time each build, test and lint the agent runs. At session start the agent hears which ones are slow, so it can finish its edits and build once. <code>jouet audit</code> shows where past sessions' tokens and time went.
    link: /commands/cost
    linkText: How the timings work
  - title: Notice what keeps failing
    details: When a command keeps failing in session after session, jouet drafts a rule against it, and a command sequence the agent keeps retyping becomes a mise task. With session review on, the user's corrections become proposals too. You or the agent decide what to adopt, and a new rule only warns at first.
    link: /winding
    linkText: How self-winding works
  - title: Procedures the agent calls once
    details: A routine is something like a skill that runs, a short tested Luau program the agent calls instead of working through each step itself. The built-in <code>land</code> routine waits for CI and merges a PR once it is ready.
    link: /routines
    linkText: Write a routine
---
