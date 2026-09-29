<script setup lang="ts">
import DefaultModelPicker from './DefaultModelPicker.vue';
import { ref, computed, inject, onMounted, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle, SwitchRoot, SwitchThumb } from 'reka-ui';
import Icon from '../../ui/components/Icon.vue';
import { useMedia } from '../../ui/composables/useMedia.ts';
import { usePageActivity } from '../../ui/composables/usePageActivity.ts';
import { useDialogFocus } from '../../ui/composables/useDialogFocus.ts';
import { tr, compactionFields } from './fields.ts';
import UiSettings from './UiSettings.vue';
import MobileSessionSettings from './MobileSessionSettings.vue';
import Modal from '../../ui/components/Modal.vue';
import AddProvider from './AddProvider.vue';
import PresetProvider from './PresetProvider.vue';
import ServerProxySettings from './ServerProxySettings.vue';
import ServerShellSettings from './ServerShellSettings.vue';
import './settings.css';
import { get } from '../../core/api/client.ts';
import { useConfigDraft } from './useConfigDraft.ts';
import Wordmark from '../../ui/components/Wordmark.vue';
import { cfg } from '../../core/config.ts';
import { i18n } from '../../core/i18n/index.ts';
import { useSettingsGuard } from './useSettingsGuard.ts';

const isMobile = useMedia('(max-width: 899px)');
const pageActive = usePageActivity();
const focus = useDialogFocus();

function handleOutside(event: CustomEvent) {
  if ((event.detail.originalEvent.target as Element)?.closest?.('.pwa-update')) event.preventDefault();
}

const closeSettings = inject<() => unknown>('closeSettings')!;

const sections = computed(() => [
  { id: 'service', icon: 'settings', label: tr('服务与会话', 'Service & sessions'),
    summary: tr('默认模型、提示词与上下文', 'Model, instructions and context'),
    description: tr('新会话的默认模型、提示词、上下文压缩和命令执行环境。', 'Defaults for new sessions: model, instructions, context compaction and shell.') },
  { id: 'providers', icon: 'bot', label: tr('提供商', 'Providers'),
    summary: tr('连接、认证与模型管理', 'Connections, credentials and models'),
    description: tr('管理模型提供商、凭据与网络代理，保存后对新的调用生效。', 'Model providers, credentials and the network proxy. Saved changes apply to new calls.') },
  { id: 'capabilities', icon: 'layers', label: tr('能力', 'Capabilities'),
    summary: tr('检索、技能、记忆与插件', 'Search, skills, memory and plugins'),
    description: tr('引擎内置能力的开关与参数，以及 MCP 插件通道。保存后对新的调用生效。', 'Built-in engine capabilities and the MCP plugin channel. Saved changes apply to new calls.') },
  { id: 'ui', icon: 'settings-2', label: tr('界面', 'Interface'),
    summary: tr('外观、通知与本地偏好', 'Appearance, notifications and preferences'),
    description: tr('外观、输入与本地数据，只保存在这个浏览器中。', 'Appearance, input and local data, stored in this browser only.') },
]);
const current = computed(() => sections.value.find(item => item.id === tab.value));
// 235929 reads as "23.6万" / "236K" beside the exact input.
const compactTokens = (value: unknown) => typeof value === 'number' && value > 0
  ? new Intl.NumberFormat(i18n.locale.value === 'zh' ? 'zh-CN' : 'en', { notation: 'compact', maximumFractionDigits: 1 }).format(value)
  : '';

const route = useRoute();
const router = useRouter();
const desktopSection = ref('service');
const mobileSection = computed(() =>
  sections.value.some((item) => item.id === route.query.section) ? String(route.query.section) : ''
);
const tab = computed(() => (isMobile.value ? mobileSection.value : desktopSection.value));

function selectSection(id: string) {
  if (isMobile.value) router.push({ path: '/settings', query: { section: id } });
  else desktopSection.value = id;
}

// Desktop tabs share one scroller; each tab starts at its top.
const content = ref<HTMLElement>();
watch(tab, () => { if (!isMobile.value && content.value) content.value.scrollTop = 0; });

function backToCategories() {
  router.replace({ path: '/settings' });
}

const {
  draft,
  revision,
  source,
  busy,
  catalog,
  proxyEnvironment,
  shells,
  adding,
  newProviderId,
  advanced,
  advancedPending,
  serverDirty,
  providerOptions,
  effortOptions,
  protocolOptions,
  findPreset,
  accept,
  load,
  applyAdvanced,
  saveConfig,
  addProvider,
  changeProtocol,
  removeProvider,
} = useConfigDraft();

