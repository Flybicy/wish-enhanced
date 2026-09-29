<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { get } from '../../core/api/client.ts';
import { chat } from '../../core/state/chatSlice.ts';
import { tr } from '../../core/i18n/tr.ts';
import Icon from '../../ui/components/Icon.vue';

// The right rail answers the question the session list cannot: where does this
// conversation actually live, and how heavy has it become? Workspace first,
// because wish organizes by working directory rather than by project object.
const props = defineProps<{ sessionId: string }>();

const snapshot = computed(() => chat.snapshot.value);
const usage = ref<any>(null);
const config = ref<any>(null);

const cwd = computed(() => (snapshot.value?.descriptor as any)?.cwd ?? '');
const folderName = computed(() => {
  const path = cwd.value.replace(/[\\/]+$/, '');
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
});

interface Row { key: string; label: string; value: string; mono?: boolean; on?: boolean | null }
const rows = computed<Row[]>(() => {
  const items: Row[] = [];
  const model = snapshot.value?.model ?? '';
  if (model) items.push({ key: 'model', label: tr('模型', 'Model'), value: model, mono: true });
  const totals = usage.value?.statistics?.totals?.tokens;
  if (totals) {
    items.push({ key: 'tokens-in', label: tr('输入 Token', 'Input tokens'), value: String(totals.input_tokens ?? 0), mono: true });
    items.push({ key: 'tokens-out', label: tr('输出 Token', 'Output tokens'), value: String(totals.output_tokens ?? 0), mono: true });
    if (totals.reasoning_tokens) items.push({ key: 'tokens-reason', label: tr('其中推理', 'Reasoning'), value: String(totals.reasoning_tokens), mono: true });
  }
  const stats = usage.value?.statistics;
  if (stats) items.push({ key: 'calls', label: tr('模型调用', 'Model calls'), value: String(stats.model_attempts ?? 0), mono: true });
  return items;
});

// Capability switches come from the running configuration, so the rail doubles
// as a truthful status board next to the (session-scoped) capabilities pane.
const switches = computed(() => {
  const c = config.value?.config;
  if (!c) return [] as { key: string; label: string; on: boolean }[];
  return [
    { key: 'web', label: tr('Web 检索', 'Web access'), on: !!c.web?.enabled },
    { key: 'skills', label: tr('技能库', 'Skills'), on: !!c.skills?.enabled },
    { key: 'memory', label: tr('观察式记忆', 'Memory'), on: !!c.memory?.enabled },
    { key: 'subagents', label: tr('子代理', 'Subagents'), on: !!c.subagents?.enabled },
  ];
});

async function load(id: string) {
  if (!id) return;
  try {
    const [usageData, configData] = await Promise.all([
      get('/sessions/' + encodeURIComponent(id) + '/usage'),
      get('/config'),
    ]);
    usage.value = usageData;
    config.value = configData;
  } catch {
    // A rail is a convenience; a missing endpoint must never break the pane.
  }
}

watch(() => props.sessionId, load, { immediate: true });

const router = useRouter();
const copyState = ref('');
async function copyPath() {
  try {
    await navigator.clipboard.writeText(cwd.value);
    copyState.value = tr('已复制', 'Copied');
  } catch {
    copyState.value = tr('复制失败', 'Copy failed');
  }
  setTimeout(() => { copyState.value = ''; }, 1600);
}
const openCapabilities = () => router.push({ name: 'chat-capabilities', params: { id: props.sessionId } });
const openInfo = () => router.push({ name: 'chat-info', params: { id: props.sessionId } });
</script>

