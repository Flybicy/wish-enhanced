<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import WorkspaceTree from './WorkspaceTree.vue';
import { get } from '../../core/api/client.ts';
import { chat } from '../../core/state/chatSlice.ts';
import { prefs } from '../../core/state/prefsSlice.ts';
import { tr } from '../../core/i18n/tr.ts';
import Icon from '../../ui/components/Icon.vue';

// The desk beside the conversation: the shelf of files at the top, the loose
// note taped along the bottom, and a ruler of numbers under it. Only what can
// be acted on lives here — the note takes writing, the file tree takes opening,
// and the foot reports meters. Capability flags are deliberately absent: they
// are read-only, so the model is told them and the reader is not shown them.
const props = defineProps<{ sessionId: string }>();

const snapshot = computed(() => chat.snapshot.value);
const usage = ref<any>(null);
const calls = ref<any[]>([]);
// How many entries (tool calls and messages) the run has walked through.
const steps = ref(0);

const cwd = computed(() => (snapshot.value?.descriptor as any)?.cwd ?? '');
const folderName = computed(() => {
  const path = cwd.value.replace(/[\\/]+$/, '');
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
});

// The foot reads like a tachometer: how many calls, how fast the tokens came
// back, how much traffic there was, and how much of it the cache served.
const speed = computed(() => {
  let output = 0;
  let elapsed = 0;
  for (const call of calls.value) {
    const produced = call?.usage?.output_tokens ?? 0;
    if (produced > 0 && call?.elapsed_ms) { output += produced; elapsed += call.elapsed_ms; }
  }
  return elapsed > 0 ? (output / (elapsed / 1000)) : null;
});
const stats = computed(() => {
  const s = usage.value?.statistics;
  const totals = s?.totals?.tokens;
  const cache = s?.totals?.cache;
  const read = cache?.read_input_tokens ?? 0;
  const input = totals?.input_tokens ?? 0;
  return {
    calls: s?.model_attempts ?? 0,
    input: input,
    output: totals?.output_tokens ?? 0,
    reasoning: totals?.reasoning_tokens ?? 0,
    cacheHit: read + input > 0 ? Math.round((read / (read + input)) * 100) : null,
  };
});

async function load(id: string) {
  if (!id) return;
  try {
    const [u, c, e] = await Promise.all([
      get('/sessions/' + encodeURIComponent(id) + '/usage'),
      get('/sessions/' + encodeURIComponent(id) + '/calls', { query: { limit: 200 } }),
      get('/sessions/' + encodeURIComponent(id) + '/entries', { query: { limit: 1 } }),
    ]);
    usage.value = u;
    calls.value = c?.items ?? [];
    // The last page's cursor is the count of entries before it.
    steps.value = Number(e?.next ?? e?.start ?? 0) + (e?.items?.length ?? 0);
  } catch { /* the rail is a convenience; a missing endpoint must not break the pane */ }
}
watch(() => props.sessionId, load, { immediate: true });

const copying = ref('');
async function copyPath() {
  try { await navigator.clipboard.writeText(cwd.value); copying.value = tr('已复制', 'Copied'); }
  catch { copying.value = tr('失败', 'Failed'); }
  setTimeout(() => { copying.value = ''; }, 1400);
}

// 笺 — the note taped along the rail's foot, opened on demand.
const noteOpen = ref(prefs.btwRail.value);
watch(noteOpen, value => prefs.setBtwRail(value));
// One readable number for total traffic: 3.6k under a million, 78.9M above it.
const totalTokens = computed(() => {
  const total = stats.value.input + stats.value.output;
  if (total >= 1_000_000) return (total / 1_000_000).toFixed(1) + 'M tok';
  if (total >= 1_000) return (total / 1_000).toFixed(1) + 'k tok';
  return total + ' tok';
});
</script>

