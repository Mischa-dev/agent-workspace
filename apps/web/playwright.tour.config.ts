import { defineConfig, devices } from '@playwright/test'

import baseConfig from './playwright.config'

// The UI tour (AWD-84): the same throwaway server as the smoke tests, seeded
// with a busy workspace (e2e/tour/seed.ts), then about 15 key screens at 1440
// and 390 px in light and dark. Run it with tools/ui-tour/run.sh; it isn't
// part of CI.
process.env.E2E_SEED = 'tour'

export default defineConfig({
  ...baseConfig,
  testDir: './e2e/tour',
  testIgnore: [],
  // A screenshot that fails shouldn't stop the rest of the tour.
  retries: 0,
  timeout: 90_000,
  reporter: [['list']],
  use: { ...baseConfig.use, ...devices['Desktop Chrome'], trace: 'off', screenshot: 'off' },
  projects: [{ name: 'tour', use: { ...devices['Desktop Chrome'] } }],
})
