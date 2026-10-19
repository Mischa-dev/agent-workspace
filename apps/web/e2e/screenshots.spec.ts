import path from 'node:path'

import { expect, test, type Page } from '@playwright/test'

import { screenshotDir, seeded, seededValue, storageStatePath } from './environment'

// Screenshots of key pages in light and dark, saved as run artifacts in
// e2e-output/screenshots for a person to look at. This is a smoke check that
// each page renders in both schemes, not visual regression: nothing is
// compared against committed images.

type KeyPage = { name: string; path: () => string; ready: (page: Page) => Promise<void> }

const keyPages: KeyPage[] = [
  {
    name: 'home',
    path: () => '/home',
    ready: (page) => expect(page.getByRole('heading', { name: new RegExp(`^(Welcome|Good \\w+), ${seeded.displayName.split(' ')[0]}$`) })).toBeVisible(),
  },
  {
    name: 'welcome',
    path: () => '/welcome',
    ready: (page) => expect(page.getByRole('navigation', { name: 'Setup steps' })).toBeVisible(),
  },
  // Before the Project's own pages: opening a Project returns to the last page
  // seen in it, and the smoke test expects that to be one with the sidebar.
  {
    name: 'automation-rules',
    path: () => `/settings/projects/${seededValue('E2E_PROJECT_ID')}/automations`,
    ready: (page) => expect(page.getByRole('heading', { name: 'No rules yet' })).toBeVisible(),
  },
  {
    name: 'channel',
    path: () => `/projects/${seededValue('E2E_PROJECT_ID')}/channels/${seededValue('E2E_CHANNEL_ID')}`,
    ready: (page) => expect(page.getByText('Hello from the smoke tests.')).toBeVisible(),
  },
  {
    name: 'work-items-list',
    path: () => `/projects/${seededValue('E2E_PROJECT_ID')}/work-items`,
    ready: (page) => expect(page.getByRole('heading', { name: 'Tasks', exact: true })).toBeVisible(),
  },
  {
    name: 'work-items-board',
    path: () => `/projects/${seededValue('E2E_PROJECT_ID')}/work-items?view=board`,
    ready: (page) => expect(page.getByRole('region', { name: 'Task board' })).toBeVisible(),
  },
  {
    name: 'later',
    path: () => '/later',
    ready: (page) => expect(page.getByRole('heading', { name: 'Later', exact: true })).toBeVisible(),
  },
  {
    name: 'runs',
    path: () => '/runs',
    ready: (page) => expect(page.getByRole('heading', { name: 'Runs', exact: true })).toBeVisible(),
  },
  {
    name: 'whiteboards',
    path: () => '/whiteboards',
    ready: (page) => expect(page.getByRole('heading', { name: 'Whiteboards', exact: true })).toBeVisible(),
  },
  {
    name: 'project-agents',
    path: () => `/settings/projects/${seededValue('E2E_PROJECT_ID')}/agents`,
    ready: (page) => expect(page.getByRole('heading', { name: 'Project agents' })).toBeVisible(),
  },
  {
    name: 'harnesses',
    path: () => '/settings/workspace/harnesses',
    ready: (page) => expect(page.getByRole('heading', { name: 'Harnesses', exact: true })).toBeVisible(),
  },
  {
    name: 'workspace-runs',
    path: () => '/settings/workspace/runs',
    ready: (page) => expect(page.getByText(/to pick from/).first()).toBeVisible(),
  },
  {
    name: 'project-runs',
    path: () => `/settings/projects/${seededValue('E2E_PROJECT_ID')}/runs`,
    ready: (page) => expect(page.getByRole('textbox', { name: 'Prompt template' })).toBeVisible(),
  },
  {
    name: 'webhooks',
    path: () => '/settings/account/webhooks',
    ready: (page) => expect(page.getByRole('heading', { name: 'No webhooks yet' })).toBeVisible(),
  },
  {
    name: 'api-reference',
    path: () => '/settings/account/api-reference',
    ready: (page) => expect(page.getByText('task_list', { exact: true })).toBeVisible(),
  },
  {
    name: 'new-task',
    path: () => '/new-task',
    ready: (page) => expect(page.getByRole('heading', { name: 'New task' })).toBeVisible(),
  },
  {
    name: 'share',
    path: () => '/share?title=Flaky%20deploy&text=Fails%20on%20the%20second%20try%20https%3A%2F%2Fci.example%2Frun%2F1',
    ready: (page) => expect(page.getByRole('heading', { name: 'Share', exact: true })).toBeVisible(),
  },
  {
    name: 'appearance',
    path: () => '/settings/appearance',
    ready: (page) => expect(page.getByRole('heading', { name: 'Appearance' })).toBeVisible(),
  },
]

// The theme each scheme should resolve to when the person picks "Match system".
const schemes = [
  { scheme: 'light', theme: 'indigo' },
  { scheme: 'dark', theme: 'midnight' },
] as const

test.use({ storageState: storageStatePath })

for (const { scheme, theme } of schemes) {
  test.describe(`${scheme} scheme`, () => {
    test.use({ colorScheme: scheme })

    test.beforeEach(async ({ page }) => {
      await page.addInitScript(() => {
        window.localStorage.setItem(
          'agent-workspace:appearance:v2',
          JSON.stringify({ theme: 'system', overrides: {}, density: 'comfortable', textSize: 'default' }),
        )
      })
    })

    for (const keyPage of keyPages) {
      test(keyPage.name, async ({ page }) => {
        const errors: string[] = []
        page.on('pageerror', (error) => errors.push(error.message))

        await page.goto(keyPage.path())
        await keyPage.ready(page)
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
        await page.screenshot({
          path: path.join(screenshotDir, `${keyPage.name}-${scheme}.png`),
          fullPage: true,
          animations: 'disabled',
        })
        expect(errors, 'uncaught errors on the page').toEqual([])
      })

      test(`${keyPage.name} on a phone`, async ({ page }) => {
        await page.setViewportSize({ width: 390, height: 844 })
        await page.goto(keyPage.path())
        await keyPage.ready(page)
        await page.screenshot({
          path: path.join(screenshotDir, `${keyPage.name}-${scheme}-phone.png`),
          fullPage: true,
          animations: 'disabled',
        })
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
        expect(overflow, 'horizontal scroll on a 390 px phone').toBeLessThanOrEqual(0)
      })
    }
  })
}