// Capabilities: the engine's own abilities and the MCP plugin channel. They
// live in the same config document as everything else, so they bind to the
// same draft and save through the same footer — no separate save path.
const skillCatalog = ref<any>(null);
async function loadSkillCatalog() {
  try { skillCatalog.value = await get('/skills'); } catch { skillCatalog.value = null; }
}
watch(tab, value => { if (value === 'capabilities' && !skillCatalog.value) void loadSkillCatalog(); }, { immediate: true });
const builtinSkillsDir = computed(() => skillCatalog.value?.directories?.find((entry: any) => entry.builtin)?.path ?? '');
const builtinSkillCount = computed(() => skillCatalog.value
  ? (skillCatalog.value.total ?? 0) + ' ' + tr('个技能', 'skills')
  : tr('未扫描', 'not scanned'));
function skillCount(dir: string) {
  if (!skillCatalog.value || !dir) return tr('未扫描', 'not scanned');
  const hit = skillCatalog.value.directories?.find((entry: any) => entry.path === dir);
  return hit ? hit.count + ' ' + tr('个技能', 'skills') : tr('没有技能', 'no skills');
}
// MCP environment and headers read as KEY=value lines, one pair per row.
const kvText = (map: Record<string, string> | undefined) =>
  Object.entries(map ?? {}).map(([key, value]) => key + '=' + value).join('\n');
function parseKv(text: string, target: Record<string, string>) {
  for (const key of Object.keys(target)) delete target[key];
  for (const row of text.split('\n').map(row => row.trim()).filter(Boolean)) {
    const at = row.indexOf('=');
    if (at > 0) target[row.slice(0, at)] = row.slice(at + 1);
  }
}
const eventValue = (event: Event) => (event.target as HTMLInputElement | HTMLTextAreaElement).value;
const newMcpId = ref('');
function addMcp() {
  const id = newMcpId.value.trim();
  if (!id || !draft.value || draft.value.mcp_servers[id]) return;
  draft.value.mcp_servers[id] = { command: '', args: [], env: {}, headers: {}, call_timeout_secs: 60, endpoint: null };
  newMcpId.value = '';
}
function removeMcp(id: string) {
  if (draft.value) delete draft.value.mcp_servers[id];
}


const {
  dirty,
  save,
  discard,
  discardChanges,
  leave,
  leaveBusy,
  resolveLeave,
  saveAndReturn,
} = useSettingsGuard({
  isMobile,
  serverDirty,
  source,
  revision,
  accept,
  load,
  saveConfig,
});

