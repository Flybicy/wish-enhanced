<script setup lang="ts">
// The dialog is owned by the list, outside recycled virtual rows. Its target
// never follows the currently open chat; writes belong to the captured ID.
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import * as api from '../../core/api/endpoints.ts';
import { sessions, metaOf } from '../../core/state/sessionsSlice.ts';
import { chat } from '../../core/state/chatSlice.ts';
import { i18n } from '../../core/i18n/index.ts';
import { tr } from '../../core/i18n/tr.ts';
import Modal from '../../ui/components/Modal.vue';
import Icon from '../../ui/components/Icon.vue';
const props = defineProps<{ target: { id: string; name?: string; cwd?: string; rows?: any[] }; kind: 'rename' | 'tags' | 'delete' | 'rename-project' | 'delete-project' }>();
const emit = defineEmits<{ close: [] }>();
const router = useRouter();
const snapshot = ref<any>();
const name = ref(props.target.name || '');
const tags = ref<string[]>([]);
const tagInput = ref('');
const busy = ref(false);
// Project dialogs have no i18n keys of their own; session ones stay keyed.
const title = computed(() => props.kind === 'rename-project' ? tr('重命名项目', 'Rename project')
  : props.kind === 'delete-project' ? tr('删除项目', 'Delete project')
  : i18n.t(`manage.${props.kind}`));
const loading = ref(props.kind === 'tags');
const error = ref('');
const draftTags = computed(() => [...new Set([...tags.value, ...(tagInput.value.trim() ? [tagInput.value.trim()] : [])])]);
const changed = computed(() => props.kind === 'rename' || props.kind === 'rename-project' ? name.value.trim() !== (props.target.name || '')
  : props.kind === 'tags' ? JSON.stringify(draftTags.value) !== JSON.stringify(snapshot.value?.metadata?.tags ?? []) : true);
const canSave = computed(() => !busy.value && !loading.value && changed.value && (props.kind !== 'rename' || !!name.value.trim())
  && (props.kind !== 'tags' || (!!snapshot.value && draftTags.value.length <= 16 && draftTags.value.every(tag => [...tag].length <= 64))));
function addTag() {
  if (draftTags.value.length > 16 || draftTags.value.some(tag => [...tag].length > 64)) return;
  tags.value = draftTags.value;
  tagInput.value = '';
}
async function loadTags() {
  loading.value = true;
  error.value = '';
  try {
    snapshot.value = await api.sessionGet(props.target.id);
    tags.value = Array.isArray(snapshot.value.metadata?.tags) ? [...snapshot.value.metadata.tags] : [];
  } catch (e: any) { error.value = String(e?.detail || e?.message || e); }
  finally { loading.value = false; }
}
onMounted(() => { if (props.kind === 'tags') loadTags(); });
async function save() {
  if (!canSave.value) return;
  const id = props.target.id;
  busy.value = true;
  error.value = '';
  try {
    if (props.kind === 'delete' || props.kind === 'delete-project') {
      const ids = props.kind === 'delete' ? [id] : (props.target.rows ?? []).map((row: any) => row.id);
      for (const victim of ids) {
        await api.sessionDelete(victim);
        sessions.dropRow(victim);
        if (chat.sessionId.value === victim) { chat.close(); await router.push('/sessions'); }
      }
    } else if (props.kind === 'rename-project') {
      // A project rename is a pure metadata write; the folder on disk is
      // never touched. An empty label clears the override, falling back
      // to the directory's own basename.
      const label = name.value.trim();
      for (const row of props.target.rows ?? []) {
        await api.sessionUpdateMeta(row.id, { ...metaOf(row), project: label || undefined }, row.revision);
      }
      await sessions.rebuild();
    } else {
      const snap = props.kind === 'rename' ? await sessions.rename(id, name.value.trim())
        : await sessions.updateMeta(snapshot.value, { tags: draftTags.value });
      if (chat.sessionId.value === id && (chat.snapshot.value?.revision ?? 0) <= snap.revision) chat.snapshot.value = snap;
    }
    emit('close');
  } catch (e: any) { error.value = String(e?.detail || e?.message || e); }
  finally { busy.value = false; }
}
</script>

