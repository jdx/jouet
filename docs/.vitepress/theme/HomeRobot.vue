<script setup lang="ts">
import { onMounted, ref } from "vue";

// The robot winds its key a few turns when the page opens, and again when the
// pointer reaches it. Reduced motion keeps it still.
const winding = ref(false);

function wind() {
  if (winding.value) return;
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  winding.value = true;
}

onMounted(wind);
</script>

<template>
  <svg
    class="image-src JouetRobot"
    :class="{ winding }"
    viewBox="0 0 128 128"
    role="img"
    aria-labelledby="jouet-robot-title"
    @mouseenter="wind"
  >
    <title id="jouet-robot-title">
      The jouet logo, a coral wind-up tin robot with a brass key on top
    </title>
    <g class="key" @animationend="winding = false">
      <path
        fill="#f2b84b"
        fill-rule="evenodd"
        d="M60 15C56 11 52 8 47 8A12 12 0 0 0 47 32C52 32 56 29 60 25V36H68V25C72 29 76 32 81 32A12 12 0 0 0 81 8C76 8 72 11 68 15zM42.5 20a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0zM76.5 20a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0z"
      />
    </g>
    <rect x="55" y="33" width="18" height="9" rx="2.5" fill="#d9962b" />
    <rect x="6" y="64" width="12" height="28" rx="5" fill="#2a9d8f" />
    <rect x="110" y="64" width="12" height="28" rx="5" fill="#2a9d8f" />
    <rect x="14" y="40" width="100" height="78" rx="20" fill="#ef6a55" />
    <rect x="26" y="52" width="76" height="54" rx="14" fill="#fbf3e6" />
    <ellipse cx="37" cy="90" rx="5" ry="3.5" fill="#f6a596" />
    <ellipse cx="91" cy="90" rx="5" ry="3.5" fill="#f6a596" />
    <circle cx="48" cy="74" r="9" fill="#1d0f0c" />
    <circle cx="80" cy="74" r="9" fill="#1d0f0c" />
    <circle cx="51" cy="71" r="3" fill="#fff" />
    <circle cx="83" cy="71" r="3" fill="#fff" />
    <path
      d="M55 91q9 8 18 0"
      fill="none"
      stroke="#1d0f0c"
      stroke-width="4"
      stroke-linecap="round"
    />
  </svg>
</template>

<style scoped>
.JouetRobot {
  filter: drop-shadow(0 22px 26px rgb(29 15 12 / 0.28));
  height: 300px;
  overflow: visible;
  width: 300px;
}
.dark .JouetRobot {
  filter: drop-shadow(0 24px 30px rgb(0 0 0 / 0.45));
}
.key {
  transform-box: fill-box;
  transform-origin: center;
}
/* Seen from the front, a key turning on its stem narrows to an edge and
   widens again, darkening as it turns away. */
.winding .key {
  animation: jouet-wind 0.7s ease-in-out 3;
}
@keyframes jouet-wind {
  0%,
  100% {
    transform: scaleX(1);
    filter: none;
  }
  25%,
  75% {
    transform: scaleX(0.12);
    filter: brightness(0.8);
  }
  50% {
    transform: scaleX(-1);
    filter: brightness(0.92);
  }
}
@media (max-width: 959px) {
  .JouetRobot {
    height: 220px;
    width: 220px;
  }
}
@media (max-width: 639px) {
  .JouetRobot {
    height: 172px;
    width: 172px;
  }
}
@media (prefers-reduced-motion: reduce) {
  .winding .key {
    animation: none;
  }
}
</style>