<template>
  <aside id="workspace-rail" class="workspace-rail" :aria-label="tr('工作区', 'Workspace')">
    <div class="rail-card rail-files">
      <button class="rail-path" type="button" :title="cwd" @click="copyPath">
        <Icon name="folder" />
        <span class="rail-path-text">
          <strong>{{ folderName || tr('未设置目录', 'No directory') }}</strong>
          <small>{{ cwd }}</small>
        </span>
        <span class="rail-path-hint">{{ copying || tr('复制', 'Copy') }}</span>
      </button>
      <WorkspaceTree :session-id="sessionId" />
    </div>

    <section class="rail-card rail-note" :class="{ open: noteOpen }">
      <button type="button" class="note-head" :aria-expanded="noteOpen" @click="noteOpen = !noteOpen">
        <Icon :name="noteOpen ? 'chevron-down' : 'chevron-up'" />
        <span>{{ tr('笺', 'Note') }}</span>
        <small>{{ tr('侧问 · 不打断主线', 'Aside · keeps the thread') }}</small>
      </button>
      <div v-show="noteOpen" class="note-body">
        <slot name="note" />
      </div>
    </section>

    <p class="rail-foot" :title="tr('模型调用与 token 计量', 'Model calls and token meters')">
      <span>{{ stats.calls }} {{ tr('次调用', 'calls') }}</span>
      <span class="sep">·</span>
      <span>{{ steps }} {{ tr('步', 'steps') }}</span>
      <template v-if="speed !== null">
        <span class="sep">·</span>
        <span>{{ speed.toFixed(1) }} tok/s</span>
      </template>
      <span class="sep">·</span>
      <span>{{ totalTokens }}</span>
      <template v-if="stats.cacheHit !== null">
        <span class="sep">·</span>
        <span :class="{ good: stats.cacheHit >= 50 }">{{ tr('缓存命中', 'cache') }} {{ stats.cacheHit }}%</span>
      </template>
    </p>
  </aside>
</template>

<style scoped>
/* hanako rules apply here: regions are cards on paper, told apart by tone and
   breathing room rather than by ruled lines. */
.workspace-rail {
  width: 292px; flex: none; display: flex; flex-direction: column; gap: 10px;
  min-height: 0; padding: 12px 12px 0; background: var(--bg-sunken);
}
.rail-card {
  border-radius: 14px; background: var(--bg-raised);
  box-shadow: 0 1px 2px rgb(31 35 28 / 4%);
  display: flex; flex-direction: column; min-height: 0;
}
html[data-theme='dark'] .rail-card { box-shadow: 0 1px 2px rgb(0 0 0 / 18%); }
.rail-files { flex: 1; overflow: hidden; padding: 8px; }
.rail-path {
  display: flex; align-items: center; gap: 9px; width: 100%; text-align: left; cursor: pointer;
  padding: 8px 9px; border: 0; border-radius: 10px; background: none; color: inherit;
  transition: background var(--dur-fast) var(--ease-out);
}
@media (hover: hover) { .rail-path:hover { background: var(--bg-hover); } }
.rail-path .icon { width: 15px; height: 15px; flex: none; color: var(--accent); }
.rail-path-text { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
.rail-path-text strong { font-size: 12.5px; font-weight: 600; color: var(--fg); }
.rail-path-text small { font-size: 10.5px; color: var(--fg-faint); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; text-align: left; }
.rail-path-hint { flex: none; font-size: 10px; letter-spacing: .05em; color: var(--fg-faint); }
.rail-files > :deep(.ws-tree) { flex: 1; min-height: 0; overflow-y: auto; padding: 2px 2px 6px; }
.rail-note { flex: none; overflow: hidden; }
.note-head {
  display: flex; align-items: center; gap: 8px; width: 100%; cursor: pointer;
  padding: 10px 12px; border: 0; background: none; color: var(--fg-muted); font: 600 12px/1.4 var(--font);
}
@media (hover: hover) { .note-head:hover { color: var(--fg); } }
.note-head .icon { width: 14px; height: 14px; color: var(--accent); }
.note-head small { margin-left: auto; font-weight: 400; font-size: 10.5px; color: var(--fg-faint); }
.note-body { max-height: 44vh; overflow-y: auto; padding: 0 12px 12px; }
.rail-foot {
  flex: none; display: flex; align-items: center; flex-wrap: wrap; gap: 4px; margin: 0 0 10px;
  padding: 8px 12px; border-radius: 12px; background: var(--bg-raised);
  font: 400 10.5px/1.5 var(--mono); color: var(--fg-faint);
}
.rail-foot .sep { opacity: .45; }
.rail-foot .good { color: var(--ok); }
</style>
