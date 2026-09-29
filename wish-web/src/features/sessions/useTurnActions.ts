import { computed, ref } from 'vue';
import { useRouter } from 'vue-router';
import * as api from '../../core/api/endpoints.ts';
import { chat } from '../../core/state/chatSlice.ts';
import { sessions } from '../../core/state/sessionsSlice.ts';
import { tr } from '../../core/i18n/tr.ts';
import { toast } from '../../ui/toast.ts';
import { showError } from '../../ui/errorDialog.ts';

// Turn-level actions borrowed from codex/zcode: rewind the live thread back to a
// chosen turn (later turns leave the thread but the sealed generation keeps them),
// or branch a fresh session that starts from that turn. Both act on an EntryId.
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

  return { canAct, busy, rewind, branch };
}
