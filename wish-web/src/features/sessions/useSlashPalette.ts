import { computed, ref, watch, type Ref } from 'vue';
import { useRouter } from 'vue-router';
import * as api from '../../core/api/endpoints.ts';
import { get } from '../../core/api/client.ts';
import { chat } from '../../core/state/chatSlice.ts';
import { sessions } from '../../core/state/sessionsSlice.ts';
import { tr } from '../../core/i18n/tr.ts';
import { toast } from '../../ui/toast.ts';
import { showError } from '../../ui/errorDialog.ts';

// A codex/zcode-style "/" palette for the composer. Typing "/" as the very first
// character (no space yet) opens a menu of session commands and skill references.
// A command acts on the session and clears the draft; a skill inserts an @mention
// the model reads as an explicit reference, leaving the caret ready to keep typing.
export type SlashKind = 'command' | 'skill';
export interface SlashItem { key: string; kind: SlashKind; icon: string; title: string; desc: string; keywords: string; run: () => void | Promise<void> }

export function useSlashPalette(params: {
  sessionId: Ref<string>;
  enabled: Ref<boolean>;
  text: Ref<string>;
  setText: (value: string) => void;
  focusEditor: () => void;
}) {
  const router = useRouter();
  const busy = ref(false);
  const active = ref(0);
  // Escape parks the palette on the exact draft that dismissed it; any further
  // edit differs from that parked value and lets the palette open again.
  const dismissed = ref<string | null>(null);

  // The query is whatever follows the leading "/", so "/comp" filters to compact.
  const query = computed(() => (params.text.value.startsWith('/') ? params.text.value.slice(1) : ''));

  // Skills are loaded once, lazily, on first open — the catalog is small and the
  // reader rarely opens the palette on the very first keystroke.
  const skills = ref<{ name: string; description?: string; triggers?: string[] }[] | null>(null);
  async function ensureSkills() {
    if (skills.value !== null) return;
    try {
      const catalog: any = await get('/skills');
      skills.value = Array.isArray(catalog?.skills) ? catalog.skills : [];
    } catch {
      skills.value = [];
    }
  }

  function insertSkill(name: string) {
    // A leading @mention reads as an explicit reference beside the model's own
    // skill search; the trailing space leaves the caret ready for the request.
    params.setText('@' + name + ' ');
    dismissed.value = null;
    params.focusEditor();
  }

  const commands = computed<SlashItem[]>(() => [
    { key: 'cmd:compact', kind: 'command', icon: 'layers',
      title: tr('整理上下文', 'Compact context'),
      desc: tr('总结较早的对话以释放上下文，历史仍保留', 'Summarize earlier turns to free context; history is kept'),
      keywords: 'compact zhengli yasuo 整理 压缩',
      run: async () => {
        const id = params.sessionId.value; params.setText('');
        try { await api.sessionCompact(id); toast(tr('已请求整理上下文。', 'Requested a context compaction.')); }
        catch (error) { showError({ title: tr('无法整理上下文', 'Could not compact'), error }); }
      } },
    { key: 'cmd:clear', kind: 'command', icon: 'trash-2',
      title: tr('清空上下文', 'Clear context'),
      desc: tr('新起一段活动上下文，历史记录仍保留', 'Start a fresh active context; history is kept'),
      keywords: 'clear reset qingkong chongzhi 清空 重置',
      run: async () => {
        const id = params.sessionId.value; params.setText('');
        try { await api.sessionClearContext(id); await chat.reload(); toast(tr('已清空活动上下文。', 'Active context cleared.')); }
        catch (error) { showError({ title: tr('无法清空上下文', 'Could not clear'), error }); }
      } },
    { key: 'cmd:branch', kind: 'command', icon: 'git-branch',
      title: tr('分支会话', 'Branch session'),
      desc: tr('从当前对话复制出一个新会话', 'Copy this conversation into a new session'),
      keywords: 'branch fork fenzhi fuzhi 分支 复制',
      run: async () => {
        const id = params.sessionId.value; params.setText('');
        try { const snap = await api.sessionFork(id); await sessions.refresh(); await router.push('/s/' + snap.id); toast(tr('已分支出新会话。', 'Branched a new session.')); }
        catch (error) { showError({ title: tr('无法分支', 'Could not branch'), error }); }
      } },
  ]);

  const skillItems = computed<SlashItem[]>(() =>
    (skills.value ?? []).map(skill => ({
      key: 'skill:' + skill.name,
      kind: 'skill' as const,
      icon: 'sparkles',
      title: skill.name,
      desc: skill.description || tr('引用这个技能', 'Reference this skill'),
      keywords: (skill.triggers ?? []).join(' '),
      run: () => insertSkill(skill.name),
    })),
  );

  const items = computed<SlashItem[]>(() => {
    const term = query.value.trim().toLocaleLowerCase();
    const pool = [...commands.value, ...skillItems.value];
    if (!term) return pool;
    return pool.filter(item => (item.title + ' ' + item.desc + ' ' + item.keywords).toLocaleLowerCase().includes(term));
  });

  const open = ref(false);
  watch([params.text, params.enabled], () => {
    const matches = params.enabled.value && /^\/[^\s]*$/.test(params.text.value);
    open.value = matches && params.text.value !== dismissed.value;
    if (open.value) { void ensureSkills(); }
  }, { immediate: true });

  // Keep the highlight in range as the result set shrinks or grows.
  watch(items, list => { if (active.value >= list.length) active.value = Math.max(0, list.length - 1); });
  watch(open, isOpen => { if (isOpen) active.value = 0; });

  function close() { dismissed.value = params.text.value; open.value = false; }

  async function choose(key: string) {
    if (busy.value) return;
    const item = items.value.find(entry => entry.key === key);
    if (!item) return;
    open.value = false;
    busy.value = true;
    try { await item.run(); } finally { busy.value = false; }
  }

  // Returns true when the palette consumed the key, so the composer skips its own
  // Enter-to-send and Escape-to-interrupt handling for that stroke.
  function onKeydown(event: KeyboardEvent): boolean {
    if (!open.value) return false;
    const count = items.value.length;
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault(); if (count) active.value = (active.value + 1) % count; return true;
      case 'ArrowUp':
        event.preventDefault(); if (count) active.value = (active.value - 1 + count) % count; return true;
      case 'Enter':
      case 'Tab':
        if (!count) return false;
        event.preventDefault(); void choose(items.value[active.value]!.key); return true;
      case 'Escape':
        event.preventDefault(); close(); return true;
      default:
        return false;
    }
  }

  return { open, items, active, busy, onKeydown, choose, close };
}
