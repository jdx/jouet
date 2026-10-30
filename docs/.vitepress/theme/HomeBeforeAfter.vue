<script setup lang="ts">
import { ref } from "vue";
import HomeHeading from "./HomeHeading.vue";
import { data as code, type Showcase } from "./showcase.data.mts";

// Each tab draws what happens when an agent runs today's script and when it
// runs the jouet command instead: a diagram first, then the outcome, then the
// code. The code and output come from showcase.data.mts.
type Tone = "ok" | "bad" | "warn" | "held" | "off";
type Side = {
  caption: string;
  badge: string;
  outcome: string;
  code: keyof Showcase;
  output?: keyof Showcase;
  note?: string;
};
type Example = {
  id: "edit" | "log" | "ci";
  tab: string;
  before: Side;
  after: Side;
  footnote?: string;
};

const examples: Example[] = [
  {
    id: "edit",
    tab: "jouet edit",
    before: {
      caption: "A Python heredoc edits one file at a time",
      badge: "exit 1",
      outcome:
        "Half edited. src/main.rs changed, Cargo.toml did not, and the error does not say so.",
      code: "editBefore",
      output: "editBeforeOutput",
      note: "inside python3 - <<'EOF'",
    },
    after: {
      caption: "jouet edit checks every edit, then writes all or none",
      badge: "exit 4",
      outcome:
        "Refused, so nothing changed. The closest match shows that Cargo.toml moved on to 0.1.1.",
      code: "editAfter",
      output: "editOutput",
      note: "the spec, on stdin",
    },
  },
  {
    id: "log",
    tab: "jouet wait log",
    before: {
      caption: "A grep loop reads the whole log, the last run included",
      badge: "done at 0s",
      outcome:
        "Too early: the old line matches before this server is up. If the line never comes, the loop never ends.",
      code: "logBefore",
    },
    after: {
      caption: "jouet wait log reads only lines written after it starts",
      badge: "met at 10s",
      outcome:
        "It prints the line that matched. If none comes by --timeout, it exits 124 instead of hanging.",
      code: "logAfter",
      output: "logOutput",
    },
  },
  {
    id: "ci",
    tab: "jouet wait ci",
    before: {
      caption: "The agent polls, and every check costs a model turn",
      badge: "7 turns",
      outcome:
        "Each turn re-reads the whole conversation. The failed job's log stays locked until the entire run ends, and a conflicted PR never gets CI, so the polling never stops.",
      code: "ciBefore",
    },
    after: {
      caption: "jouet wait ci blocks in one call and returns at the first failure",
      badge: "1 turn",
      outcome:
        "It returns with the failed step's log while the run is still going, and exits 4 straight away for a conflicted PR.",
      code: "ciAfter",
      output: "ciOutput",
    },
    footnote:
      "Illustrative timing: 60-second polls, a check that fails 3 minutes in, and a run that ends at 7.",
  },
];

// jouet edit: which step ran in which order, and how it ended.
type Step = { n?: number; tone: Tone; text: string };
const files = {
  before: [
    {
      path: "src/main.rs",
      check: { n: 1, tone: "ok", text: "assert passes" },
      write: { n: 2, tone: "warn", text: "written" },
    },
    {
      path: "Cargo.toml",
      check: { n: 3, tone: "bad", text: "AssertionError" },
      write: { tone: "off", text: "never runs" },
    },
  ],
  after: [
    {
      path: "src/main.rs",
      check: { n: 1, tone: "ok", text: "matches once" },
      write: { n: 3, tone: "held", text: "not written" },
    },
    {
      path: "Cargo.toml",
      check: { n: 2, tone: "bad", text: "no match" },
      write: { n: 3, tone: "bad", text: "failed" },
    },
  ],
} satisfies Record<string, { path: string; check: Step; write: Step }[]>;

// jouet wait log: app.log, oldest line first.
const log = [
  { time: "11:58:02", text: "Starting server", old: true },
  { time: "11:58:04", text: "Listening on http://localhost:3000", old: true },
  { time: "11:58:40", text: "Shutting down", old: true },
  { time: "12:06:22", text: "Starting server", old: false },
  { time: "12:06:31", text: "Listening on http://localhost:3000", old: false },
];

