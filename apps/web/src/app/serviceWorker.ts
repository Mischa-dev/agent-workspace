/**
 * Registers public/sw.js, which makes the app installable and lets it open
 * offline from the last app shell it loaded. Production builds only: in
 * development Vite serves unhashed modules that the worker would never cache.
 */
export function registerServiceWorker() {
  if (!import.meta.env.PROD || !('serviceWorker' in navigator)) return
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js').catch(() => {
      // The app works the same without the worker; it only loses offline start and install.
    })
  })
}
