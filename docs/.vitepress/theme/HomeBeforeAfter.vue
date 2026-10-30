<script setup lang="ts">
import { ref } from "vue";
import HomeHeading from "./HomeHeading.vue";

// Each example pairs what agents write today with the jouet command that
// replaces it. Output shown for available commands is real jouet output.
const examples = [
  {
    id: "edit",
    tab: "jouet edit",
    status: null,
    before: {
      caption: "A Python heredoc, with the assert agents add by hand",
      code: `python3 - <<'EOF'
from pathlib import Path
for path, old, new in [
    ("src/main.rs", "let x = 1;", "let x = 2;"),
    ("Cargo.toml", 'version = "0.1.0"', 'version = "0.2.0"'),
]:
    s = Path(path).read_text()
    assert s.count(old) == 1, path
    Path(path).write_text(s.replace(old, new))
EOF`,
      output: "AssertionError: Cargo.toml",
      outcome:
        "Cargo.toml changed since the agent read it, so the script stops there. src/main.rs was already rewritten: the tree is half edited.",
    },
    after: {
      caption: "The same two edits as search/replace blocks",
      code: `jouet edit <<'JOUET_EDIT'
=== src/main.rs
<<<<<<< SEARCH
    let x = 1;
=======
    let x = 2;
>>>>>>> REPLACE
=== Cargo.toml
<<<<<<< SEARCH
version = "0.1.0"
=======
version = "0.2.0"
>>>>>>> REPLACE
JOUET_EDIT`,
      output: `not written src/main.rs  1 edit  +1 -1
failed      Cargo.toml   1 edit
  edit 1: no match
    closest at line 3 (94% similar), - spec / + file:
    -version = "0.1.0"
    +version = "0.1.1"
0 of 2 files written`,
      outcome:
        "jouet checks every edit before it writes any file. It writes nothing, exits 4 (refused), and shows the line that changed.",
    },
  },
  {
    id: "wait-log",
    tab: "jouet wait log",
    status: null,
    before: {
      caption: "A grep loop, with the last run's lines still in the log",
      code: `until grep -q 'Listening on' app.log; do
  sleep 2
done`,
      output: null,
      outcome:
        "The old line matches straight away, so the loop exits before this server is up. If the server never gets there, it never ends.",
    },
    after: {
      caption: "Only lines written after the wait starts count",
      code: "jouet wait log app.log --match 'Listening on' --timeout 2m",
      output: `[0s] log app.log: following
[10s] log app.log: matched
MET log app.log: matched (10s)
  12:06:31 Listening on http://localhost:3000`,
      outcome:
        "It waits for a new matching line and prints it. If none comes, it stops at the timeout with exit 124.",
    },
  },
  {
    id: "wait-ci",
    tab: "jouet wait ci",
    status: null,
    before: {
      caption: "A poll loop around gh",
      code: `until gh pr checks 42; do
  sleep 60
done`,
      output: null,
      outcome:
        "This only stops when every check passes. A failing check, or a conflicted PR that never gets CI, keeps it polling until the harness times it out.",
    },
    after: {
      caption: "Back at the first failure, with the failed step's log",
      code: "jouet wait ci 15195 --repo jdx/mise --check release",
      output: `FAIL jdx/mise#15195 @ 843cc16  0 pass, 1 fail, 0 skipped, 0 pending  (3s)

FAIL release  step "Require auto-merge on the release PR"  required
  https://github.com/jdx/mise/actions/runs/41083340820/job/124157989724
  ##[error]Enable auto-merge on this pull request to build and test the release tarballs.
  ##[error]Process completed with exit code 1.
  full log: ~/.local/state/jouet/logs/jdx-mise-job-124157989724.log`,
      outcome:
        "It follows the PR head, returns at the first failure with the failed step's log, and reports a conflicted PR straight away.",
    },
  },
];

const active = ref(0);
const tabs = ref<HTMLButtonElement[]>([]);

function select(index: number, focus = false) {
  active.value = (index + examples.length) % examples.length;
  if (focus) tabs.value[active.value]?.focus();
}
</script>

