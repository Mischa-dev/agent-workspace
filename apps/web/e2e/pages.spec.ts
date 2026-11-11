import path from 'node:path'

import { expect, test, type BrowserContext, type Page } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath } from './environment'

// A page written live (AWD-53). Two people type into one page at once while a
// third writer changes it through the API, as an agent does, and every change
// reaches every open copy and the page's Markdown. Opening a page for the first
// time changes nothing about it. The PNGs land in e2e-output/screenshots for a
// person to look at, light and dark and on a phone.

test.use({ storageState: storageStatePath })

const BODY = [
  '# Launch plan',
  '',
  'We ship on Friday.',
  '',
  '- [ ] Write the notes',
  '- [x] Tell support',
  '  - nested under it',
  '',
  '> Keep it short.',
  '',
  '| Who | What |',
  '| :-- | --: |',
  '| Ana | **Notes**, see SMK-1 |',
  '',
  '```js',
  "const day = 'Friday'",
  '```',
  '',
].join('\n')

type Saved = { body: string; version: number }

async function writer(context: BrowserContext, page: Page) {
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const headers = (key: string) => ({ 'x-csrf-token': csrf, 'idempotency-key': key, origin: new URL(page.url()).origin })
  return { request: page.request, headers }
}

/** A new page holding BODY, made the way an agent or an import makes one. */
async function newPage(context: BrowserContext, page: Page, title: string) {
  const projectId = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${projectId}/pages`)
  const { request, headers } = await writer(context, page)
  const created = await request.post(`/api/v1/projects/${projectId}/pages`, {
    headers: headers(`page-${title.replace(/[^A-Za-z0-9]+/g, '-')}`),
    data: { title, body: BODY },
  })
  expect(created.ok(), await created.text()).toBe(true)
  const { id } = (await created.json()) as { id: string }
  return { id, href: `/projects/${projectId}/pages/${id}` }
}

const editor = (page: Page) => page.locator('.aw-live__doc')

/** Puts the cursor at the end of the line holding `text`. */
async function endOf(page: Page, text: string) {
  await editor(page).getByText(text).first().click()
  await page.keyboard.press('End')
}

test('two people and an agent write one page at once', async ({ browser, context, page }) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  const { id, href } = await newPage(context, page, `Live page ${Date.now()}`)
  const { request, headers } = await writer(context, page)
  const read = async () => (await (await request.get(`/api/v1/pages/${id}`)).json()) as Saved

  const made = await read()
  await page.goto(href)
  await expect(editor(page)).toContainText('We ship on Friday.')
  await expect(page.getByRole('toolbar', { name: 'Formatting' })).toBeVisible()
  // Opening the page made its live document and changed nothing anyone sees.
  expect(await read()).toMatchObject({ body: made.body, version: made.version })

  const other = await browser.newContext({ storageState: storageStatePath })
  const second = await other.newPage()
  second.on('pageerror', (error) => errors.push(error.message))
  await second.goto(href)
  await expect(editor(second)).toContainText('Tell support')

  await endOf(page, 'We ship on Friday.')
  await endOf(second, 'Write the notes')
  await Promise.all([page.keyboard.type(' Or Saturday.'), second.keyboard.type(' today')])
  // Each sees the other's typing, as well as their own.
  await expect(editor(page)).toContainText('Write the notes today')
  await expect(editor(second)).toContainText('We ship on Friday. Or Saturday.')

  // Ticking a box is a change like any other.
  await second.getByRole('checkbox', { name: 'Not done' }).first().click()
  await expect(page.getByRole('checkbox', { name: 'Done' })).toHaveCount(2)
  await expect(page.locator('.aw-live__save')).toHaveText('Saved')
  await expect(second.locator('.aw-live__save')).toHaveText('Saved')

  // An agent's change, written as Markdown through the API, reaches both.
  const before = await read()
  const patched = await request.patch(`/api/v1/pages/${id}`, {
    headers: headers(`agent-${id}`),
    data: { body: `${before.body}\nAdded by an agent.\n`, expectedVersion: before.version },
  })
  expect(patched.ok(), await patched.text()).toBe(true)
  await expect(editor(page)).toContainText('Added by an agent.')
  await expect(editor(second)).toContainText('Added by an agent.')

  // The page's Markdown holds all of it, and the rest as it was.
  const after = await read()
  expect(after.body).toContain('We ship on Friday. Or Saturday.')
  expect(after.body).toContain('- [x] Write the notes today\n- [x] Tell support\n  - nested under it')
  expect(after.body).toContain('| Ana | **Notes**, see SMK-1 |')
  expect(after.body).toContain('Added by an agent.')
  expect(errors, 'uncaught errors on the page').toEqual([])
  await other.close()
})

for (const scheme of ['light', 'dark'] as const) {
  test(`a page opens live looking as it did, ${scheme}`, async ({ browser }) => {
    const context = await browser.newContext({ storageState: storageStatePath, colorScheme: scheme })
    await context.addInitScript(() => {
      window.localStorage.setItem(
        'agent-workspace:appearance:v2',
        JSON.stringify({ theme: 'system', overrides: {}, density: 'comfortable', textSize: 'default' }),
      )
    })
    const page = await context.newPage()
    const errors: string[] = []
    page.on('pageerror', (error) => errors.push(error.message))
    const { href } = await newPage(context, page, `Live page ${scheme} ${Date.now()}`)
    await page.goto(href)
    await expect(editor(page)).toContainText('Keep it short.')
    await expect(page.locator('.aw-live__save')).toHaveText('Saved')
    await page.screenshot({ path: path.join(screenshotDir, `live-page-${scheme}.png`), fullPage: true, animations: 'disabled' })

    // The link field, under the toolbar.
    await endOf(page, 'Keep it short.')
    await page.keyboard.press('ControlOrMeta+k')
    await expect(page.getByLabel('Link address')).toBeFocused()
    await page.screenshot({ path: path.join(screenshotDir, `live-page-link-${scheme}.png`), animations: 'disabled' })
    await page.keyboard.press('Escape')

    await page.setViewportSize({ width: 390, height: 844 })
    await page.screenshot({ path: path.join(screenshotDir, `live-page-${scheme}-phone.png`), fullPage: true, animations: 'disabled' })
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
    expect(overflow, 'horizontal scroll on a 390 px phone').toBeLessThanOrEqual(0)
    expect(errors, 'uncaught errors on the page').toEqual([])
    await context.close()
  })
}
