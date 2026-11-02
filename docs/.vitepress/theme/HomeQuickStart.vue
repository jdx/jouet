<script setup lang="ts">
import { ref } from "vue";

const command = "mise use -g jouet";
const status = ref("");

async function copyCommand() {
  try {
    await navigator.clipboard.writeText(command);
    status.value = "Copied";
  } catch {
    status.value = "Select the command to copy it.";
  }
}
</script>

<template>
  <section class="JouetQuickStart" aria-label="Install with mise">
    <div class="install-command">
      <code>{{ command }}</code>
      <button
        type="button"
        aria-label="Copy the installation command"
        @click="copyCommand"
      >
        Copy
      </button>
    </div>
    <p class="install-note">
      <span>mise 2026.10.25+ · Linux · macOS (Apple Silicon) · Windows</span>
      <a href="/getting-started"
        >More install options <span aria-hidden="true">→</span></a
      >
    </p>
    <p class="copy-status" role="status">{{ status }}</p>
    <p class="early-note">
      <strong>Early development.</strong> Benefits vary by project. Your ecosystem
      may need configuration or changes to jouet before you see savings.
      Compatibility may change before 1.0.
      <a href="/why#early-development">What to expect →</a>
    </p>
  </section>
</template>

<style scoped>
.JouetQuickStart {
  margin-top: 28px;
  max-width: 540px;
}
.install-command {
  align-items: center;
  background: var(--jouet-panel);
  border: 1px solid var(--vp-c-border);
  border-radius: 10px;
  box-shadow: var(--jouet-shadow);
  display: flex;
  gap: 12px;
  padding: 10px 10px 10px 16px;
  text-align: left;
}
code {
  color: var(--vp-c-text-1);
  flex: 1;
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  line-height: 1.8;
  overflow-wrap: anywhere;
  user-select: all;
}
code::before {
  color: var(--vp-c-text-3);
  content: "$ ";
  user-select: none;
}
button {
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  color: var(--vp-c-brand-1);
  cursor: pointer;
  flex-shrink: 0;
  font-size: 12px;
  min-height: 44px;
  padding: 0 12px;
}
button:hover {
  border-color: var(--vp-c-brand-1);
}
.install-note {
  color: var(--vp-c-text-2);
  display: flex;
  flex-wrap: wrap;
  font-size: 12px;
  gap: 8px 20px;
  justify-content: space-between;
  line-height: 1.6;
  margin-top: 12px;
}
a {
  color: var(--vp-c-brand-1);
}
a:hover {
  text-decoration: underline;
}
.copy-status {
  color: var(--vp-c-brand-1);
  font-size: 12px;
  min-height: 20px;
  text-align: left;
}
.early-note {
  border-left: 2px solid var(--jouet-brass);
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.65;
  margin-top: 8px;
  padding-left: 12px;
  text-align: left;
}
@media (max-width: 959px) {
  .JouetQuickStart {
    margin-left: auto;
    margin-right: auto;
  }
  .install-note {
    justify-content: center;
    text-align: center;
  }
}
</style>