onMounted(load);
</script>
<template>
  <DialogRoot :open="pageActive" :modal="!isMobile" @update:open="open => { if (!open && !isMobile) closeSettings(); }">
  <DialogPortal :disabled="isMobile">
    <DialogOverlay v-if="!isMobile" class="settings-overlay"/>
    <DialogContent as-child :aria-describedby="undefined" @open-auto-focus="focus.opened" @close-auto-focus="focus.closed" @interact-outside="handleOutside">
  <div class="page native-settings">
    <aside v-show="!isMobile||!mobileSection" class="settings-sidebar">
      <header class="settings-heading settings-home-heading">
        <button v-if="isMobile" class="btn ghost icon-only settings-back" :aria-label="tr('返回','Back')" @click="closeSettings"><Icon name="arrow-left"/></button>
        <DialogTitle class="settings-title">{{tr('设置','Settings')}}</DialogTitle>
      </header>
      <nav :aria-label="tr('设置分类','Settings categories')">
        <button v-for="item in sections" :key="item.id" class="settings-section" :class="{selected:!isMobile&&tab===item.id}" :aria-current="tab===item.id?'page':undefined" @click="selectSection(item.id)">
          <span class="settings-section-icon"><Icon :name="item.icon"/></span>
          <span class="settings-section-text">{{item.label}}<small v-if="isMobile" class="category-description">{{item.summary}}</small></span>
          <Icon v-if="isMobile" name="chevron-right" class="section-chevron"/>
        </button>
      </nav>
      <footer class="settings-brand" aria-hidden="true"><img src="/app-icons/mark.svg" alt=""/><Wordmark class="settings-brand-word"/><span>{{cfg.meta.appVersion}}</span></footer>
    </aside>
    <section v-show="!isMobile||mobileSection" :key="isMobile?mobileSection:'desktop'" class="settings-detail">
    <header class="settings-heading">
      <button v-if="isMobile" class="btn ghost icon-only" :aria-label="tr('返回设置','Back to settings')" @click="backToCategories"><Icon name="arrow-left"/></button>
      <div class="settings-heading-text"><h2>{{current?.label}}</h2><p v-if="!isMobile">{{current?.description}}</p></div>
      <button v-if="!isMobile" class="btn ghost icon-only" :aria-label="tr('关闭设置','Close settings')" @click="closeSettings"><Icon name="x"/></button>
    </header>
    <div ref="content" class="settings-content" data-scroll-preserve>
    <UiSettings v-if="tab==='ui'"/>
    <template v-else-if="draft">
      <MobileSessionSettings v-if="isMobile&&tab==='service'" :config="draft" :shells="shells" :providers="providerOptions" :efforts="effortOptions" :save="save" :busy="busy"/>
      <fieldset :disabled="busy" v-else-if="tab==='service'" class="settings-form">
        <section class="set-section">
          <header class="set-section-head"><h3>{{tr('新会话','New sessions')}}</h3><p>{{tr('只用于之后新建的会话，已有会话保持原来的配置。','Applies to sessions created from now on. Existing sessions keep their configuration.')}}</p></header>
          <div class="set-card">
            <div class="set-row"><span class="set-label"><span>{{tr('默认模型','Default model')}}</span><small>{{tr('新会话使用的模型和思考强度','Model and reasoning effort for new sessions')}}</small></span><DefaultModelPicker class="set-end" :config="draft" :providers="providerOptions" :efforts="effortOptions"/></div>
            <label class="set-row"><span class="set-label"><span>{{tr('工作目录','Working directory')}}</span><small>{{tr('命令执行的起始目录，需要绝对路径','Where commands start. Use an absolute path.')}}</small></span><input class="input set-mono" v-model="draft.defaults.cwd" autocomplete="off" autocapitalize="off" spellcheck="false"/></label>
            <label class="set-row stacked"><span class="set-label"><span>{{tr('固定提示词','Instructions')}}</span><small>{{tr('每个新会话都会带上这段提示词','Included in every new session')}}</small></span><textarea class="input" rows="5" v-model="draft.defaults.instructions" :placeholder="tr('未设置','Not set')"/></label>
          </div>
        </section>
        <section v-if="draft.defaults.compaction" class="set-section">
          <header class="set-section-head"><h3>{{tr('上下文压缩','Context compaction')}}</h3><p>{{tr('对话接近上下文上限时，把较早的内容压缩成摘要。','Summarizes earlier turns as a conversation approaches its context limit.')}}</p></header>
          <div class="set-card">
            <label v-for="field in compactionFields()" :key="field.key" class="set-row"><span class="set-label"><span>{{field.label}}</span><small>{{field.hint}}</small></span><span class="set-number"><em>{{compactTokens(draft.defaults.compaction[field.key])}}</em><input class="input" type="number" min="1" inputmode="numeric" v-model.number="draft.defaults.compaction[field.key]"/></span></label>
          </div>
        </section>
        <section v-if="draft.shell" class="set-section">
          <header class="set-section-head"><h3>Shell</h3><p>{{tr('保存后，跟随全局设置的会话从下一条命令开始使用新的 Shell；单独设置了 Shell 的会话不受影响。','Saved changes apply to the next command of every session that follows this setting; sessions with their own shell keep it.')}}</p></header>
          <div class="set-card"><ServerShellSettings :value="draft.shell" :catalog="shells"/></div>
        </section>
      </fieldset>
      <fieldset :disabled="busy" v-else-if="tab==='capabilities'" class="settings-form">
        <section v-if="draft.web" class="set-section">
          <header class="set-section-head"><h3>{{tr('网页检索','Web search')}}</h3><p>{{tr('允许模型搜索和读取网页资料，保存后对新的调用生效。','Lets the model search and read web sources. Saved changes apply to new calls.')}}</p></header>
          <div class="set-card">
            <div class="set-row"><span class="set-label"><span>{{tr('启用','Enabled')}}</span><small>{{tr('关闭后模型不再发起检索','Off: the model stops searching')}}</small></span>
              <SwitchRoot v-model="draft.web.enabled" class="cfg-switch" :aria-label="tr('启用网页检索','Enable web search')"><SwitchThumb class="cfg-switch-thumb"/></SwitchRoot></div>
            <label class="set-row"><span class="set-label"><span>{{tr('检索服务','Search service')}}</span><small>{{tr('duckduckgo 无需密钥；tavily 与 brave 从环境变量读取密钥','duckduckgo needs no key; tavily and brave read theirs from the environment')}}</small></span>
              <span class="set-number"><select class="input" v-model="draft.web.provider"><option value="duckduckgo">duckduckgo</option><option value="tavily">tavily</option><option value="brave">brave</option></select></span></label>
            <label class="set-row"><span class="set-label"><span>{{tr('读取超时','Fetch timeout')}}</span><small>{{tr('抓取单个网页的最长等待','How long one fetch may wait')}}</small></span>
              <span class="set-number"><em>{{draft.web.fetch_timeout_secs}}s</em><input class="input" type="number" min="1" inputmode="numeric" v-model.number="draft.web.fetch_timeout_secs"/></span></label>
          </div>
        </section>
        <section v-if="draft.skills" class="set-section">
          <header class="set-section-head"><h3>{{tr('技能库','Skills')}}</h3><p>{{tr('技能是放在目录里的说明文件，模型按需检索。内置目录随应用维护，额外目录可以指向任何位置。','Skills are instruction files the model searches on demand. The built-in directory ships with the app; extra directories can point anywhere.')}}</p></header>
          <div class="set-card">
            <div class="set-row"><span class="set-label"><span>{{tr('启用','Enabled')}}</span><small>{{tr('关闭后模型检索不到任何技能','Off: no skill can be found')}}</small></span>
              <SwitchRoot v-model="draft.skills.enabled" class="cfg-switch" :aria-label="tr('启用技能库','Enable skills')"><SwitchThumb class="cfg-switch-thumb"/></SwitchRoot></div>
            <div v-for="(dir, i) in draft.skills.dirs" :key="i" class="set-row">
              <span class="set-label"><span>{{tr('额外目录','Extra directory')}}</span><small>{{skillCount(dir)}}</small></span>
              <span class="set-number skills-dir-controls"><input class="input" v-model="draft.skills.dirs[i]" autocomplete="off" autocapitalize="off" spellcheck="false"/>
                <button type="button" class="btn ghost icon-only" :aria-label="tr('移除目录','Remove directory')" @click="draft.skills.dirs.splice(i, 1)"><Icon name="x"/></button></span>
            </div>
            <div class="set-row"><span class="set-label"><span>{{tr('内置目录','Built-in directory')}}</span><small>{{builtinSkillsDir || '—'}} · {{builtinSkillCount}}</small></span></div>
            <button type="button" class="btn ghost skills-dir-add" @click="draft.skills.dirs.push('')"><Icon name="plus"/>{{tr('添加目录','Add directory')}}</button>
          </div>
        </section>
        <section v-if="draft.memory" class="set-section">
          <header class="set-section-head"><h3>{{tr('观察式记忆','Observant memory')}}</h3><p>{{tr('把长对话的要点沉淀成观察记录，供之后的会话检索。数值为 token 门槛。','Distills long turns into observations later sessions can recall. Values are token thresholds.')}}</p></header>
          <div class="set-card">
            <div class="set-row"><span class="set-label"><span>{{tr('启用','Enabled')}}</span><small>{{tr('关闭后不再记录观察','Off: nothing is observed')}}</small></span>
              <SwitchRoot v-model="draft.memory.enabled" class="cfg-switch" :aria-label="tr('启用记忆','Enable memory')"><SwitchThumb class="cfg-switch-thumb"/></SwitchRoot></div>
            <label class="set-row"><span class="set-label"><span>{{tr('观察门槛','Observe after')}}</span><small>{{tr('单条内容超过此规模才记录','Entries larger than this are observed')}}</small></span>
              <span class="set-number"><em>{{compactTokens(draft.memory.observe_after_tokens)}}</em><input class="input" type="number" min="1" inputmode="numeric" v-model.number="draft.memory.observe_after_tokens"/></span></label>
            <label class="set-row"><span class="set-label"><span>{{tr('沉淀门槛','Reflect after')}}</span><small>{{tr('累计超过此规模才整理','Distilled once this much accumulates')}}</small></span>
              <span class="set-number"><em>{{compactTokens(draft.memory.reflect_after_tokens)}}</em><input class="input" type="number" min="1" inputmode="numeric" v-model.number="draft.memory.reflect_after_tokens"/></span></label>
            <label class="set-row"><span class="set-label"><span>{{tr('池上限','Pool target')}}</span><small>{{tr('整理后活动观察的规模','Active pool size after reflection')}}</small></span>
              <span class="set-number"><em>{{compactTokens(draft.memory.pool_target_tokens)}}</em><input class="input" type="number" min="1" inputmode="numeric" v-model.number="draft.memory.pool_target_tokens"/></span></label>
          </div>
        </section>
        <section v-if="draft.subagents" class="set-section">
          <header class="set-section-head"><h3>{{tr('子代理','Subagents')}}</h3><p>{{tr('把独立的调查任务交给子代理并行执行，主对话不受影响。','Hands self-contained investigations to subagents in parallel, leaving the main thread alone.')}}</p></header>
          <div class="set-card">
            <div class="set-row"><span class="set-label"><span>{{tr('启用','Enabled')}}</span><small>{{tr('关闭后模型不再派出子代理','Off: no subagent is spawned')}}</small></span>
              <SwitchRoot v-model="draft.subagents.enabled" class="cfg-switch" :aria-label="tr('启用子代理','Enable subagents')"><SwitchThumb class="cfg-switch-thumb"/></SwitchRoot></div>
            <label class="set-row"><span class="set-label"><span>{{tr('并行上限','Concurrency')}}</span><small>{{tr('同时运行的子代理数','Subagents running at once')}}</small></span>
              <span class="set-number"><em>{{draft.subagents.max_concurrent}}</em><input class="input" type="number" min="1" inputmode="numeric" v-model.number="draft.subagents.max_concurrent"/></span></label>
            <label class="set-row"><span class="set-label"><span>{{tr('嵌套深度','Depth')}}</span><small>{{tr('子代理可以再派出几层','How deep subagents may nest')}}</small></span>
              <span class="set-number"><em>{{draft.subagents.max_depth}}</em><input class="input" type="number" min="0" inputmode="numeric" v-model.number="draft.subagents.max_depth"/></span></label>
          </div>
        </section>
        <section v-if="draft.mcp_servers" class="set-section">
          <header class="set-section-head"><h3>{{tr('MCP 服务器','MCP servers')}}</h3><p>{{tr('外部工具通过 MCP 接入：填命令跑本地进程，或填 HTTP 端点走远程。','External tools join through MCP: a command runs a local process, an HTTP endpoint connects remotely.')}}</p></header>
          <div v-for="(server, id) in draft.mcp_servers" :key="id" class="set-card mcp-card">
            <div class="set-row mcp-head"><span class="set-label"><span>{{id}}</span></span>
              <button type="button" class="btn ghost icon-only" :aria-label="tr('移除服务器','Remove server')" @click="removeMcp(String(id))"><Icon name="trash-2"/></button></div>
            <label class="set-row"><span class="set-label"><span>{{tr('命令','Command')}}</span><small>{{tr('本地进程；留空则不启动','Local process; empty never starts')}}</small></span>
              <span class="set-number"><input class="input" v-model="server.command" autocomplete="off" autocapitalize="off" spellcheck="false" :placeholder="tr('例如 npx','e.g. npx')"/></span></label>
            <label class="set-row"><span class="set-label"><span>{{tr('参数','Arguments')}}</span><small>{{tr('以空格分隔','Separated by spaces')}}</small></span>
              <span class="set-number"><input class="input" :value="(server.args ?? []).join(' ')" autocomplete="off" autocapitalize="off" spellcheck="false" @change="server.args = eventValue($event).split(/\s+/).filter(Boolean)"/></span></label>
            <label class="set-row"><span class="set-label"><span>{{tr('HTTP 端点','HTTP endpoint')}}</span><small>{{tr('填写后忽略命令，以远程方式连接','When set the command is ignored and the server is remote')}}</small></span>
              <span class="set-number"><input class="input" :value="server.endpoint ?? ''" autocomplete="off" autocapitalize="off" spellcheck="false" placeholder="https://…" @change="server.endpoint = eventValue($event).trim() || null"/></span></label>
            <label class="set-row"><span class="set-label"><span>{{tr('调用超时','Call timeout')}}</span><small>{{tr('单次工具调用的最长等待秒数','Seconds one tool call may wait')}}</small></span>
              <span class="set-number"><em>{{server.call_timeout_secs}}s</em><input class="input" type="number" min="1" inputmode="numeric" v-model.number="server.call_timeout_secs"/></span></label>
            <label class="set-row stacked"><span class="set-label"><span>{{tr('环境变量','Environment')}}</span><small>KEY=value</small></span>
              <textarea class="input" rows="2" spellcheck="false" :value="kvText(server.env)" @change="parseKv(eventValue($event), server.env)"/></label>
            <label class="set-row stacked"><span class="set-label"><span>{{tr('请求头','Headers')}}</span><small>KEY=value</small></span>
              <textarea class="input" rows="2" spellcheck="false" :value="kvText(server.headers)" @change="parseKv(eventValue($event), server.headers)"/></label>
          </div>
          <div class="set-row mcp-add">
            <input class="input" v-model="newMcpId" :placeholder="tr('服务器 ID','server id')" autocomplete="off" autocapitalize="off" spellcheck="false" @keydown.enter.prevent="addMcp"/>
            <button type="button" class="btn ghost" :disabled="busy || !newMcpId.trim()" @click="addMcp"><Icon name="plus"/>{{tr('添加服务器','Add server')}}</button>
          </div>
        </section>
      </fieldset>

      <div v-else class="provider-settings">
        <section class="set-section">
          <header class="set-section-head"><h3>{{tr('模型提供商','Model providers')}}</h3><p v-if="!isMobile">{{tr('选择预置服务商后填写密钥，也可以引用服务器环境变量。','Choose a preset, then enter credentials or reference server environment variables.')}}</p></header>
          <div class="provider-list">
            <PresetProvider v-for="(provider,id) in draft.providers" :key="id" :id="String(id)" :initially-open="id===newProviderId" :value="provider" :preset="findPreset(provider.preset)" :protocols="protocolOptions" :save="save" :saving="busy" :disabled="busy" @remove="removeProvider(String(id))" @protocol="changeProtocol(String(id),$event)" @login-complete="load">
              <details class="provider-json" @toggle="($event.target as HTMLDetailsElement).open&&!advancedPending[id]&&(advanced[id]=JSON.stringify(provider,null,2))"><summary><span>{{tr('完整配置 JSON','Full configuration JSON')}}</span><Icon name="chevron-down"/></summary><textarea class="input code" rows="16" v-model="advanced[id]" @input="advancedPending[id]=true"/><button v-if="!isMobile" class="btn" @click="applyAdvanced(String(id))">{{tr('应用到表单','Apply to form')}}</button></details>
            </PresetProvider>
            <button type="button" class="provider-add" :disabled="busy" @click="adding=true"><span class="provider-add-icon"><Icon name="plus"/></span><span>{{tr('添加提供商','Add provider')}}</span></button>
          </div>
        </section>
        <section v-if="draft.proxy" class="set-section">
          <header class="set-section-head"><h3>{{tr('网络代理','Network proxy')}}</h3><p>{{tr('提供商请求使用的代理，每个提供商可以在连接设置中单独关闭。','Used for provider requests. Each provider can opt out in its connection settings.')}}</p></header>
          <ServerProxySettings :value="draft.proxy" :environment="proxyEnvironment"/>
        </section>
      </div>
    </template>
    <div v-else-if="!busy" class="settings-empty"><p>{{tr('设置尚未载入。','Settings are not loaded.')}}</p><button class="btn" @click="load"><Icon name="refresh-cw"/>{{tr('重新载入','Reload')}}</button></div>
    </div>
      <footer v-if="draft&&tab!=='ui'&&!isMobile" class="settings-footer">
        <span class="settings-status" :class="{dirty}"><i aria-hidden="true"/>{{dirty?tr('有未保存的修改','Unsaved changes'):tr('所有修改已保存','All changes saved')}}</span>
        <button class="btn ghost" :disabled="busy||!dirty" @click="discard=true">{{tr('放弃修改','Discard')}}</button>
        <button class="btn primary" :disabled="busy||!dirty" @click="save"><Icon :name="busy?'loader-circle':'save'" :class="{spin:busy}"/>{{busy?tr('保存中…','Saving…'):tr('保存','Save')}}</button>
      </footer>
    </section>

    <AddProvider v-if="adding" :catalog="catalog" @close="adding=false" @select="addProvider"/>
    <Modal compact :open="discard" :title="tr('放弃修改？','Discard changes?')" @close="discard=false"><p>{{tr('丢弃未保存的修改，恢复已保存的配置。','Discard unsaved changes and restore the saved configuration.')}}</p><template #footer><button class="btn" @click="discard=false">{{tr('继续编辑','Keep editing')}}</button><button class="btn danger" @click="discardChanges">{{tr('放弃修改','Discard changes')}}</button></template></Modal>
    <Modal compact :layer="120" :dismissable="!leaveBusy" :open="leave" :title="isMobile?tr('保存修改？','Save changes?'):tr('尚未保存','Unsaved changes')" @close="resolveLeave(false)"><p>{{tr('返回前是否保存已进行的修改？','Save your changes before returning?')}}</p><template #footer><button class="btn ghost" :disabled="leaveBusy" @click="resolveLeave(false)">{{tr('继续编辑','Keep editing')}}</button><button class="btn danger" :disabled="leaveBusy" @click="resolveLeave(true)">{{tr('放弃修改','Discard')}}</button><button class="btn primary" :disabled="leaveBusy" @click="saveAndReturn"><Icon v-if="leaveBusy" name="loader-circle" class="spinner"/>{{leaveBusy?tr('保存中…','Saving…'):tr('保存并返回','Save & return')}}</button></template></Modal>
  </div>
    </DialogContent>
  </DialogPortal>
  </DialogRoot>