// jouet wait ci: minutes on a shared axis.
const runEnd = 7;
const failAt = 3;
const axisEnd = 8;
const pct = (minute: number) => `${(minute / axisEnd) * 100}%`;
const turns = Array.from({ length: runEnd }, (_, i) => i + 1);
const turnNotes = [
  { turns: "1–2", text: "pending" },
  { turns: "3", text: "a check failed" },
  { turns: "4–6", text: "--log-failed refuses: the run is still going" },
  { turns: "7", text: "the run ends; the log, at last" },
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
        <figure
          v-for="side in ['before', 'after'] as const"
          :key="side"
          class="side"
          :class="`is-${side}`"
        >
          <figcaption>
            <span class="tag">{{ side }}</span>
            <span>{{ example[side].caption }}</span>
          </figcaption>

          <!-- jouet edit: check and write, file by file -->
          <div v-if="example.id === 'edit'" class="diagram files">
            <span class="head" aria-hidden="true"></span>
            <span class="head">{{
              side === "before" ? "Check" : "1 · Check every edit"
            }}</span>
            <span class="head" aria-hidden="true"></span>
            <span class="head">{{
              side === "before" ? "Write" : "2 · Write all or none"
            }}</span>
            <template v-for="(file, row) in files[side]" :key="file.path">
              <code class="path" :style="{ gridRow: row + 2 }">{{
                file.path
              }}</code>
              <span
                class="step check"
                :class="`is-${file.check.tone}`"
                :style="{ gridRow: row + 2 }"
              >
                <span v-if="file.check.n" class="n">{{ file.check.n }}</span>
                {{ file.check.text }}
              </span>
              <span
                v-if="side === 'before'"
                class="link"
                :class="{ 'is-cut': file.write.tone === 'off' }"
                :style="{ gridRow: row + 2 }"
                aria-hidden="true"
              ></span>
              <span
                class="step write"
                :class="`is-${file.write.tone}`"
                :style="{ gridRow: row + 2 }"
              >
                <span v-if="file.write.n" class="n">{{ file.write.n }}</span>
                {{ file.write.text }}
              </span>
            </template>
            <span
              v-if="side === 'after'"
              class="gate"
              aria-hidden="true"
            ></span>
            <p class="files-note">
              {{
                side === "before"
                  ? "Each file is written as soon as its own assert passes."
                  : "1 of 2 edits failed, so no file is written."
              }}
            </p>
          </div>

          <!-- jouet wait log: app.log as a timeline, oldest line first -->
          <div v-else-if="example.id === 'log'" class="diagram logfile">
            <div class="logname"><code>app.log</code></div>
            <ol>
              <template v-for="(line, i) in log" :key="i">
                <li v-if="i === 3" class="start">
                  <span>{{
                    side === "before" ? "server restarts" : "the wait starts"
                  }}</span>
                </li>
                <li
                  class="line"
                  :class="{
                    'is-old': line.old,
                    'is-skipped': side === 'after' && line.old,
                    'is-unread': side === 'before' && !line.old,
                    'is-hit-bad': side === 'before' && i === 1,
                    'is-hit-ok': side === 'after' && i === 4,
                  }"
                >
                  <time>{{ line.time }}</time>
                  <span class="text">{{ line.text }}</span>
                  <span
                    v-if="side === 'before' && i === 1"
                    class="mark is-bad"
                  >
                    matched at 0s, from the last run
                  </span>
                  <span
                    v-else-if="side === 'after' && i === 4"
                    class="mark is-ok"
                  >
                    matched at 10s
                  </span>
                </li>
              </template>
            </ol>
            <p class="legend">
              {{
                side === "before"
                  ? "grep reads from the top, so the last run's line counts."
                  : "Lines already in the file are skipped."
              }}
            </p>
          </div>

          <!-- jouet wait ci: turns on a shared time axis -->
          <div v-else class="diagram ci">
            <div class="counter">
              <strong>{{ side === "before" ? runEnd : 1 }}</strong>
              <span>
                model {{ side === "before" ? "turns" : "turn" }}
                <br />
                log read at {{ side === "before" ? runEnd : failAt }} min
              </span>
            </div>
            <div class="track" aria-hidden="true">
              <span
                class="phase is-pending"
                :style="{ left: 0, width: pct(failAt) }"
              ></span>
              <span
                class="phase is-failed"
                :style="{ left: pct(failAt), width: pct(runEnd - failAt) }"
              ></span>
              <template v-if="side === 'before'">
                <span
                  v-for="turn in turns"
                  :key="turn"
                  class="turn"
                  :class="{ 'is-locked': turn > failAt && turn < runEnd }"
                  :style="{ left: pct(turn), '--i': turn }"
                  >{{ turn }}</span
                >
              </template>
              <template v-else>
                <span class="block" :style="{ width: pct(failAt) }">
                  <span>jouet wait ci</span>
                </span>
                <span
                  class="turn is-ok"
                  :style="{ left: pct(failAt), '--i': 1 }"
                  >1</span
                >
              </template>
            </div>
            <div class="axis" aria-hidden="true">
              <span
                v-for="minute in [0, 2, 4, 6, 8]"
                :key="minute"
                :style="{ left: pct(minute) }"
                >{{ minute }}m</span
              >
              <span class="fail-at" :style="{ left: pct(failAt) }"
                >check fails</span
              >
            </div>
            <ul v-if="side === 'before'" class="legend">
              <li v-for="note in turnNotes" :key="note.turns">
                <span class="n">{{ note.turns }}</span> {{ note.text }}
              </li>
            </ul>
            <ul v-else class="legend">
              <li>
                <span class="n">1</span> returns at the failure, with the
                failed step's log
              </li>
              <li>no turns spent while it waits</li>
            </ul>
          </div>

          <p class="outcome">
            <span class="badge">{{ example[side].badge }}</span>
            <span>{{ example[side].outcome }}</span>
          </p>

          <div class="code">
            <p v-if="example[side].note" class="note">
              {{ example[side].note }}
            </p>
            <!-- Highlighted at build time from snippets written in this repo. -->
            <div v-html="code[example[side].code]"></div>
            <div
              v-if="example[side].output"
              class="output"
              v-html="code[example[side].output!]"
            ></div>
          </div>
        </figure>
        <p v-if="example.footnote" class="footnote">{{ example.footnote }}</p>
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