<template>
  <aside class="workspace-rail" :aria-label="tr('工作区', 'Workspace')">
    <header class="rail-head">
      <h2>{{ tr('工作区', 'Workspace') }}</h2>
    </header>

    <section class="rail-section">
      <button class="rail-path" type="button" :title="cwd" @click="copyPath">
        <Icon name="folder" />
        <span class="rail-path-text">
          <strong>{{ folderName || tr('未设置目录', 'No directory') }}</strong>
          <small>{{ cwd || tr('这个会话还没有工作目录', 'This session has no working directory yet') }}</small>
        </span>
        <span class="rail-path-hint">{{ copyState || tr('复制', 'Copy') }}</span>
      </button>
    </section>

    <section v-if="rows.length" class="rail-section">
      <h3>{{ tr('本会话计量', 'This session') }}</h3>
      <dl class="rail-stats">
        <div v-for="row in rows" :key="row.key" class="rail-stat">
          <dt>{{ row.label }}</dt>
          <dd :class="{ mono: row.mono }">{{ row.value }}</dd>
        </div>
      </dl>
    </section>

    <section v-if="switches.length" class="rail-section">
      <h3>{{ tr('能力', 'Capabilities') }}</h3>
      <ul class="rail-list">
        <li v-for="item in switches" :key="item.key" :class="{ 'is-on': item.on }">
          <span class="dot" aria-hidden="true" />
          <span>{{ item.label }}</span>
          <span class="rail-state">{{ item.on ? tr('已启用', 'on') : tr('已关闭', 'off') }}</span>
        </li>
      </ul>
      <button class="rail-link" type="button" @click="openCapabilities">
        <span>{{ tr('打开能力面板', 'Open capabilities') }}</span>
        <Icon name="chevron-right" />
      </button>
    </section>

    <section class="rail-section">
      <h3>{{ tr('会话', 'Session') }}</h3>
      <ul class="rail-list">
        <li><span class="dot" aria-hidden="true" /><span>{{ tr('Shell', 'Shell') }}</span><span class="rail-state">{{ snapshot?.descriptor?.shell ? tr('自有', 'own') : tr('跟随全局', 'global') }}</span></li>
      </ul>
      <button class="rail-link" type="button" @click="openInfo">
        <span>{{ tr('会话详情', 'Session details') }}</span>
        <Icon name="chevron-right" />
      </button>
    </section>
  </aside>
</template>

<style scoped>
.workspace-rail {
  width: 268px; flex: none; display: flex; flex-direction: column; gap: 18px;
  padding: 16px 14px 20px; overflow-y: auto; min-height: 0;
  border-left: 1px solid var(--line); background: var(--bg-sunken);
}
.rail-head h2 { margin: 0; font: 600 13px/1.4 var(--font); letter-spacing: .08em; text-transform: uppercase; color: var(--fg-subtle); }
.rail-section { display: flex; flex-direction: column; gap: 8px; }
.rail-section h3 { margin: 0; font: 600 11px/1.4 var(--font); letter-spacing: .1em; text-transform: uppercase; color: var(--fg-faint); }
.rail-path {
  display: flex; align-items: center; gap: 10px; width: 100%; text-align: left; cursor: pointer;
  padding: 10px 12px; border: 1px solid var(--line); border-radius: 12px; background: var(--bg-raised);
  color: inherit; transition: border-color var(--dur-fast) var(--ease-out), background var(--dur-fast) var(--ease-out);
}
@media (hover: hover) { .rail-path:hover { border-color: var(--line-strong); background: var(--bg-hover); } }
.rail-path .icon { width: 16px; height: 16px; flex: none; color: var(--accent); }
.rail-path-text { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
.rail-path-text strong { font-size: 13px; font-weight: 600; color: var(--fg); }
.rail-path-text small { font-size: 11px; color: var(--fg-faint); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; text-align: left; }
.rail-path-hint { flex: none; font-size: 10px; letter-spacing: .06em; color: var(--fg-faint); text-transform: uppercase; }
.rail-stats { margin: 0; display: flex; flex-direction: column; gap: 6px; }
.rail-stat { display: flex; align-items: baseline; justify-content: space-between; gap: 10px; }
.rail-stat dt { font-size: 12px; color: var(--fg-muted); }
.rail-stat dd { margin: 0; font-size: 12px; color: var(--fg); }
.rail-stat dd.mono { font-family: var(--mono); font-size: 11.5px; }
.rail-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
.rail-list li { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--fg-muted); }
.rail-list li .dot { width: 6px; height: 6px; border-radius: 50%; flex: none; background: var(--fg-faint); }
.rail-list li.is-on .dot { background: var(--ok); box-shadow: var(--glow-on); }
.rail-state { margin-left: auto; font-size: 11px; color: var(--fg-faint); }
.rail-link {
  display: flex; align-items: center; justify-content: space-between; gap: 8px; width: 100%; cursor: pointer;
  padding: 8px 10px; border: 1px solid var(--line); border-radius: 10px; background: none; color: var(--fg-muted);
  font-size: 12px; transition: border-color var(--dur-fast) var(--ease-out), color var(--dur-fast) var(--ease-out);
}
@media (hover: hover) { .rail-link:hover { border-color: var(--accent); color: var(--accent); } }
.rail-link .icon { width: 14px; height: 14px; }
</style>
