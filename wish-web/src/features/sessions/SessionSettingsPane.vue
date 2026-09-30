<script setup lang="ts">
// Settings that belong to this session alone: its shell and context actions.
// New sessions keep taking the defaults from Settings → Service & sessions.
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { SwitchRoot, SwitchThumb } from 'reka-ui';
import { get } from '../../core/api/client.ts';
import * as api from '../../core/api/endpoints.ts';
import type { PermissionMode } from '../../core/api/endpoints.ts';
import { chat } from '../../core/state/chatSlice.ts';
import { tr } from '../../core/i18n/tr.ts';
import Modal from '../../ui/components/Modal.vue';
import Icon from '../../ui/components/Icon.vue';
import SelectField from '../../ui/components/SelectField.vue';
import { useMedia } from '../../ui/composables/useMedia.ts';
import { showError } from '../../ui/errorDialog.ts';
import { toast } from '../../ui/toast.ts';
import ServerShellSettings, { type ShellCatalog } from '../settings/ServerShellSettings.vue';
import '../settings/settings.css';

defineEmits<{ close: [] }>();
const isMobile = useMedia('(max-width: 899px)');
const snapshot = computed(() => chat.snapshot.value);
const hasShell = computed(() => !!snapshot.value?.descriptor?.shell);

type Shell = { program: string; args: string[] | null };
const ownShell = ref(false);
const shell = ref<Shell>({ program: '', args: null });
const permission = ref<PermissionMode>('operate');
const source = ref('');
const draftValue = () => JSON.stringify({ shell: ownShell.value ? shell.value : null, permission: permission.value });
const dirty = computed(() => !!source.value && draftValue() !== source.value);
const permissionOptions = computed(() => [
  { value: 'operate', label: tr('自动执行', 'Auto-execute') },
  { value: 'ask', label: tr('每次询问', 'Ask every time') },
  { value: 'read_only', label: tr('只读', 'Read only') },
]);

let loadedFor: string | null = null;
function reset() {
  const current = snapshot.value;
  if (!current) return;
  loadedFor = current.id;
  const own = current.descriptor?.shell_command;
  ownShell.value = !!own;
  shell.value = own ? { program: own.program ?? '', args: own.args ?? null } : { program: '', args: null };
  permission.value = current.permission ?? 'operate';
  source.value = draftValue();
}
// The page can open before the session's snapshot arrives (a direct link);
// build the draft once it does, and again for another session — never over edits.
watch(snapshot, value => { if (value && value.id !== loadedFor) reset(); }, { immediate: true });

// The shell label needs the catalog and the global setting.
const catalog = ref<ShellCatalog | null>(null);
const globalShell = ref<Shell | null>(null);
let alive = true;
onUnmounted(() => { alive = false; });
onMounted(async () => {
  const [shells, config] = await Promise.allSettled([get('/shells'), get('/config')]);
  if (!alive) return;
  if (shells.status === 'fulfilled') catalog.value = shells.value;
  if (config.status === 'fulfilled') globalShell.value = config.value.config.shell ?? null;
});
const shellName = (value: Shell | null) => {
  const program = value?.program || catalog.value?.default.program;
  return program ? program.split(/[\\/]/).pop() : tr('系统默认', 'System default');
};
// Leaving the global shell starts from its values rather than a blank choice.
function setOwnShell(own: boolean) {
  ownShell.value = own;
  if (own && !snapshot.value?.descriptor?.shell_command && globalShell.value) shell.value = { program: globalShell.value.program ?? '', args: globalShell.value.args ?? null };
}

const busy = ref(false);
async function save() {
  const id = chat.sessionId.value;
  const current = snapshot.value;
  if (!id || !current || busy.value) return;
  const saved = JSON.parse(source.value);
  busy.value = true;
  try {
    const wanted = ownShell.value ? shell.value : null;
    if (JSON.stringify(wanted) !== JSON.stringify(saved.shell)) {
      const next = await api.sessionSetShell(id, wanted);
      if (chat.sessionId.value === id) chat.snapshot.value = next;
    }
    if (permission.value !== saved.permission) {
      const next = await api.sessionSetPermission(id, permission.value);
      if (chat.sessionId.value === id) chat.snapshot.value = next;
    }
    if (chat.sessionId.value === id) { reset(); toast(tr('会话设置已保存。', 'Session settings saved.')); }
  } catch (error) {
    showError({ title: tr('无法保存会话设置', 'Could not save session settings'), error });
  } finally {
    busy.value = false;
  }
}

