import { defineConfig, devices } from '@playwright/test'

import { outputRoot, ports, serverOrigin, webOrigin } from './e2e/environment'

// Browser smoke tests: a few real flows against a throwaway server. See e2e/README.md.
export default defineConfig({
  testDir: './e2e',
  // The UI tour has its own config and seed (playwright.tour.config.ts).
  testIgnore: ['tour/**'],
  outputDir: `${outputRoot}/test-results`,
  globalSetup: './e2e/global-setup.ts',
  // One seeded User and Project shared by every spec, so run them in order.
  workers: 1,
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  // A flaky pass is a failure (P0.3): no retries, here or in CI.
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [['list'], ['html', { outputFolder: `${outputRoot}/report`, open: 'never' }]],
  use: {
    baseURL: webOrigin,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    launchOptions: {
      // For machines whose Chromium isn't Playwright's own build (NixOS, a
      // preinstalled browser): PLAYWRIGHT_CHROMIUM_EXECUTABLE=/path/to/chrome.
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
    },
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    // Vite's dev server, proxying /api to the server the global setup starts.
    command: `node ./node_modules/vite/bin/vite.js --host 127.0.0.1 --port ${ports.web} --strictPort`,
    url: webOrigin,
    env: { AGENT_WORKSPACE_API_PROXY_TARGET: serverOrigin },
    reuseExistingServer: false,
    timeout: 120_000,
  },
})
