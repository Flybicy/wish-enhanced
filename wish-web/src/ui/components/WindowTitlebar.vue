<script setup lang="ts">
// The desktop shell removes the native window frame (see shell_window.rs) and
// paints its own caption so the title bar joins the paper instead of the OS
// accent. Because the WebView2 view fills the whole client area, its child
// window swallows every mouse event over this strip, so the native frame's
// hit-testing never sees them. Instead the caption talks to the host directly
// over the WebView2 web-message channel: dragging and the three window buttons
// post an intent the shell acts on (ShowWindow / WM_CLOSE / caption move).
import { i18n } from '../../core/i18n/index.ts';

type Host = { postMessage?: (message: unknown) => void };
const host = (): Host | undefined =>
  (window as { chrome?: { webview?: Host } }).chrome?.webview;
const post = (intent: string) => host()?.postMessage?.('wish:' + intent);

// Left mouse on the caption (but not on a button) begins a window drag; the
// host answers with the standard caption move loop. A double-click toggles
// maximize, matching a normal title bar.
function onCaptionDown(event: MouseEvent) {
  if (event.button !== 0) return;
  if ((event.target as HTMLElement).closest('.wt-controls')) return;
  // The second mousedown of a double-click toggles maximize instead of
  // starting another drag, so the move loop never races the toggle.
  if (event.detail === 2) {
    post('maximize');
    return;
  }
  post('drag');
}
</script>

<template>
  <div class="window-titlebar" role="presentation" @mousedown="onCaptionDown">
    <div class="wt-brand">
      <img class="wt-logo" src="/app-icons/mark.svg" alt="" draggable="false" />
      <span class="wt-title">{{ i18n.t('app.name') }}</span>
    </div>
    <div class="wt-spacer" />
    <div class="wt-controls">
      <button type="button" class="wt-btn" :aria-label="i18n.t('window.minimize')" @click="post('minimize')">
        <svg viewBox="0 0 10 10" width="10" height="10"><line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" stroke-width="1" /></svg>
      </button>
      <button type="button" class="wt-btn" :aria-label="i18n.t('window.maximize')" @click="post('maximize')">
        <svg viewBox="0 0 10 10" width="10" height="10"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1" /></svg>
      </button>
      <button type="button" class="wt-btn wt-close" :aria-label="i18n.t('window.close')" @click="post('close')">
        <svg viewBox="0 0 10 10" width="10" height="10"><line x1="0.5" y1="0.5" x2="9.5" y2="9.5" stroke="currentColor" stroke-width="1" /><line x1="9.5" y1="0.5" x2="0.5" y2="9.5" stroke="currentColor" stroke-width="1" /></svg>
      </button>
    </div>
  </div>
</template>
