// Service Worker — network-first for the app shell (so a new firmware build is
// picked up without a manual cache clear), cache-first for immutable branding.
const CACHE_VERSION = '2';
const CACHE_NAME = `encore-v${CACHE_VERSION}`;

const STATIC_ASSETS = [
  '/',
  '/index.html',
  '/manifest.json',
  '/pkg/encore_wasm.js',
  '/pkg/encore_wasm_bg.wasm',
  '/branding/favicon-dark.png',
  '/branding/favicon-light.png',
  '/branding/logo-mark-dark.png',
  '/branding/logo-mark-light.png',
  '/branding/logo-wordmark-dark.png',
  '/branding/logo-wordmark-light.png',
  '/branding/pwa-icon-192-dark.png',
  '/branding/pwa-icon-512-dark.png',
];

// Install: pre-cache static assets (best-effort — never block activation)
self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) =>
      Promise.allSettled(
        STATIC_ASSETS.map((url) =>
          cache.add(url).catch(() => {})
        )
      )
    )
  );
  self.skipWaiting();
});

// Activate: purge old caches (bumping CACHE_VERSION clears stale builds),
// claim clients immediately.
self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k))
      )
    ).then(() => self.clients.claim())
  );
});

// The app shell: served fresh whenever the device is reachable, so an OTA
// update shows up on the next load. The shell file names are fixed (no content
// hash), so cache-first would otherwise pin the old build indefinitely.
function isShell(pathname) {
  return pathname === '/' || pathname === '/index.html' || pathname.startsWith('/pkg/');
}

// Cacheable static asset (branding, manifest) — content rarely changes.
function isCacheableStatic(pathname) {
  return pathname.startsWith('/branding/') || pathname === '/manifest.json';
}

self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);

  // Bypass cache for API, WebSocket upgrade, and non-GET.
  if (
    url.pathname.startsWith('/api/') ||
    url.pathname === '/ws' ||
    event.request.method !== 'GET'
  ) {
    return;
  }

  // App shell — network-first, cache as offline fallback.
  // `cache: 'reload'` bypasses the *browser HTTP cache* so a new firmware
  // build is always pulled fresh — otherwise the shell (fixed file names, no
  // content hash) can be served stale by the HTTP cache even though the SW is
  // network-first, which hid dashboard updates until a manual hard reload.
  if (isShell(url.pathname)) {
    event.respondWith(
      fetch(event.request, { cache: 'reload' })
        .then((response) => {
          if (response.ok) {
            const clone = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(event.request, clone));
          }
          return response;
        })
        .catch(() => caches.match(event.request))
    );
    return;
  }

  // Branding/manifest — cache-first for speed.
  event.respondWith(
    caches.match(event.request).then((cached) => {
      if (cached) return cached;
      return fetch(event.request).then((response) => {
        if (response.ok && isCacheableStatic(url.pathname)) {
          const clone = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(event.request, clone));
        }
        return response;
      });
    })
  );
});
