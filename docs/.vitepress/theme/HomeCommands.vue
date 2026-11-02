<script setup lang="ts">
import HomeHeading from "./HomeHeading.vue";

// The commands, grouped as in the sidebar, each with the habit it replaces.
const groups = [
  {
    title: "Code and data",
    commands: [
      ["edit", "/commands/edit", "Exact edits across files that all land or none do"],
      ["search", "/commands/search", "rg, grouped by function and cut to a budget"],
      ["read", "/commands/read", "A file's outline, one symbol, or a repository map"],
      ["graph", "/commands/graph", "Callers, dependencies and the tests a change reaches"],
      ["diag", "/commands/diag", "The lint findings in just the files you changed"],
      ["docs", "/commands/docs", "A dependency's API at the version you locked"],
      ["deps", "/commands/deps", "Check a package exists, and its advisories, before adding it"],
      ["db", "/commands/db", "A schema in a few lines, and read-only queries"],
      ["http", "/commands/http", "The status and only the JSON paths asked for"],
    ],
  },
  {
    title: "Running and waiting",
    commands: [
      ["test", "/commands/test", "Only the tests a change affects, and one line when they pass"],
      ["wait", "/commands/wait", "CI, logs, ports and processes, in one call"],
      ["job", "/commands/job", "A build or server in the background, with an optional ready check"],
      ["ci run", "/commands/ci", "Run a workflow's jobs here before pushing"],
      ["ci explain", "/commands/ci#jouet-ci-explain", "Tell a flaky CI failure from a real one"],
      ["task", "/commands/task", "Turn a retyped command sequence into a mise task"],
    ],
  },
  {
    title: "Pull requests",
    commands: [
      ["pr status", "/commands/pr-status", "Many PRs, one line each"],
      ["pr threads", "/commands/pr-threads", "List, reply to and resolve review threads"],
      ["pr next", "/commands/pr-next", "The one thing a PR needs next"],
      ["pr land", "/commands/pr-land", "Merge a PR once it is ready"],
      ["stack", "/commands/stack", "Check that each layer of a stack has the one below"],
      ["review-package", "/commands/review-package", "One file with everything a reviewer needs"],
    ],
  },
  {
    title: "Git and sessions",
    commands: [
      ["commit", "/commands/commit", "Commit only the paths you name"],
      ["secrets", "/commands/secrets", "Catch a key or token before it is committed"],
      ["handoff", "/commands/handoff", "Where the work stands, for the next session"],
      ["sessions", "/commands/sessions", "Which sessions are working where on this machine"],
      ["claim", "/commands/claim", "Warn other sessions off the files you are editing"],
    ],
  },
  {
    title: "Measuring",
    commands: [
      ["cost", "/commands/cost", "How long each build, test and lint takes here"],
      ["audit", "/commands/audit", "Where past sessions spent tokens and time"],
      ["stats", "/commands/stats", "What jouet saved, estimated from your history"],
    ],
  },
] as const;
</script>

<template>
  <section class="jouet-section JouetCommands" aria-labelledby="jouet-all-commands">
    <div class="jouet-container">
      <HomeHeading
        id="jouet-all-commands"
        eyebrow="Commands"
        title="One command per chore"
      >
        Each replaces a script or a loop agents write over and over. The hooks
        name the command when an agent hand-rolls one anyway.
      </HomeHeading>
      <div class="groups">
        <section v-for="group in groups" :key="group.title" class="group">
          <h3>{{ group.title }}</h3>
          <ul>
            <li v-for="[name, link, text] in group.commands" :key="name">
              <a :href="link">
                <code>jouet {{ name }}</code>
                <span>{{ text }}</span>
              </a>
            </li>
          </ul>
        </section>
      </div>
      <a class="more" href="/cli/"
        >Every command and flag in the CLI reference
        <span aria-hidden="true">→</span></a
      >
    </div>
  </section>
</template>

<style scoped>
.groups {
  columns: 3 300px;
  column-gap: 32px;
  margin-top: 32px;
}
.group {
  break-inside: avoid;
  margin-bottom: 28px;
}
h3 {
  border-top: 3px solid transparent;
  border-image: var(--jouet-stripe) 1;
  border-image-width: 3px 0 0 0;
  color: var(--vp-c-text-1);
  font-family: var(--jouet-display);
  font-size: 17px;
  font-weight: 600;
  letter-spacing: -0.02em;
  line-height: 1.4;
  margin: 0 0 8px;
  padding-top: 14px;
}
ul {
  list-style: none;
  margin: 0;
  padding: 0;
}
li a {
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0 -10px;
  padding: 7px 10px;
  transition: background-color 0.2s;
}
li a:hover {
  background: var(--vp-c-default-soft);
}
code {
  color: var(--vp-c-brand-1);
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  font-weight: 600;
}
li span {
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
}
.more {
  color: var(--vp-c-brand-1);
  display: inline-block;
  font-size: 14px;
  font-weight: 500;
  margin-top: 4px;
}
.more:hover {
  text-decoration: underline;
  text-underline-offset: 4px;
}
</style>
