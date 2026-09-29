<script setup lang="ts">
import { computed, ref } from 'vue';
import { get } from '../../core/api/client.ts';
import { tr } from '../../core/i18n/tr.ts';
import Icon from '../../ui/components/Icon.vue';

// One level of the working directory, fetched on demand: the rail lists what the
// agent's shell can reach, and only what the reader opens is ever requested.
interface Entry { name: string; path: string; kind: 'directory' | 'file'; size?: number }
const props = defineProps<{ sessionId: string }>();

const children = ref<Record<string, Entry[]>>({});
const open = ref<Set<string>>(new Set());
const loading = ref<Set<string>>(new Set());
const err = ref('');
const truncated = ref<Set<string>>(new Set());

async function load(path: string) {
  if (children.value[path] || loading.value.has(path)) return;
  loading.value.add(path);
  try {
    const page = await get('/sessions/' + encodeURIComponent(props.sessionId) + '/workspace', { query: { path } });
    children.value = { ...children.value, [path]: page.items ?? [] };
    if (page.truncated) truncated.value = new Set([...truncated.value, path]);
  } catch (e: any) {
    err.value = String(e?.message ?? e);
  } finally {
    const next = new Set(loading.value);
    next.delete(path);
    loading.value = next;
  }
}
void load('');

function toggle(entry: Entry) {
  const next = new Set(open.value);
  if (next.has(entry.path)) next.delete(entry.path);
  else { next.add(entry.path); void load(entry.path); }
  open.value = next;
}
function isOpen(path: string) { return open.value.has(path); }

function sizeLabel(bytes?: number) {
  if (!bytes) return '';
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return Math.round(bytes / 1024) + ' KB';
  return (bytes / 1024 / 1024).toFixed(1) + ' MB';
}
const rootEntries = computed(() => children.value[''] ?? []);
</script>

<template>
  <div class="ws-tree">
    <p v-if="err" class="ws-error">{{ err }}</p>
    <ul v-else class="ws-list">
      <li v-for="entry in rootEntries" :key="entry.path">
        <button type="button" class="ws-row" :class="{ directory: entry.kind === 'directory' }" @click="entry.kind === 'directory' ? toggle(entry) : undefined">
          <Icon v-if="entry.kind === 'directory'" :name="isOpen(entry.path) ? 'chevron-down' : 'chevron-right'" />
          <Icon v-else name="file-diff" />
          <span class="ws-name">{{ entry.name }}</span>
          <span v-if="entry.kind === 'file'" class="ws-size">{{ sizeLabel(entry.size) }}</span>
        </button>
        <ul v-if="entry.kind === 'directory' && isOpen(entry.path)" class="ws-children">
          <li v-if="loading.has(entry.path)" class="ws-loading">{{ tr('读取中…', 'Loading…') }}</li>
          <li v-for="child in children[entry.path] ?? []" :key="child.path">
            <button type="button" class="ws-row" :class="{ directory: child.kind === 'directory' }" @click="child.kind === 'directory' ? toggle(child) : undefined">
              <Icon v-if="child.kind === 'directory'" :name="isOpen(child.path) ? 'chevron-down' : 'chevron-right'" />
              <Icon v-else name="file-diff" />
              <span class="ws-name">{{ child.name }}</span>
              <span v-if="child.kind === 'file'" class="ws-size">{{ sizeLabel(child.size) }}</span>
            </button>
            <ul v-if="child.kind === 'directory' && isOpen(child.path)" class="ws-children">
              <li v-for="leaf in children[child.path] ?? []" :key="leaf.path" class="ws-leaf">
                <Icon :name="leaf.kind === 'directory' ? 'folder' : 'file-diff'" />
                <span class="ws-name">{{ leaf.name }}</span>
              </li>
            </ul>
          </li>
        </ul>
      </li>
    </ul>
    <p v-if="!err && !rootEntries.length" class="ws-empty">{{ tr('工作目录是空的', 'The working directory is empty') }}</p>
  </div>
</template>

<style scoped>
.ws-tree { min-height: 0; }
.ws-list, .ws-children { list-style: none; margin: 0; padding: 0; }
.ws-children { padding-left: 14px; }
.ws-row {
  display: flex; align-items: center; gap: 6px; width: 100%; text-align: left; cursor: pointer;
  padding: 4px 6px; border: 0; border-radius: 6px; background: none; color: var(--fg-muted); font-size: 12px;
}
.ws-leaf { display: flex; align-items: center; gap: 6px; padding: 4px 6px 4px 20px; color: var(--fg-faint); font-size: 12px; }
@media (hover: hover) { .ws-row:hover { background: var(--bg-hover); color: var(--fg); } }
.ws-row .icon, .ws-leaf .icon { width: 13px; height: 13px; flex: none; color: var(--fg-faint); }
.ws-row.directory .icon { color: var(--accent); }
.ws-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ws-size { margin-left: auto; font-size: 10.5px; color: var(--fg-faint); font-family: var(--mono); }
.ws-loading, .ws-empty, .ws-error { margin: 4px 0; font-size: 11.5px; color: var(--fg-faint); }
.ws-error { color: var(--err); }
</style>