</template>
<style scoped>
.native-settings { width: 100%; height: 100%; min-height: 0; margin: 0 auto; overflow: hidden; }
.settings-sidebar { flex: none; display: flex; flex-direction: column; min-height: 0; }
.settings-title { margin: 0; font-size: 16px; font-weight: 600; }
.settings-sidebar nav { display: flex; flex-direction: column; gap: 2px; }
.settings-section { display: flex; align-items: center; gap: 10px; width: 100%; padding: 6px 10px 6px 6px; border: 0; border-radius: 9px; background: transparent; color: var(--fg-muted); font: inherit; font-size: 13.5px; text-align: left; cursor: pointer; transition: background var(--dur-fast), color var(--dur-fast); }
.settings-section-icon { display: grid; place-items: center; flex: none; width: 28px; height: 28px; border-radius: 8px; color: var(--fg-subtle); transition: background var(--dur-fast), color var(--dur-fast); }
.settings-section-icon .icon { width: 16px; height: 16px; }
.settings-section-text { flex: 1; min-width: 0; }
@media (hover: hover) { .settings-section:hover { background: var(--bg-hover); color: var(--fg); } }
.settings-section.selected { background: var(--bg-raised); color: var(--fg); font-weight: 500; box-shadow: 0 0 0 1px var(--line), 0 1px 3px rgb(0 0 0 / 5%); }
.settings-section.selected .settings-section-icon { background: var(--accent-soft); color: var(--accent); }
.settings-brand { display: flex; align-items: center; gap: 8px; margin-top: auto; color: var(--fg); }
.settings-brand img { display: block; width: 26px; height: auto; }
.settings-brand-word { height: 12px; }
.settings-brand span { margin-left: auto; font-size: 11px; color: var(--fg-faint); font-variant-numeric: tabular-nums; }

