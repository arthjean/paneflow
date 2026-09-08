self.addEventListener("install", (event) => event.waitUntil(self.skipWaiting()));
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));
self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (url.pathname === "/service-worker-probe") {
    event.respondWith(new Response("served-by-service-worker", { headers: { "Content-Type": "text/plain" } }));
  }
});
