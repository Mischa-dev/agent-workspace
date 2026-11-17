import path from 'node:path'

import { expect, test, type BrowserContext, type Page } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath } from './environment'

// W1.40: the visual directions side by side for the owner's pick: a channel,
// the tasks list and a page, under today's look and each direction, at 1440
// and 390 px, light and dark. Only on request (E2E_DIRECTIONS=1), since it
// takes 48 screenshots and checks nothing; they land in
// e2e-output/screenshots/directions.

const DIRECTIONS = ['none', 'quiet', 'paper', 'frame'] as const
const SCHEMES = [
  { scheme: 'light', theme: 'daylight' },
  { scheme: 'dark', theme: 'midnight' },
] as const
const WIDTHS = [
  { name: 'wide', width: 1440, height: 900 },
  { name: 'phone', width: 390, height: 844 },
] as const

test.skip(!process.env.E2E_DIRECTIONS, 'Screenshots for the W1.40 pick: set E2E_DIRECTIONS=1')
test.use({ storageState: storageStatePath })

let pageId: string | null = null

/** One page with some body to read, made once. */
async function samplePage(context: BrowserContext, page: Page) {
  if (pageId) return pageId
  const projectId = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${projectId}/pages`)
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const created = await page.request.post(`/api/v1/projects/${projectId}/pages`, {
    headers: { 'x-csrf-token': csrf, 'idempotency-key': 'directions-sample-page', origin: new URL(page.url()).origin },
    data: {
      title: 'Launch plan',
      body: [
        'What ships in the first week, who owns each part, and what we watch once it is out.',
        '',
        '## Goals',
        '',
        '- Every team can start a project from a template in under a minute.',
        '- Agents pick up ready tasks without being asked.',
        '- Nothing on the home page needs explaining.',
        '',
        '## Open questions',
        '',
        '1. Do we keep the old export for a release?',
        '2. Who answers agents on the weekend?',
      ].join('\n'),
    },
  })
  expect(created.ok(), await created.text()).toBe(true)
  pageId = ((await created.json()) as { id?: string; page?: { id: string } }).page?.id ?? ((await created.json()) as { id: string }).id
  return pageId
}

const screens = [
  {
    name: 'channel',
    path: async () => `/projects/${seededValue('E2E_PROJECT_ID')}/channels/${seededValue('E2E_CHANNEL_ID')}`,
    ready: (page: Page) => expect(page.getByText('Hello from the smoke tests.')).toBeVisible(),
  },
  {
    name: 'tasks',
    path: async () => `/projects/${seededValue('E2E_PROJECT_ID')}/work-items`,
    ready: (page: Page) => expect(page.getByText('Seeded work item').first()).toBeVisible(),
  },
  {
    name: 'page',
    path: async (context: BrowserContext, page: Page) => `/projects/${seededValue('E2E_PROJECT_ID')}/pages/${await samplePage(context, page)}`,
    ready: (page: Page) => expect(page.getByText('What ships in the first week').first()).toBeVisible(),
  },
]

for (const { scheme, theme } of SCHEMES) {
  test.describe(`${scheme}`, () => {
    test.use({ colorScheme: scheme })
    test.beforeEach(async ({ page }) => {
      await page.addInitScript(() => {
        window.localStorage.setItem(
          'agent-workspace:appearance:v2',
          JSON.stringify({ theme: 'system', overrides: {}, density: 'comfortable', textSize: 'default' }),
        )
      })
    })
    for (const direction of DIRECTIONS) {
      for (const screen of screens) {
        test(`${direction} ${screen.name}`, async ({ context, page }) => {
          for (const size of WIDTHS) {
            await page.setViewportSize({ width: size.width, height: size.height })
            const target = await screen.path(context, page)
            await page.goto(`${target}${target.includes('?') ? '&' : '?'}direction=${direction}`)
            await screen.ready(page)
            // Everything on the screen loaded, so the directions compare like with like.
            await page.waitForLoadState('networkidle')
            await expect(page.locator('[aria-busy="true"]')).toHaveCount(0)
            await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
            if (direction === 'none') await expect(page.locator('html')).not.toHaveAttribute('data-direction', /.+/)
            else await expect(page.locator('html')).toHaveAttribute('data-direction', direction)
            await page.screenshot({
              path: path.join(screenshotDir, 'directions', `${screen.name}-${direction}-${scheme}-${size.name}.png`),
              animations: 'disabled',
            })
          }
        })
      }
    }
  })
}
