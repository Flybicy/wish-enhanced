// Service worker retirement.
//
// The interface ships inside the desktop application: its assets are local
// files served from the same process, so an offline cache buys nothing and
// costs a great deal — a precache-first worker pins the window to whichever
// build it first saw, and the "update ready" banner covers the composer of an
// application that has no business updating itself at all.
//
// Registering with selfDestroying unregisters any worker from earlier builds
// and clears its caches, so a stale window heals on the next load. A browser
// tab pointed at the same server simply reloads from the network, which is what
// it did before the worker existed.
import { shallowRef } from 'vue';

/// Kept for callers that still render an update affordance; never set now.
export const needRefresh = shallowRef(false);

export async function initPWA() {
  if (!('serviceWorker' in navigator) || !location.protocol.startsWith('http')) return;
  try {
    const mod = await import('virtual:pwa-register');
    await mod.registerSW({ immediate: true });
    // Drop any worker the previous build left behind, along with its caches.
    const registrations = await navigator.serviceWorker.getRegistrations();
    await Promise.all(registrations.map(registration => registration.unregister()));
    if ('caches' in window) {
      const keys = await caches.keys();
      await Promise.all(keys.filter(key => key.startsWith('workbox')).map(key => caches.delete(key)));
    }
  } catch (err) {
    console.warn('[pwa] worker retirement failed:', err);
  }
}

export function refreshApp() {
  location.reload();
}