/* Two figures whose rows line up: caption, diagram, outcome, code. */
.panel {
  column-gap: 20px;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  grid-template-rows: repeat(4, auto) auto;
}
.panel[hidden] {
  display: none;
}
.side {
  --accent: var(--jouet-coral);
  --accent-text: var(--vp-c-brand-1);
  background: var(--jouet-panel);
  border: 1px solid var(--vp-c-divider);
  border-radius: var(--jouet-radius);
  border-top: 3px solid var(--accent);
  box-shadow: var(--jouet-shadow);
  display: grid;
  gap: 16px;
  grid-row: span 4;
  grid-template-rows: subgrid;
  margin: 0;
  min-width: 0;
  padding: 18px;
}
.side.is-after {
  --accent: #6fcfc1;
  --accent-text: var(--vp-c-tip-1);
}
figcaption {
  align-items: baseline;
  color: var(--vp-c-text-1);
  display: flex;
  font-family: var(--jouet-display);
  font-size: 16px;
  font-weight: 600;
  gap: 10px;
  letter-spacing: -0.01em;
  line-height: 1.4;
}
.tag,
.badge {
  background: var(--accent);
  border-radius: 999px;
  color: var(--jouet-ink);
  flex: none;
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
  line-height: 20px;
  padding: 0 9px;
  text-transform: uppercase;
}
.badge {
  font-size: 12px;
  letter-spacing: 0.02em;
  line-height: 24px;
  padding: 0 10px;
  text-transform: none;
}

/* Diagrams share one frame. */
.diagram {
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  font-size: 13px;
  line-height: 1.4;
  min-width: 0;
  padding: 14px;
}
.n {
  align-items: center;
  border: 1px solid currentColor;
  border-radius: 999px;
  display: inline-flex;
  flex: none;
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  font-weight: 700;
  height: 18px;
  justify-content: center;
  min-width: 18px;
  padding: 0 4px;
}
.legend {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  margin: 12px 0 0;
}

/* jouet edit */
.files {
  align-content: center;
  align-items: center;
  display: grid;
  gap: 8px 0;
  grid-template-columns: max-content minmax(0, 1fr) 28px minmax(0, 1fr);
}
.files .head {
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 600;
  grid-row: 1;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}
