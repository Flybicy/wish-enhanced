<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import WorkspaceTree from './WorkspaceTree.vue';
import { chat } from '../../core/state/chatSlice.ts';
import { prefs } from '../../core/state/prefsSlice.ts';
import { tr } from '../../core/i18n/tr.ts';
import Icon from '../../ui/components/Icon.vue';

// The desk beside the conversation: the shelf of files, and the loose note taped
// along the foot. Only what can be acted on lives here — the note takes writing
// and the tree takes opening. Capability flags are deliberately absent (they are
// read-only, so the model is told them and the reader is not shown them), and so
// are the meters: a fixed-width column with a variable-width sentence inside it
// only ever wrapped badly.
const props = defineProps<{ sessionId: string }>();

const snapshot = computed(() => chat.snapshot.value);

const cwd = computed(() => (snapshot.value?.descriptor as any)?.cwd ?? '');
const folderName = computed(() => {
  const path = cwd.value.replace(/[\\/]+$/, '');
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
});

const copying = ref('');
async function copyPath() {
  try { await navigator.clipboard.writeText(cwd.value); copying.value = tr('已复制', 'Copied'); }
  catch { copying.value = tr('失败', 'Failed'); }
  setTimeout(() => { copying.value = ''; }, 1400);
}

// BTW — the aside taped along the rail's foot, opened on demand. It asks
// against the live context without disturbing the main thread.
const noteOpen = ref(prefs.btwRail.value);
watch(noteOpen, value => prefs.setBtwRail(value));
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
        <span>{{ tr('BTW', 'BTW') }}</span>
        <small>{{ tr('侧问 · 不打断主线', 'Aside · keeps the thread') }}</small>
      </button>
      <div v-show="noteOpen" class="note-body">
        <slot name="note" />
      </div>
    </section>
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
.rail-note { flex: none; overflow: hidden; margin-bottom: 12px; }
.note-head {
  display: flex; align-items: center; gap: 8px; width: 100%; cursor: pointer;
  padding: 10px 12px; border: 0; background: none; color: var(--fg-muted); font: 600 12px/1.4 var(--font);
}
@media (hover: hover) { .note-head:hover { color: var(--fg); } }
.note-head .icon { width: 14px; height: 14px; color: var(--accent); }
.note-head small { margin-left: auto; font-weight: 400; font-size: 10.5px; color: var(--fg-faint); }
.note-body { min-height: 260px; max-height: 46vh; overflow-y: auto; padding: 0 12px 12px; }
/* The head already names the aside, so the inner label would only repeat it;
   its clear button stays. The log becomes an inset sheet on the card. */
.note-body :deep(.ask-heading > div) { display: none; }
.note-body :deep(.ask-log) { padding: 8px 12px 4px; }
.note-body :deep(.ask-composer) { border-top: 0; background: transparent; padding: 8px 4px 4px; }
/* The aside's input reads as a field on the card: what is typed sits inside
   the box, and focus picks up the seal accent. */
.note-body :deep(.ask-composer textarea) {
  background: var(--bg-inset); border: 1px solid var(--line); border-radius: 10px;
  padding: 9px 12px; min-height: 64px; line-height: 1.6;
}
.note-body :deep(.ask-composer:focus-within) { border-top-color: transparent; }
.note-body :deep(.ask-composer:focus-within textarea) { border-color: var(--accent); }
</style>
