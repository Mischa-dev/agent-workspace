// Excalidraw loads its fonts from this path. They're copied into the build
// (see vite.config.ts) so the app's strict font-src 'self' holds; the default
// is a public CDN the policy blocks.
declare global {
  interface Window {
    EXCALIDRAW_ASSET_PATH?: string
  }
}

window.EXCALIDRAW_ASSET_PATH = '/excalidraw-assets/'

export {}
