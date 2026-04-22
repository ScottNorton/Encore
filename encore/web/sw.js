// Service Worker — cache-first for static assets, bypass for API/WebSocket.
const CACHE_VERSION = '1772283029';
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

// Activate: purge old caches, claim clients immediately
self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k))
      )
    ).then(() => self.clients.claim())
  );
});

// Fetch: cache-first for static, network-only for API/WS
self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);

  // Bypass cache for API, WebSocket upgrade, and non-GET
  if (
    url.pathname.startsWith('/api/') ||
    url.pathname === '/ws' ||
    event.request.method !== 'GET'
  ) {
    return;
  }

  event.respondWith(
    caches.match(event.request).then((cached) => {
      if (cached) return cached;
      return fetch(event.request).then((response) => {
        // Cache successful responses for known static paths
        if (response.ok && STATIC_ASSETS.some((a) => url.pathname === a || url.pathname.startsWith('/pkg/') || url.pathname.startsWith('/branding/'))) {
          const clone = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(event.request, clone));
        }
        return response;
      });
    })
  );
});
