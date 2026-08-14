/* The offline worker for the CMI Timetable Planner. CACHE and MANIFEST are
 * prepended at build time by app/hooks/gen-sw.sh.
 *
 * Rules:
 *   install  — precache the whole build; skipWaiting so a new deploy never
 *              sits in "waiting" behind an old worker.
 *   activate — delete this app's older caches ONLY (prefix cmitt-sw-; the
 *              github.io origin is shared with the user's other project
 *              pages, so deleting every cache would vandalise them), then
 *              claim open pages.
 *   fetch    — same-origin GETs only. Navigations: network-first with a 5 s
 *              cap, falling back to the cached shell. Everything else:
 *              cache-first by exact URL (asset names carry content hashes),
 *              plain pass-through on a miss, nothing cached at runtime.
 *              Cross-origin (cmi.ac.in, the CORS relays) is NEVER
 *              intercepted — we return before respondWith so the browser
 *              handles those requests as if no worker existed.
 */
'use strict';

const NAV_TIMEOUT_MS = 5000;

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      // cache:'reload' bypasses the HTTP cache, so the precache holds what
      // the server serves NOW, not a CDN-stale copy of it.
      .then((cache) =>
        cache.addAll(MANIFEST.map((url) => new Request(url, { cache: 'reload' })))
      )
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(
          names
            .filter((name) => name.startsWith('cmitt-sw-') && name !== CACHE)
            .map((name) => caches.delete(name))
        )
      )
      .then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;
  if (new URL(request.url).origin !== self.location.origin) return;

  if (request.mode === 'navigate') {
    event.respondWith(navigate(request));
    return;
  }

  // Cache-first with an exact-URL match: hashed assets always hit; anything
  // else — including the app's ?nw-probe=… network probe, whose unique
  // query can never match a cached URL — passes through to the network.
  event.respondWith(caches.match(request).then((hit) => hit || fetch(request)));
});

// Network-first, so an online reload always shows the newest deploy; the
// cached shell answers when the network fails or stalls past the cap. Any
// path under the scope falls back to index.html — the same bounce 404.html
// performs online — so share links with queries, deep links, and the e2e
// suite's /e2e-blank all work offline too.
async function navigate(request) {
  const shell =
    (await caches.match(request)) || (await caches.match('./index.html'));
  if (!shell) return fetch(request); // nothing cached yet: be transparent
  const fallback = new Promise((resolve) =>
    setTimeout(() => resolve(shell), NAV_TIMEOUT_MS)
  );
  // A server ERROR page must not beat a working offline copy: during a
  // Pages outage github.io answers fast with a 5xx page, which would win
  // the race and shadow the fully cached app. 4xx still passes through —
  // the online 404 bounce (404.html) is load-bearing for deep links.
  const network = fetch(request)
    .then((response) => (response.status >= 500 ? shell : response))
    .catch(() => shell);
  return Promise.race([network, fallback]);
}