// Context actions that used to live in the management window.
const confirmClear = ref(false);
async function act(kind: 'compact' | 'clear') {
  const id = chat.sessionId.value;
  if (!id || busy.value) return;
  busy.value = true;
  try {
    if (kind === 'compact') {
      await api.sessionCompact(id);
      toast(tr('已开始压缩上下文。', 'Compaction started.'));
    } else {
      await api.sessionClearContext(id);
      confirmClear.value = false;
      await chat.reload();
      toast(tr('已清空当前上下文，历史记录仍然保留。', 'Context cleared. History is kept.'));
    }
  } catch (error) {
    showError({ title: kind === 'compact' ? tr('无法压缩上下文', 'Could not compact the context') : tr('无法清空上下文', 'Could not clear the context'), error });
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Modal :open="true" content-class="session-window session-settings-window" :title="tr('会话设置', 'Session settings')" :page="isMobile" @close="$emit('close')">
    <p class="session-settings-intro">{{ tr('只影响这个会话。新建会话仍使用「设置 → 服务与会话」中的默认值。', 'These apply to this session only. New sessions keep using the defaults in Settings → Service & sessions.') }}</p>

    <section class="set-section">
      <header class="set-section-head"><h3>{{ tr('上下文压缩', 'Context compaction') }}</h3><p>{{ tr('接近上限时自动压缩；预算跟随「设置 → 服务与会话」的默认值。', 'Compacts automatically near the limit; budgets follow the defaults in Settings → Service & sessions.') }}</p></header>
      <div class="set-card context-actions">
        <div class="set-row inline"><span class="set-label"><span>{{ tr('立即压缩', 'Compact now') }}</span><small>{{ tr('现在就把较早的内容压缩成摘要', 'Summarize earlier turns right away') }}</small></span><button type="button" class="btn" :disabled="busy || !snapshot?.config?.compaction" @click="act('compact')">{{ tr('压缩', 'Compact') }}</button></div>
        <div class="set-row inline"><span class="set-label"><span>{{ tr('清空上下文', 'Clear the context') }}</span><small>{{ tr('之后从空白上下文继续，历史记录保留', 'Continue from an empty context; history is kept') }}</small></span><button type="button" class="btn danger" :disabled="busy" @click="confirmClear = true">{{ tr('清空', 'Clear') }}</button></div>
      </div>
    </section>

    <section class="set-section">
      <header class="set-section-head"><h3>{{ tr('权限模式', 'Permission mode') }}</h3><p>{{ tr('控制工具的执行方式；会话运行中也可以随时切换。', 'Controls how tools run; it can switch any time, even mid-run.') }}</p></header>
      <div class="set-card">
        <label class="set-row inline"><span class="set-label"><span>{{ tr('模式', 'Mode') }}</span><small>{{ tr('自动执行 / 每次询问 / 只读', 'Auto-execute / Ask every time / Read only') }}</small></span><SelectField mobile-page :picker-title="tr('权限模式', 'Permission mode')" :model-value="permission" :options="permissionOptions" :aria-label="tr('权限模式', 'Permission mode')" @update:model-value="permission = $event as PermissionMode" /></label>
      </div>
    </section>

    <section v-if="hasShell" class="set-section">
      <header class="set-section-head"><h3>Shell</h3><p>{{ tr('修改后从这个会话的下一条命令开始生效。', 'Applies from this session\'s next command.') }}</p></header>
      <div class="set-card">
        <div class="set-row inline toggle-row"><span class="set-label"><span>{{ tr('跟随全局设置', 'Follow the global setting') }}</span><small>{{ tr('当前全局：', 'Global: ') }}{{ shellName(globalShell) }}</small></span><SwitchRoot :model-value="!ownShell" class="cfg-switch" :aria-label="tr('跟随全局设置', 'Follow the global setting')" @update:model-value="setOwnShell(!$event)"><SwitchThumb class="cfg-switch-thumb" /></SwitchRoot></div>
        <ServerShellSettings v-if="ownShell" :value="shell" :catalog="catalog" />
      </div>
    </section>

    <template #footer>
      <span class="session-settings-status">{{ dirty ? tr('有未保存的修改', 'Unsaved changes') : tr('没有未保存的修改', 'No unsaved changes') }}</span>
      <button type="button" class="btn ghost" :disabled="busy || !dirty" @click="reset">{{ tr('放弃', 'Discard') }}</button>
      <button type="button" class="btn primary" :disabled="busy || !dirty" @click="save"><Icon :name="busy ? 'loader-circle' : 'save'" :class="{ spin: busy }" />{{ tr('保存', 'Save') }}</button>
    </template>
  </Modal>
  <Modal compact :open="confirmClear" :dismissable="!busy" :title="tr('清空当前上下文？', 'Clear the current context?')" @close="confirmClear = false">
    <p>{{ tr('模型之后看不到之前的对话内容，历史记录仍然保留并可以搜索。', 'The model will no longer see earlier turns. History stays available and searchable.') }}</p>
    <template #footer><button class="btn ghost" :disabled="busy" @click="confirmClear = false">{{ tr('取消', 'Cancel') }}</button><button class="btn danger" :disabled="busy" @click="act('clear')">{{ tr('清空', 'Clear') }}</button></template>
  </Modal>
</template>

<style>
.modal-card.session-settings-window:not(.modal-page) { width: min(92vw, 30rem); }
.session-settings-intro { margin: 0 0 18px; font-size: 12px; line-height: 1.6; color: var(--fg-subtle); }
.session-settings-window .set-row.inline { grid-template-columns: minmax(0, 1fr) auto; }
.session-settings-window .context-actions .btn { min-width: 64px; }
.session-settings-window .modal-foot { align-items: center; }
.session-settings-status { margin-right: auto; font-size: 12px; color: var(--fg-subtle); }
.session-settings-window .modal-foot .btn { display: inline-flex; align-items: center; gap: 6px; }
.session-settings-window .modal-foot .icon { width: 15px; height: 15px; }
</style>
