<script setup lang="ts">
// The desktop shell removes the native window frame (see shell_window.rs) and
// paints its own caption so the title bar joins the paper instead of the OS
// accent. The window buttons are visual only: the host answers their clicks
// through WM_NCHITTEST (HTMINBUTTON / HTMAXBUTTON / HTCLOSE), so their geometry
// must stay in step with hit_test() — 36px tall, three 46px buttons at the end.
import { i18n } from '../../core/i18n/index.ts';
</script>

<template>
  <div class="window-titlebar" role="presentation">
    <div class="wt-brand">
      <img class="wt-logo" src="/app-icons/mark.svg" alt="" draggable="false" />
      <span class="wt-title">{{ i18n.t('app.name') }}</span>
    </div>
    <div class="wt-spacer" />
    <div class="wt-controls" aria-hidden="true">
      <span class="wt-btn">
        <svg viewBox="0 0 10 10" width="10" height="10"><line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" stroke-width="1" /></svg>
      </span>
      <span class="wt-btn">
        <svg viewBox="0 0 10 10" width="10" height="10"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1" /></svg>
      </span>
      <span class="wt-btn wt-close">
        <svg viewBox="0 0 10 10" width="10" height="10"><line x1="0.5" y1="0.5" x2="9.5" y2="9.5" stroke="currentColor" stroke-width="1" /><line x1="9.5" y1="0.5" x2="0.5" y2="9.5" stroke="currentColor" stroke-width="1" /></svg>
      </span>
    </div>
  </div>
</template>
