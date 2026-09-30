import { computed, ref } from 'vue';
import { useRouter } from 'vue-router';
import * as api from '../../core/api/endpoints.ts';
import { chat } from '../../core/state/chatSlice.ts';
import { sessions } from '../../core/state/sessionsSlice.ts';
import { tr } from '../../core/i18n/tr.ts';
import { toast } from '../../ui/toast.ts';
import { showError } from '../../ui/errorDialog.ts';
import type { EntryView } from '../../core/api/projections.ts';

// Turn-level actions borrowed from codex/zcode: rewind the live thread back to a
// chosen turn (later turns leave the thread but the sealed generation keeps them),
// or branch a fresh session that starts from that turn. Both act on an EntryId.
// 重新回复 (regenerate) and 修改 (edit) drop the clicked user message itself: the
// rewind target is the live thread's previous entry, then the original send is
// repeated as-is, or handed back to the composer as an editable draft.
export function useTurnActions() {
  const router = useRouter();
  const busy = ref(false);
  // Only when the session is idle: rebuilding the active generation needs a still
  // thread, and the server rejects the call while it runs.
  const canAct = computed(() => !busy.value && !chat.snapshot.value?.running);

  async function rewind(entryId: number) {
    const id = chat.sessionId.value;
    if (!id || !canAct.value) return;
    busy.value = true;
    try {
      await api.sessionRewind(id, entryId);
      await chat.reload();
      toast(tr('已回档到此处，后面的内容移出主线（历史仍保留）。', 'Rewound here. Later turns left the thread; history is kept.'));
    } catch (error) {
      showError({ title: tr('无法回档', 'Could not rewind'), error });
    } finally {
      busy.value = false;
    }
  }

  async function branch(entryId: number) {
    const id = chat.sessionId.value;
    if (!id || !canAct.value) return;
    busy.value = true;
    try {
      const snap = await api.sessionFork(id, entryId);
      await sessions.refresh();
      await router.push('/s/' + snap.id);
      toast(tr('已从此处分支出新会话。', 'Branched a new session from here.'));
    } catch (error) {
      showError({ title: tr('无法分支', 'Could not branch'), error });
    } finally {
      busy.value = false;
    }
  }

  // The live thread's ordered entry ids, straight from the active generation.
  // The flat chat log keeps sealed turns visible after a rewind, so the entry
  // before a message cannot be read off the rendered list.
  async function liveThreadIds(): Promise<number[]> {
    const id = chat.sessionId.value;
    const active = chat.snapshot.value?.status?.active_generation;
    if (!id || active == null) return [];
    const ids: number[] = [];
    let start = 0;
    for (let pages = 0; pages < 100; pages++) {
      const page = await api.generationEntries(id, active, { start, limit: 1000 });
      ids.push(...page.items);
      if (page.next == null) break;
      start = page.next;
    }
    return ids;
  }

  // Rewind so the clicked user message leaves the live thread: the target is the
  // previous live entry, or nothing when the message opens the thread. True when
  // the rewind succeeded and the session is ready for the next input.
  async function rewindBefore(entry: EntryView): Promise<boolean> {
    const id = chat.sessionId.value;
    if (!id) return false;
    const thread = await liveThreadIds();
    const index = thread.indexOf(entry.id as number);
    if (index < 0) {
      toast(tr('这条消息已不在主线中。', 'This message is no longer in the live thread.'));
      return false;
    }
    await api.sessionRewind(id, index > 0 ? thread[index - 1] : undefined);
    await chat.reload();
    return true;
  }

  async function regenerate(entry: EntryView) {
    const resend = entry.payload?.resend;
    if (!canAct.value || !resend) return;
    busy.value = true;
    try {
      const id = chat.sessionId.value;
      if (!id || !(await rewindBefore(entry))) return;
      await api.messageSend(id, { content: resend.text, blocks: resend.attachments });
      void chat.fetchNewer();
      toast(tr('已重新发送这条消息。', 'Resent this message.'));
    } catch (error) {
      showError({ title: tr('无法重新回复', 'Could not regenerate'), error });
    } finally {
      busy.value = false;
    }
  }

  async function edit(entry: EntryView) {
    const resend = entry.payload?.resend;
    if (!canAct.value || !resend) return;
    busy.value = true;
    try {
      const id = chat.sessionId.value;
      if (!id || !(await rewindBefore(entry))) return;
      chat.prefill.value = {
        sessionId: id,
        text: resend.text,
        attachments: resend.attachments.map(a => ({
          kind: a.type, blob_id: `${id}/${a.blob_id}`, filename: a.filename ?? undefined,
          byte_count: a.byte_count ?? undefined, placeholder: a.placeholder,
        })),
      };
      toast(tr('原消息已回到输入框，修改后发送。', 'The original message is back in the composer; edit it and send.'));
    } catch (error) {
      showError({ title: tr('无法修改', 'Could not edit'), error });
    } finally {
      busy.value = false;
    }
  }

  return { canAct, busy, rewind, branch, regenerate, edit };
}