<template>
  <Modal :open="true" compact :title="title" :dismissable="!busy" @close="emit('close')">
    <form id="session-list-action" @submit.prevent="save">
      <p class="sl-action-name">{{ target.name || target.id.slice(0, 8) }}</p>
      <p v-if="kind === 'delete-project'">{{ tr('将删除此项目下的 ' + (target.rows?.length ?? 0) + ' 个会话记录，磁盘上的工作区文件不受影响。', 'Deletes the ' + (target.rows?.length ?? 0) + ' session records in this project. Files on disk are untouched.') }}</p>
      <input v-else-if="kind === 'rename'" v-model="name" class="input" :aria-label="i18n.t('manage.rename')" :disabled="busy" />
      <template v-else-if="kind === 'rename-project'">
        <input v-model="name" class="input" :aria-label="title" :disabled="busy" />
        <span class="hint">{{ tr('留空则恢复为文件夹名。', 'Leave empty to fall back to the folder name.') }}</span>
      </template>
      <p v-else-if="kind === 'delete'">{{ i18n.t('manage.deleteConfirm') }}</p>
      <template v-else>
        <p v-if="loading" role="status">{{ i18n.t('sessions.loading') }}</p>
        <div v-else-if="snapshot" class="sl-tag-editor">
          <div v-if="tags.length" class="sl-tag-chips"><span v-for="tag in tags" :key="tag" class="tag-chip">{{ tag }}<button type="button" :disabled="busy" :aria-label="`${i18n.t('common.remove')} ${tag}`" @click="tags = tags.filter(t => t !== tag)"><Icon name="x" /></button></span></div>
          <input v-model="tagInput" class="input" :disabled="busy || tags.length >= 16" :placeholder="i18n.t('manage.tagPlaceholder')" :aria-label="i18n.t('manage.tags')" @keydown.enter="event => { if (!event.isComposing) { event.preventDefault(); addTag(); } }" />
          <span class="hint">{{ i18n.t('manage.tagsHint') }} · {{ draftTags.length }} / 16</span>
        </div>
      </template>
      <div v-if="error" class="load-error" role="alert">{{ error }}</div>
      <button v-if="kind === 'tags' && error" type="button" class="btn ghost sm" :disabled="busy || loading" @click="loadTags">{{ i18n.t('info.refresh') }}</button>
    </form>
    <template #footer>
      <button class="btn ghost" :disabled="busy" @click="emit('close')">{{ i18n.t('manage.cancel') }}</button>
      <button form="session-list-action" type="submit" class="btn" :class="kind === 'delete' || kind === 'delete-project' ? 'danger' : 'primary'" :disabled="!canSave">{{ busy ? i18n.t('sessions.loading') : i18n.t(kind === 'delete' || kind === 'delete-project' ? 'manage.delete' : 'common.save') }}</button>
    </template>
  </Modal>
</template>

<style scoped>
.sl-action-name { margin: 0 0 4px; color: var(--fg-subtle); overflow-wrap: anywhere; }
form > p { margin: 0 0 4px; }
form > .hint { display: block; margin: 4px 0 0; font-size: 12px; line-height: 1.5; color: var(--fg-subtle); }
form > :last-child { margin-bottom: 0; }
.sl-tag-editor { display: flex; flex-direction: column; gap: 10px; }
.sl-tag-chips { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 4px; }
.sl-tag-editor .hint { font-size: 12px; line-height: 1.5; }
.sl-tag-editor .tag-chip { max-width: 100%; overflow-wrap: anywhere; }
.sl-tag-editor .input { width: 100%; }
</style>
