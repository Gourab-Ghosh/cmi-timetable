/* Debug/dev worker (`trunk serve`): caches NOTHING and has no fetch
 * handler, so no dev request is ever intercepted or served stale. It also
 * cleans up after any release worker previously installed on this origin:
 * caches deleted, registration removed. index.html re-registers it on each
 * load; the cycle is register → clean → unregister, and it never answers a
 * single fetch. No reload loop: nothing here navigates or reloads clients.
 */
'use strict';

self.addEventListener('install', () => self.skipWaiting());

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(
          names.filter((n) => n.startsWith('cmitt-sw-')).map((n) => caches.delete(n))
        )
      )
      .then(() => self.registration.unregister())
  );
});
