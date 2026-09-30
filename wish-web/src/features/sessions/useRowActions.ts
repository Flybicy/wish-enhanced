// Row menu actions shared by the sidebar list and the start page's recent
// list: metadata writes (pin, archive) and session forks are one-shot API
// calls confirmed by a toast. A failure keeps the list untouched and
// surfaces in the error dialog instead of half-applying.
import { useRouter } from 'vue-router';
import { sessions } from '../../core/state/sessionsSlice.ts';
import * as api from '../../core/api/endpoints.ts';
import { tr } from '../../core/i18n/tr.ts';
import { toast } from '../../ui/toast.ts';
import { showError } from '../../ui/errorDialog.ts';

export function useRowActions() {
  const router = useRouter();
  async function runRowAction(row: any, kind: string): Promise<void> {
    try {
      if (kind === 'pin' || kind === 'unpin') {
        await sessions.updateMeta(row, kind === 'pin' ? { pinned: true } : { pinned: undefined });
        toast(kind === 'pin' ? tr('已置顶。', 'Pinned.') : tr('已取消置顶。', 'Unpinned.'));
      } else if (kind === 'archive' || kind === 'unarchive') {
        await sessions.updateMeta(row, kind === 'archive' ? { archived: true } : { archived: undefined });
        toast(kind === 'archive' ? tr('已归档。', 'Archived.') : tr('已取消归档。', 'Unarchived.'));
      } else if (kind === 'branch') {
        const snap = await api.sessionFork(row.id);
        await sessions.rebuild();
        toast(tr('已分支为新会话。', 'Branched into a new session.'));
        await router.push('/s/' + snap.id);
      }
    } catch (e: any) {
      showError({ title: tr('操作失败', 'Operation failed'), error: e });
    }
  }
  return { runRowAction };
}
