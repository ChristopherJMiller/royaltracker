// Minimal service worker. Required so the page is installable to the iOS Home
// Screen (a precondition for Web Push later). No offline caching yet.
self.addEventListener('install', function () { self.skipWaiting(); });
self.addEventListener('activate', function (e) { e.waitUntil(self.clients.claim()); });

// Placeholder push handler — activated once VAPID web-push delivery ships.
self.addEventListener('push', function (event) {
  var data = {};
  try { data = event.data ? event.data.json() : {}; } catch (e) {}
  var title = data.title || 'Price drop';
  var body = data.body || 'A tracked price changed.';
  event.waitUntil(self.registration.showNotification(title, { body: body }));
});
