import react from '@vitejs/plugin-react'
import { configDefaults, defineConfig } from 'vitest/config'

// The e2e smoke tests (e2e/) point the proxy at their own throwaway server.
const apiTarget = process.env.AGENT_WORKSPACE_API_PROXY_TARGET ?? 'http://127.0.0.1:3100'

export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    proxy: {
      '/api': {
        target: apiTarget,
        ws: true,
      },
      '/health': apiTarget,
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
    // Playwright owns e2e/*.spec.ts; vitest would try to run them otherwise.
    exclude: [...configDefaults.exclude, 'e2e/**'],
    // The full-App tests take 4 to 5 seconds on an idle machine, so vitest's
    // 5 second default fails them whenever the CI host is busy.
    testTimeout: 20_000,
  },
})
