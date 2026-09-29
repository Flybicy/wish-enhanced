// UI preferences slice. Persisted through the platform storage adapter —
// in a shell these become native settings with zero feature changes.
import { shallowRef, type ShallowRef } from 'vue';
import { platform } from '../../platform/index.ts';

export const prefs = (() => {
  const sendOnEnter = shallowRef(true);
  const keepAwake = shallowRef(false);
  const showAdvanced = shallowRef(false);
  const notifyOnFailure = shallowRef(false);   // decisions 22: default OFF
  const sessionListCollapsed = shallowRef(false);
  // The right workspace rail is on by default: a wide window reads better with
  // the workspace in view, and the toggle remembers the choice either way.
  const workspaceRail = shallowRef(true);
  // The rail's note panel starts closed: it is a place to jot an aside, not a
  // panel that should claim space before it is wanted.
  const btwRail = shallowRef(false);
  // Sessions group by working directory by default: one folder is the closest
  // thing this application has to a project, and the list reads better for it.
  const groupSessions = shallowRef(true);
  const loaded = shallowRef(false);

  const KEYS = { sendOnEnter: 'pref.sendOnEnter', keepAwake: 'pref.keepAwake', showAdvanced: 'pref.showAdvanced', notifyOnFailure: 'pref.notifyOnFailure', sessionListCollapsed: 'pref.sessionListCollapsed', workspaceRail: 'pref.workspaceRail', btwRail: 'pref.btwRail', groupSessions: 'pref.groupSessions' };

  function load() {
    const s = platform('storage');
    if (s.get(KEYS.sendOnEnter) != null) sendOnEnter.value = s.get(KEYS.sendOnEnter) === '1';
    if (s.get(KEYS.keepAwake) != null) keepAwake.value = s.get(KEYS.keepAwake) === '1';
    if (s.get(KEYS.showAdvanced) != null) showAdvanced.value = s.get(KEYS.showAdvanced) === '1';
    if (s.get(KEYS.notifyOnFailure) != null) notifyOnFailure.value = s.get(KEYS.notifyOnFailure) === '1';
    sessionListCollapsed.value = s.get(KEYS.sessionListCollapsed) === '1';
    if (s.get(KEYS.workspaceRail) != null) workspaceRail.value = s.get(KEYS.workspaceRail) === '1';
    if (s.get(KEYS.groupSessions) != null) groupSessions.value = s.get(KEYS.groupSessions) === '1';
    loaded.value = true;
  }

  function persist(key: string, sig: ShallowRef<boolean>) {
    platform('storage').set(key, sig.value ? '1' : '0');
  }

  return {
    sendOnEnter, keepAwake, showAdvanced, notifyOnFailure, sessionListCollapsed, workspaceRail, btwRail, groupSessions, loaded,
    load,
    setSendOnEnter(v: boolean) { sendOnEnter.value = v; persist(KEYS.sendOnEnter, sendOnEnter); },
    setKeepAwake(v: boolean) { keepAwake.value = v; persist(KEYS.keepAwake, keepAwake); },
    setShowAdvanced(v: boolean) { showAdvanced.value = v; persist(KEYS.showAdvanced, showAdvanced); },
    setNotifyOnFailure(v: boolean) { notifyOnFailure.value = v; persist(KEYS.notifyOnFailure, notifyOnFailure); },
    setSessionListCollapsed(v: boolean) { sessionListCollapsed.value = v; persist(KEYS.sessionListCollapsed, sessionListCollapsed); },
    setWorkspaceRail(v: boolean) { workspaceRail.value = v; persist(KEYS.workspaceRail, workspaceRail); },
    setBtwRail(v: boolean) { btwRail.value = v; persist(KEYS.btwRail, btwRail); },
    setGroupSessions(v: boolean) { groupSessions.value = v; persist(KEYS.groupSessions, groupSessions); },
  };
})();
export type PrefsApi = typeof prefs;