<template>
  <section
    class="jouet-section JouetBeforeAfter"
    aria-labelledby="jouet-before-after"
  >
    <div class="jouet-container">
      <HomeHeading
        id="jouet-before-after"
        eyebrow="Before and after"
        title="The chore, minus the script"
      >
        Agents write these scripts by hand in session after session. They look
        right and usually work, which is why the failures are easy to miss.
      </HomeHeading>

      <div class="tabs" role="tablist" aria-label="Examples">
        <button
          v-for="(example, index) in examples"
          :id="`jouet-tab-${example.id}`"
          :key="example.id"
          ref="tabs"
          type="button"
          role="tab"
          :aria-selected="active === index"
          :aria-controls="`jouet-panel-${example.id}`"
          :tabindex="active === index ? 0 : -1"
          @click="select(index)"
          @keydown.right.prevent="select(index + 1, true)"
          @keydown.left.prevent="select(index - 1, true)"
        >
          <code>{{ example.tab }}</code>
          <span v-if="example.status" class="jouet-status is-soon">{{
            example.status
          }}</span>
        </button>
      </div>

      <div
        v-for="(example, index) in examples"
        :id="`jouet-panel-${example.id}`"
        :key="example.id"
        class="panel"
        role="tabpanel"
        :aria-labelledby="`jouet-tab-${example.id}`"
        :hidden="active !== index"
      >
        <figure class="side is-before">
          <figcaption>
            <span class="tag">Before</span>{{ example.before.caption }}
          </figcaption>
          <pre class="term"><code>{{ example.before.code }}</code></pre>
          <pre
            v-if="example.before.output"
            class="term is-output is-bad"
          ><code>{{ example.before.output }}</code></pre>
          <p class="outcome">{{ example.before.outcome }}</p>
        </figure>
        <figure class="side is-after">
          <figcaption>
            <span class="tag">After</span>{{ example.after.caption }}
          </figcaption>
          <pre class="term"><code>{{ example.after.code }}</code></pre>
          <pre
            v-if="example.after.output"
            class="term is-output"
          ><code>{{ example.after.output }}</code></pre>
          <p class="outcome">{{ example.after.outcome }}</p>
        </figure>
      </div>
    </div>
  </section>
</template>

<style scoped>
.tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin: 28px 0 16px;
}
[role="tab"] {
  align-items: center;
  background: transparent;
  border: 1px solid var(--vp-c-border);
  border-radius: 10px;
  color: var(--vp-c-text-2);
  cursor: pointer;
  display: inline-flex;
  font-size: 14px;
  min-height: 40px;
  padding: 0 14px;
}
[role="tab"] code {
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
}
[role="tab"]:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-text-1);
}
[role="tab"][aria-selected="true"] {
  background: var(--vp-c-brand-soft);
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-text-1);
}
.panel {
  display: grid;
  gap: 20px;
  grid-template-columns: repeat(2, minmax(0, 1fr));
}
.panel[hidden] {
  display: none;
}
.side {
  background: var(--jouet-panel);
  border: 1px solid var(--vp-c-divider);
  border-radius: var(--jouet-radius);
  box-shadow: var(--jouet-shadow);
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin: 0;
  min-width: 0;
  padding: 18px;
}
figcaption {
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
}
.tag {
  border-radius: 999px;
  color: var(--jouet-ink);
  display: inline-block;
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
  line-height: 20px;
  margin-right: 10px;
  padding: 0 9px;
  text-transform: uppercase;
}
.is-before .tag {
  background: var(--jouet-coral);
}
.is-after .tag {
  background: #6fcfc1;
}
.term {
  background: #1b1c22;
  border-radius: 10px;
  color: #efeae4;
  margin: 0;
  overflow-x: auto;
  padding: 14px 16px;
}
.term code {
  font-family: var(--vp-font-family-mono);
  font-size: 12.5px;
  line-height: 1.65;
  white-space: pre;
}
.term.is-output {
  background: #121317;
  border-left: 3px solid #6fcfc1;
  color: #d8d2cb;
}
.term.is-output.is-bad {
  border-left-color: var(--jouet-coral);
  color: #ff8f7c;
}
.outcome {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.65;
  margin: 0;
}
.is-before .outcome::before,
.is-after .outcome::before {
  font-weight: 700;
  margin-right: 8px;
}
.is-before .outcome::before {
  color: var(--vp-c-brand-1);
  content: "✕";
}
.is-after .outcome::before {
  color: var(--vp-c-tip-1);
  content: "✓";
}
@media (max-width: 899px) {
  .panel {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
