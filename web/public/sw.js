// Service Worker
self.addEventListener('install', (event) => {
  console.log('Service Worker: Installed');
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  console.log('Service Worker: Activated');
  event.waitUntil(self.clients.claim());
});

self.addEventListener('fetch', (event) => {
  // Skip SSE requests - they don't work through service workers
  if (event.request.url.includes('/sse')) {
    return;
  }
  // Pass through - no caching yet
  event.respondWith(fetch(event.request));
});
