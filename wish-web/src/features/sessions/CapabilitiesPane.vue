<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { get } from '../../core/api/client.ts';
import { tr } from '../../core/i18n/tr.ts';
import Icon from '../../ui/components/Icon.vue';

const props = defineProps<{ sessionId?: string }>();
defineEmits<{ close: [] }>();

// Capability surface of the running wish server: what this session can actually
// do, plus the MCP servers, memory, skills and subagent settings behind it.
//
// The injected tool list lives on the SESSION, not on the global configuration:
// reading config.config.tools reported every capability as disabled even when
// the session had web, memory and subagent tools in hand.
const config = ref<any>(null);
const session = ref<any>(null);
const err = ref<string | null>(null);

onMounted(async () => {
  try {
    const [globalConfig, snapshot] = await Promise.all([
      get('/config'),
      props.sessionId ? get('/sessions/' + encodeURIComponent(props.sessionId)) : Promise.resolve(null),
    ]);
    config.value = globalConfig;
    session.value = snapshot;
  } catch (e: any) {
    err.value = String(e?.message ?? e);
  }
});

const toolNames = computed<string[]>(() => {
  try {
    const tools = session.value?.status?.config?.tools ?? [];
    return tools.map((tool: any) => tool?.name ?? '').filter(Boolean);
  } catch { return []; }
});

const mcpServers = computed(() => Object.entries(config.value?.config?.mcp_servers ?? {}).map(([id, spec]: [string, any]) => ({
  id,
  command: spec?.command ?? '',
  toolCount: toolNames.value.filter(name => name.startsWith(`mcp_${id}_`)).length,
})));

const hasWeb = computed(() => toolNames.value.some(name => ['web_search', 'fetch_content'].includes(name)));
const hasSnapshot = computed(() => toolNames.value.some(name => name.startsWith('snapshot_')));
const hasSkills = computed(() => toolNames.value.includes('skill_search'));
const hasMemory = computed(() => toolNames.value.includes('memory_recall'));
const hasSubagents = computed(() => toolNames.value.includes('subagent'));

const memory = computed(() => config.value?.config?.memory ?? null);
const skills = computed(() => config.value?.config?.skills ?? null);
const subagents = computed(() => config.value?.config?.subagents ?? null);

const rows = computed(() => [
  { key: 'web', label: tr('Web 检索', 'Web Access'), on: hasWeb.value, detail: hasWeb.value ? tr('搜索 + 抓取 + 缓存', 'search + fetch + cache') : tr('配置中未启用', 'disabled in config') },
  { key: 'snapshots', label: tr('工作区快照', 'Workspace Snapshots'), on: hasSnapshot.value, detail: hasSnapshot.value ? tr('checkpoint / 撤销 / 重做', 'checkpoint / undo / redo') : tr('未检测到项目', 'no project detected') },
  { key: 'skills', label: tr('技能库', 'Skills'), on: hasSkills.value, detail: hasSkills.value ? tr(`已启用${skills.value?.enabled ? '' : '（配置关闭）'}`, `enabled${skills.value?.enabled ? '' : ' (off)'}`) : tr('没有技能目录', 'no skill dirs') },
  { key: 'memory', label: tr('观察式记忆', 'Observational Memory'), on: hasMemory.value, detail: hasMemory.value ? tr(`观察 ${memory.value?.observe_after_tokens ?? '-'} / 反思 ${memory.value?.reflect_after_tokens ?? '-'} tokens`, `observe ${memory.value?.observe_after_tokens ?? '-'} / reflect ${memory.value?.reflect_after_tokens ?? '-'} tokens`) : tr('配置中未启用', 'disabled in config') },
  { key: 'subagents', label: tr('子代理', 'Subagents'), on: hasSubagents.value, detail: hasSubagents.value ? tr(`并发池 ${subagents.value?.max_concurrent ?? '-'} / 深度 ${subagents.value?.max_depth ?? '-'}`, `pool ${subagents.value?.max_concurrent ?? '-'} / depth ${subagents.value?.max_depth ?? '-'}`) : tr('配置中未启用', 'disabled in config') },
]);
</script>

<template>
  <section class="capabilities-pane" aria-label="Capabilities">
    <header class="pane-head">
      <h2>{{ tr('能力面板', 'Capabilities') }}</h2>
    </header>
    <p v-if="err" class="pane-error">{{ tr('无法读取配置：', 'Could not load configuration: ') }}{{ err }}</p>
    <ul v-else class="capability-list">
      <li v-for="row in rows" :key="row.key" class="capability-row" :class="{ 'is-on': row.on }">
        <span class="dot" aria-hidden="true" />
        <span class="label">{{ row.label }}</span>
        <span class="detail">{{ row.detail }}</span>
      </li>
    </ul>
    <div v-if="mcpServers.length" class="mcp-section">
      <h3>{{ tr('MCP 服务器', 'MCP Servers') }}</h3>
      <ul class="mcp-list">
        <li v-for="server in mcpServers" :key="server.id" class="mcp-row">
          <Icon name="layers" />
          <span class="label">{{ server.id }}</span>
          <span class="detail">{{ server.toolCount }} tools · {{ server.command }}</span>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.capabilities-pane { display: flex; flex-direction: column; gap: 16px; padding: 16px; overflow-y: auto; height: 100%; }
.pane-head h2 { font-size: 1rem; margin: 0; }
.pane-error { color: var(--err); font-size: 0.85rem; }
.capability-list, .mcp-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
.capability-row, .mcp-row { display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 10px; background: var(--bg-inset); font-size: 0.85rem; }
.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--fg-faint); flex: none; transition: background var(--dur-fast) var(--ease-out), box-shadow var(--dur-fast) var(--ease-out); }
.is-on .dot { background: var(--accent); box-shadow: var(--glow-on); }
.capability-row { border-left: 2px solid transparent; transition: border-color var(--dur-fast) var(--ease-out); }
.is-on.capability-row { border-left-color: var(--accent); }
.pane-head h2 { font-size: 1rem; margin: 0; padding-bottom: 8px; background: var(--grad-soft); -webkit-background-clip: text; background-clip: text; color: transparent; width: fit-content; }
.label { font-weight: 600; }
.detail { margin-left: auto; color: var(--fg-subtle); font-size: 0.78rem; text-align: right; }
.mcp-section h3 { font-size: 0.85rem; margin: 0 0 4px; }
</style>
