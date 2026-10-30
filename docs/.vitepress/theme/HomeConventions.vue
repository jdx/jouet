<script setup lang="ts">
import HomeHeading from "./HomeHeading.vue";

const rules = [
  {
    title: "Never prompts",
    html: "Every command runs without a TTY: under an agent harness, in CI or in a pipe.",
  },
  {
    title: "One table of exit codes",
    html: "<code>1</code> means the answer is no. <code>4</code> means jouet refused and changed nothing.",
  },
  {
    title: "One JSON document",
    html: "<code>--json</code> prints exactly one document on stdout, errors included.",
  },
  {
    title: "Nothing cut silently",
    html: "Shortened output says so and names the log file that holds the rest.",
  },
];
</script>

<template>
  <section
    class="jouet-section JouetConventions"
    aria-labelledby="jouet-conventions"
  >
    <div class="jouet-container">
      <HomeHeading
        id="jouet-conventions"
        eyebrow="Built for an agent's shell"
        title="Learn the rules once"
      >
        Every jouet command follows the same conventions, so an agent that knows
        one command knows how the others behave.
      </HomeHeading>
      <ul>
        <li v-for="rule in rules" :key="rule.title">
          <h3>{{ rule.title }}</h3>
          <!-- The rule text is written here, not user input. -->
          <p v-html="rule.html"></p>
        </li>
      </ul>
      <a class="more" href="/conventions"
        >Read the shared conventions <span aria-hidden="true">→</span></a
      >
    </div>
  </section>
</template>

<style scoped>
.JouetConventions {
  padding-bottom: 88px;
}
ul {
  border-top: 1px solid var(--vp-c-divider);
  display: grid;
  gap: 0 32px;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  list-style: none;
  margin: 32px 0 0;
  padding: 0;
}
li {
  border-top: 3px solid transparent;
  border-image: var(--jouet-stripe) 1;
  border-image-width: 3px 0 0 0;
  margin-top: -1px;
  padding-top: 18px;
}
h3 {
  color: var(--vp-c-text-1);
  font-family: var(--jouet-display);
  font-size: 17px;
  font-weight: 600;
  letter-spacing: -0.02em;
  line-height: 1.4;
  margin: 0 0 6px;
}
li p {
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.65;
  margin: 0;
}
li p :deep(code) {
  background: var(--vp-c-default-soft);
  border-radius: 4px;
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-size: 0.88em;
  padding: 1px 5px;
}
.more {
  color: var(--vp-c-brand-1);
  display: inline-block;
  font-size: 14px;
  font-weight: 500;
  margin-top: 28px;
}
.more:hover {
  text-decoration: underline;
  text-underline-offset: 4px;
}
@media (max-width: 959px) {
  ul {
    gap: 24px 32px;
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
@media (max-width: 559px) {
  ul {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