.files .path {
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  grid-column: 1;
  padding-right: 12px;
}
.step {
  align-items: center;
  border: 1px solid;
  border-radius: 8px;
  display: flex;
  gap: 7px;
  min-height: 36px;
  min-width: 0;
  overflow-wrap: anywhere;
  padding: 6px 9px;
}
.step.is-ok {
  background: var(--vp-c-tip-soft);
  border-color: rgb(var(--jouet-teal-rgb) / 0.45);
  color: var(--vp-c-tip-1);
}
.step.is-bad {
  background: var(--vp-c-brand-soft);
  border-color: rgb(var(--jouet-coral-rgb) / 0.55);
  color: var(--vp-c-brand-1);
  font-weight: 600;
}
.step.is-warn {
  background: var(--vp-c-warning-soft);
  border-color: rgb(var(--jouet-brass-rgb) / 0.6);
  color: var(--vp-c-warning-1);
  font-weight: 600;
}
.step.is-held {
  background: var(--vp-c-default-soft);
  border-color: var(--vp-c-border);
  color: var(--vp-c-text-1);
}
.step.is-off {
  border-color: var(--vp-c-border);
  border-style: dashed;
  color: var(--vp-c-text-3);
}
.files .check {
  grid-column: 2;
}
.files .write {
  grid-column: 4;
}
.link {
  align-self: center;
  border-top: 2px solid var(--vp-c-text-3);
  grid-column: 3;
  margin: 0 4px;
  position: relative;
}
.link::after {
  border: 5px solid transparent;
  border-left-color: var(--vp-c-text-3);
  content: "";
  position: absolute;
  right: -6px;
  top: -6px;
}
.link.is-cut {
  border-top-style: dashed;
  opacity: 0.5;
}
/* The gate between checking and writing: closed, because a check failed. */
.gate {
  align-self: stretch;
  background: var(--jouet-coral);
  border-radius: 999px;
  grid-column: 3;
  grid-row: 2 / span 2;
  justify-self: center;
  width: 4px;
}
.files-note {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  grid-column: 1 / -1;
  grid-row: 4;
  margin: 4px 0 0;
}
.is-after .files-note {
  color: var(--vp-c-brand-1);
  font-weight: 600;
}

/* jouet wait log */
.logname {
  border-bottom: 1px solid var(--vp-c-divider);
  margin: -14px -14px 10px;
  padding: 8px 14px;
}
.logname code {
  color: var(--vp-c-text-2);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
}
.logfile ol {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  list-style: none;
  margin: 0;
  padding: 0;
}
.logfile .line {
  align-items: baseline;
  border-left: 3px solid transparent;
  border-radius: 4px;
  column-gap: 10px;
  display: flex;
  flex-wrap: wrap;
  padding: 3px 8px;
}
.logfile time {
  color: var(--vp-c-text-3);
  flex: none;
}
.logfile .text {
  color: var(--vp-c-text-1);
  min-width: 0;
  overflow-wrap: anywhere;
}
.logfile .is-skipped,
.logfile .is-unread {
  opacity: 0.45;
}
.logfile .is-skipped .text {
  text-decoration: line-through;
  text-decoration-color: var(--vp-c-text-3);
}
.logfile .is-hit-bad {
  background: var(--vp-c-brand-soft);
  border-left-color: var(--jouet-coral);
}
.logfile .is-hit-ok {
  background: var(--vp-c-tip-soft);
  border-left-color: var(--jouet-teal);
}
.mark {
  flex-basis: 100%;
  font-family: var(--vp-font-family-base);
  font-size: 12px;
  font-weight: 600;
  padding-left: calc(8ch + 10px);
}
.mark.is-bad {
  color: var(--vp-c-brand-1);
}
.mark.is-ok {
  color: var(--vp-c-tip-1);
}
.mark::before {
  content: "↑ ";
}
.logfile .start {
  align-items: center;
  color: var(--accent-text);
  display: flex;
  font-family: var(--vp-font-family-base);
  font-size: 11px;
  font-weight: 700;
  gap: 8px;
  letter-spacing: 0.06em;
  margin: 6px 0;
  text-transform: uppercase;
}
.logfile .start::before,
.logfile .start::after {
  border-top: 1px dashed currentColor;
  content: "";
  flex: 1;
}
.logfile .start::before {
  flex: 0 0 12px;
}

