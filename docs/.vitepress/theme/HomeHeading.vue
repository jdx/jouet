<script setup lang="ts">
defineProps<{ id: string; eyebrow: string; title: string }>();
</script>

<template>
  <div class="JouetHeading">
    <p class="jouet-eyebrow">{{ eyebrow }}</p>
    <h2 :id="id">{{ title }}</h2>
    <p v-if="$slots.default" class="jouet-lede"><slot /></p>
  </div>
</template>