.settings-detail { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; }
.settings-heading { display: flex; align-items: center; gap: 12px; flex: none; }
.settings-heading-text { flex: 1; min-width: 0; }
.settings-heading h2 { margin: 0; font-size: 18px; font-weight: 600; line-height: 1.4; }
.settings-heading-text p { margin: 2px 0 0; font-size: 12px; line-height: 1.5; color: var(--fg-subtle); }
.settings-content { flex: 1; min-height: 0; overflow: auto; }
.settings-form { display: block; min-width: 0; margin: 0; padding: 0; border: 0; }
.settings-empty { display: grid; justify-items: center; gap: 14px; padding: 56px 0; color: var(--fg-subtle); }
.settings-empty p { margin: 0; }
.settings-empty .btn { display: inline-flex; align-items: center; gap: 7px; }
.settings-empty .icon { width: 15px; height: 15px; }
.code { width: 100%; padding: .75em; font-family: var(--mono); font-size: calc(1em * var(--mono-scale)); line-height: 1.8; }
.input { min-width: 0; max-width: 100%; }

.settings-footer { flex: none; display: flex; align-items: center; gap: 10px; border-top: 1px solid var(--line); }
.settings-status { display: inline-flex; align-items: center; gap: 8px; margin-right: auto; font-size: 12px; color: var(--fg-subtle); }
.settings-status i { width: 7px; height: 7px; border-radius: 50%; background: var(--ok); opacity: .8; }
.settings-status.dirty { color: var(--fg-muted); }
.settings-status.dirty i { background: var(--accent); opacity: 1; box-shadow: 0 0 0 3px var(--accent-soft); }
.settings-footer .btn { display: inline-flex; align-items: center; gap: 7px; }
.settings-footer .btn .icon { width: 15px; height: 15px; }
.settings-overlay { position: fixed; inset: 0; z-index: 50; background: var(--scrim); backdrop-filter: blur(4px); }

