import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'

import { expect, test, type Page } from '@playwright/test'

import { outputRoot, storageStatePath } from '../environment'
import { tourStatePath, type TourState } from './state'

// The UI tour (AWD-84): the key screens of the app at 1440 px (the main
// score) and 390 px, in light and dark, against the busy workspace that
// seed.ts makes. Each screen saves what fits in the window, the way a person
// first sees it. Run it with tools/ui-tour/run.sh; the PNGs and a contact
// sheet go to e2e-output/tour.

const tourDir = path.join(outputRoot, 'tour')
const state = (): TourState => JSON.parse(readFileSync(tourStatePath, 'utf8')) as TourState

type Screen = {
  name: string
  title: string
  path: (s: TourState) => string
  /** Waits until the screen shows its content (not a skeleton). */
  ready: (page: Page, s: TourState) => Promise<void>
  /** Anything to do once it's ready, such as opening the command palette. */
  then?: (page: Page, s: TourState) => Promise<void>
}

const project = (s: TourState) => `/projects/${s.projectId}`

const screens: Screen[] = [
  {
    name: '01-home',
    title: 'Home',
    path: () => '/home',
    ready: (page) => expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible(),
  },
  {
    name: '02-channel',
    title: 'A channel',
    path: (s) => `${project(s)}/channels/${s.channelId}`,
    ready: (page, s) => expect(page.getByText(s.channelLastMessage).first()).toBeVisible(),
  },
  {
    name: '03-dm',
    title: 'A DM',
    path: (s) => `/messages/${s.dmId}`,
    ready: (page, s) => expect(page.getByText(s.dmLastMessage).first()).toBeVisible(),
  },
  {
    name: '04-thread',
    title: 'A thread',
    path: (s) => `${project(s)}/threads/${s.threadId}`,
    ready: (page, s) => expect(page.getByText(s.threadLastMessage).first()).toBeVisible(),
  },
  {
    name: '05-task-list',
    title: 'The task list',
    // The list or board choice sticks, so ask for the list by name.
    path: (s) => `${project(s)}/work-items?view=list`,
    ready: (page) => expect(page.getByRole('heading', { name: 'Tasks', exact: true })).toBeVisible(),
  },
  {
    name: '06-board',
    title: 'The board',
    path: (s) => `${project(s)}/work-items?view=board`,
    ready: (page) => expect(page.getByRole('region', { name: 'Task board' })).toBeVisible(),
  },
  {
    name: '07-task',
    title: 'A task',
    path: (s) => `${project(s)}/work-items/${s.taskId}`,
    ready: (page, s) => expect(page.getByText(s.taskTitle).first()).toBeVisible(),
  },
  {
    name: '08-run',
    title: 'A run',
    path: (s) => (s.runId ? `/runs/${s.runId}` : '/runs'),
    ready: (page) => expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible(),
  },
  {
    name: '09-inbox',
    title: 'The inbox',
    path: () => '/inbox',
    ready: (page) => expect(page.getByRole('heading', { name: 'Inbox', exact: true })).toBeVisible(),
  },
  {
    name: '10-search',
    title: 'Search',
    path: () => '/search?q=partition',
    ready: (page) => expect(page.getByRole('heading', { name: 'Search', exact: true })).toBeVisible(),
  },
  {
    name: '11-settings',
    title: 'Settings (workspace members)',
    path: () => '/settings/workspace/members',
    ready: (page) => expect(page.getByRole('heading', { name: 'Members', exact: true }).first()).toBeVisible(),
  },
  {
    name: '12-page',
    title: 'A page',
    path: (s) => `${project(s)}/pages/${s.pageId}`,
    ready: (page, s) => expect(page.getByText(s.pageTitle).first()).toBeVisible(),
  },
  {
    name: '13-whiteboard',
    title: 'A whiteboard',
    path: (s) => `/whiteboards/${s.whiteboardId}`,
    ready: (page) => expect(page.locator('canvas.excalidraw__canvas.interactive')).toBeVisible(),
  },
  {
    name: '14-command-palette',
    title: 'The command palette',
    path: (s) => `${project(s)}/channels/${s.channelId}`,
    ready: (page, s) => expect(page.getByText(s.channelLastMessage).first()).toBeVisible(),
    then: async (page) => {
      await page.keyboard.press('ControlOrMeta+k')
      await expect(page.getByRole('dialog').first()).toBeVisible()
      await page.keyboard.type('batt')
      // The task search settles after a short pause.
      await page.waitForTimeout(800)
    },
  },
  {
    name: '15-onboarding',
    title: 'Onboarding',
    path: () => '/welcome',
    ready: (page) => expect(page.getByRole('navigation', { name: 'Setup steps' })).toBeVisible(),
  },
]

const only = (process.env.UI_TOUR_ONLY ?? '').split(',').map((name) => name.trim()).filter(Boolean)
const chosen = only.length ? screens.filter((screen) => only.some((name) => screen.name.includes(name))) : screens

const sizes = [
  { size: 'desktop', width: 1440, height: 900 },
  { size: 'phone', width: 390, height: 844 },
] as const
const schemes = ['light', 'dark'] as const

const file = (screen: Screen, size: string, scheme: string) => `${screen.name}-${size}-${scheme}.png`

test.use({ storageState: storageStatePath })

test.beforeAll(() => mkdirSync(tourDir, { recursive: true }))

for (const scheme of schemes) {
  test.describe(scheme, () => {
    test.use({ colorScheme: scheme })

    test.beforeEach(async ({ page }) => {
      await page.addInitScript(() => {
        window.localStorage.setItem(
          'agent-workspace:appearance:v2',
          JSON.stringify({ theme: 'system', overrides: {}, density: 'comfortable', textSize: 'default' }),
        )
      })
    })

    for (const screen of chosen) {
      for (const { size, width, height } of sizes) {
        test(`${screen.name} ${size}`, async ({ page }) => {
          const s = state()
          await page.setViewportSize({ width, height })
          await page.goto(screen.path(s))
          await screen.ready(page, s)
          await screen.then?.(page, s)
          // Let lazy images, avatars and fonts settle.
          await page.waitForLoadState('networkidle').catch(() => undefined)
          await page.evaluate(() => document.fonts.ready)
          await page.screenshot({ path: path.join(tourDir, file(screen, size, scheme)), animations: 'disabled' })
        })
      }
    }
  })
}

// A contact sheet: every screen in a row, desktop light and dark, then the phones.
test.afterAll(() => {
  const rows = chosen
    .map(
      (screen) => `<section><h2>${screen.title} <small>${screen.name}</small></h2><div class="row">
${schemes.map((scheme) => `<a href="${file(screen, 'desktop', scheme)}"><img class="desktop" src="${file(screen, 'desktop', scheme)}" alt="${screen.title}, desktop, ${scheme}"></a>`).join('\n')}
${schemes.map((scheme) => `<a href="${file(screen, 'phone', scheme)}"><img class="phone" src="${file(screen, 'phone', scheme)}" alt="${screen.title}, phone, ${scheme}"></a>`).join('\n')}
</div></section>`,
    )
    .join('\n')
  writeFileSync(
    path.join(tourDir, 'index.html'),
    `<!doctype html><meta charset="utf-8"><title>UI tour</title>
<style>body{font:14px system-ui;margin:24px;background:#888}h2{font-size:16px;margin:24px 0 8px}small{opacity:.6;font-weight:400}
.row{display:flex;gap:12px;align-items:flex-start}img{border:1px solid #0003;display:block}.desktop{width:560px}.phone{width:180px}</style>
<h1>UI tour</h1>${rows}`,
  )
})