/* jouet wait ci */
.counter {
  align-items: center;
  display: flex;
  gap: 12px;
  margin-bottom: 14px;
}
.counter strong {
  color: var(--accent-text);
  font-family: var(--jouet-display);
  font-size: 44px;
  font-weight: 600;
  letter-spacing: -0.04em;
  line-height: 1;
}
.counter span {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.45;
}
.track {
  background: var(--vp-c-default-soft);
  border-radius: 6px;
  height: 34px;
  margin: 0 12px;
  position: relative;
}
.phase {
  border-radius: 6px;
  inset: 0 auto;
  position: absolute;
}
.phase.is-failed {
  background: repeating-linear-gradient(
    -45deg,
    rgb(var(--jouet-coral-rgb) / 0.16) 0 6px,
    rgb(var(--jouet-coral-rgb) / 0.06) 6px 12px
  );
  border-radius: 0 6px 6px 0;
}
.turn {
  align-items: center;
  animation: jouet-pop 0.3s calc(var(--i) * 90ms) both;
  background: var(--jouet-coral);
  border: 2px solid var(--jouet-panel);
  border-radius: 999px;
  color: var(--jouet-ink);
  display: flex;
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 700;
  height: 26px;
  justify-content: center;
  position: absolute;
  top: 4px;
  transform: translateX(-50%);
  width: 26px;
}
.turn.is-locked {
  background: var(--jouet-brass);
}
.turn.is-ok {
  animation-delay: 0.6s;
  background: #6fcfc1;
}
.block {
  align-items: center;
  animation: jouet-grow 0.6s ease-out both;
  background: linear-gradient(90deg, rgb(var(--jouet-teal-rgb) / 0.2), rgb(var(--jouet-teal-rgb) / 0.4));
  border-radius: 6px 0 0 6px;
  display: flex;
  inset: 0 auto 0 0;
  overflow: hidden;
  position: absolute;
  transform-origin: left;
}
.block span {
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 600;
  padding-left: 8px;
  white-space: nowrap;
}
.axis {
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 10.5px;
  height: 34px;
  margin: 4px 12px 0;
  position: relative;
}
.axis span {
  position: absolute;
  top: 0;
  transform: translateX(-50%);
}
.axis .fail-at {
  color: var(--vp-c-brand-1);
  font-family: var(--vp-font-family-base);
  font-weight: 600;
  top: 16px;
  white-space: nowrap;
}
.ci .legend {
  display: grid;
  gap: 4px;
  list-style: none;
  padding: 0;
}
.ci .legend li {
  align-items: center;
  display: flex;
  gap: 8px;
}
.ci .legend .n {
  color: var(--accent-text);
}
@keyframes jouet-pop {
  from {
    opacity: 0;
    transform: translateX(-50%) scale(0.4);
  }
}
@keyframes jouet-grow {
  from {
    transform: scaleX(0);
  }
}
@media (prefers-reduced-motion: reduce) {
  .turn,
  .block {
    animation: none;
  }
}

.outcome {
  align-items: flex-start;
  color: var(--vp-c-text-1);
  display: flex;
  font-size: 14px;
  gap: 12px;
  line-height: 1.6;
  margin: 0;
}
.outcome .badge {
  margin-top: 1px;
}

/* Code highlighted by Shiki, outside .vp-doc. */
.code {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
}
.note {
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
  margin: 0;
}
.code :deep(div[class*="language-"]) {
  background: var(--vp-code-block-bg);
  border: 1px solid var(--vp-c-divider);
  border-radius: 10px;
  overflow: hidden;
}
.code :deep(.copy),
.code :deep(.lang) {
  display: none;
}
.code :deep(pre) {
  margin: 0;
  overflow-x: auto;
  padding: 12px 0;
}
.code :deep(code) {
  display: block;
  font-family: var(--vp-font-family-mono);
  font-size: 12.5px;
  line-height: 1.65;
  min-width: max-content;
  padding: 0 16px;
}
.output :deep(div[class*="language-"]) {
  border-left: 3px solid var(--accent);
}
.code {
  --jouet-code-pad: 16px;
}
.footnote {
  color: var(--vp-c-text-3);
  font-size: 12.5px;
  grid-column: 1 / -1;
  margin: 14px 0 0;
}

@media (max-width: 899px) {
  .panel {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: none;
    row-gap: 20px;
  }
}
@media (max-width: 639px) {
  /* Reports wrap rather than scroll, so a phone shows them whole. */
  .output :deep(code) {
    min-width: 0;
    white-space: pre-wrap;
    word-break: break-word;
  }
}
@media (max-width: 479px) {
  /* Too narrow for the label beside the turn that ends the wait. */
  .block span {
    display: none;
  }
  .files-note {
    grid-row: auto;
  }
  .side {
    padding: 16px 14px;
  }
  .files {
    grid-template-columns: minmax(0, 1fr) 24px minmax(0, 1fr);
  }
  .files .head:first-child {
    display: none;
  }
  .files .path {
    grid-column: 1 / -1;
    grid-row: auto !important;
    margin-top: 4px;
  }
  .files .step,
  .files .link {
    grid-row: auto !important;
  }
  .files .check {
    grid-column: 1;
  }
  .files .link {
    grid-column: 2;
  }
  .files .write {
    grid-column: 3;
  }
  /* The note under the diagram says the same, and rows no longer line up. */
  .gate {
    display: none;
  }
  .step {
    font-size: 12px;
    gap: 5px;
    overflow-wrap: normal;
    padding: 6px 7px;
  }
  .files .head {
    grid-row: 1;
  }
  .files .head:nth-child(2) {
    grid-column: 1;
  }
  .files .head:nth-child(4) {
    grid-column: 3;
  }
}
</style>