@media (min-width: 900px) {
  .native-settings { position: fixed; z-index: 51; top: 50%; left: 50%; transform: translate(-50%, -50%); display: flex; flex-direction: row; width: min(980px, 92vw); height: min(740px, 88dvh); max-width: none; margin: 0; padding: 0; background: var(--bg); border: 1px solid var(--line-strong); border-radius: 16px; box-shadow: var(--shadow-pop); animation: settings-enter 180ms var(--ease-out); }
  .native-settings[data-state='closed'] { animation: settings-exit 180ms ease-in forwards; }
  .settings-overlay[data-state='closed'] { animation: settings-overlay-out 180ms ease-in forwards; pointer-events: none; }
  .settings-sidebar { width: 216px; padding: 20px 12px 16px; border-right: 1px solid var(--line); background: var(--bg-sunken); }
  .settings-home-heading { padding: 0 8px 16px; }
  .settings-brand { padding: 12px 8px 0; }
  .settings-detail > .settings-heading { padding: 18px 20px 14px 28px; border-bottom: 1px solid var(--line); }
  .settings-content { padding: 22px 28px 28px; }
  .settings-footer { padding: 12px 20px 12px 28px; background: var(--bg); }
  .provider-list { display: grid; gap: 12px; }
}

@media (max-width: 899px) {
  .native-settings { background: var(--bg-sunken); }
  /* Tablets keep the phone layout, centered at a readable width. */
  .settings-sidebar { width: 100%; height: 100%; padding: 0 max(16px, calc((100% - 640px) / 2)) max(20px, env(safe-area-inset-bottom)); overflow: auto; }
  .settings-home-heading { flex-direction: column; align-items: flex-start; gap: 2px; margin: 0 -16px 18px; padding: 6px 8px 0; }
  .settings-home-heading .settings-title { padding: 0 16px; font-size: 28px; font-weight: 700; letter-spacing: -.01em; }
  .settings-sidebar nav { gap: 0; border: 1px solid var(--line); border-radius: 14px; background: var(--bg-raised); overflow: hidden; }
  .settings-section { min-height: 66px; padding: 12px 14px; gap: 14px; border-radius: 0; color: var(--fg); font-size: 15px; font-weight: 500; }
  /* Dividers start after the icon, as in native grouped lists. */
  .settings-section + .settings-section { background: linear-gradient(var(--line), var(--line)) right top / calc(100% - 62px) 1px no-repeat; }
  .settings-section:active { background-color: var(--bg-hover); }
  .settings-section-icon { width: 34px; height: 34px; border-radius: 10px; background: var(--accent-soft); color: var(--accent); }
  .settings-section-icon .icon { width: 18px; height: 18px; }
  .category-description { display: block; margin-top: 2px; font-size: 12px; font-weight: 400; color: var(--fg-subtle); }
  .section-chevron { flex: none; width: 16px; color: var(--fg-faint); }
  .settings-brand { flex-direction: column; justify-content: center; gap: 8px; padding: 40px 0 8px; }
  .settings-brand img { width: 40px; }
  .settings-brand-word { height: 18px; }
  .settings-brand span { margin: 0; }
  .settings-detail > .settings-heading { min-height: 54px; padding: 6px 8px; gap: 4px; border-bottom: 1px solid var(--line); background: var(--bg); }
  .settings-heading h2 { font-size: 17px; }
  .settings-content { padding: 20px max(16px, calc((100% - 640px) / 2)) max(32px, env(safe-area-inset-bottom)); }
  .native-settings[data-state='closed'] { animation: settings-page-out 180ms ease-in forwards; pointer-events: none; }
  .provider-list { border: 1px solid var(--line); border-radius: 14px; background: var(--bg-raised); overflow: hidden; }
}
@keyframes settings-exit { from { opacity: 1; translate: 0 0; scale: 1; } to { opacity: 0; translate: 0 8px; scale: .985; } }
@keyframes settings-overlay-out { from { opacity: 1; } to { opacity: 0; } }
@keyframes settings-page-out { from { opacity: 1; transform: translateX(0); } to { opacity: 0; transform: translateX(24px); } }
</style>
