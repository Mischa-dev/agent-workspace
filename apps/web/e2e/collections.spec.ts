import path from 'node:path'

import { expect, test, type BrowserContext, type Page } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath } from './environment'

// Collections (AWD-90): a project's own kinds of records. One is made through
// the API, then used the way a person would: a value changed in the table, a
// record added from its last row, a filter saved into the view, and a record
// opened beside the table with its history. A page then shows the collection
// live, as an embed an agent wrote and one added from the editor's toolbar.
// Then the list, the table, a record and the page at a desktop and a phone
// width, in light and dark; the PNGs land in e2e-output/screenshots, and no
// page may scroll sideways.

test.use({ storageState: storageStatePath })

const NAME = 'Playtest feedback'

async function writer(context: BrowserContext, page: Page) {
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const headers = (key: string) => ({ 'x-csrf-token': csrf, 'idempotency-key': key, origin: new URL(page.url()).origin })
  return { request: page.request, headers }
}

/** The collection, with its fields and three records, made once however many tests ask. */
async function seed(context: BrowserContext, page: Page) {
  const projectId = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${projectId}/collections`)
  const { request, headers } = await writer(context, page)
  const listed = (await (await request.get(`/api/v1/projects/${projectId}/collections`)).json()) as { collections: Array<{ id: string; name: string }> }
  const found = listed.collections.find((collection) => collection.name === NAME)
  if (found) return { projectId, collectionId: found.id }
  const created = await request.post(`/api/v1/projects/${projectId}/collections`, {
    headers: headers('collection-playtest'),
    data: { name: NAME, description: 'What players said after each build.', titleName: 'Note' },
  })
  expect(created.ok(), await created.text()).toBe(true)
  const collectionId = ((await created.json()) as { collection: { id: string } }).collection.id
  const base = `/api/v1/projects/${projectId}/collections/${collectionId}`
  const fields = [
    { name: 'Severity', type: 'single_choice', options: [{ name: 'Low', color: 'green' }, { name: 'Medium', color: 'yellow' }, { name: 'High', color: 'red' }] },
    { name: 'Build', type: 'text' },
    { name: 'Score', type: 'number' },
    { name: 'Owner', type: 'person' },
    { name: 'Fixed', type: 'checkbox' },
    { name: 'Seen on', type: 'date' },
    { name: 'Clip', type: 'url' },
  ]
  for (const field of fields) {
    const made = await request.post(`${base}/fields`, { headers: headers(`collection-field-${field.name.replace(/[^A-Za-z0-9]+/g, '-')}`), data: field })
    expect(made.ok(), await made.text()).toBe(true)
  }
  const records = [
    { title: 'Jump feels floaty on the second level', values: { Severity: 'High', Build: '0.4.2', Score: 3, Owner: 'me', 'Seen on': '2026-11-02' } },
    { title: 'Menu music loops too soon', values: { Severity: 'Low', Build: '0.4.2', Score: 7, Fixed: true } },
    { title: 'Boss fight: unclear where to hit', values: { Severity: 'Medium', Build: '0.4.3', Score: 5, Clip: 'https://example.com/clips/boss-fight' } },
  ]
  for (const [index, record] of records.entries()) {
    const made = await request.post(`${base}/records`, { headers: headers(`collection-record-${index}`), data: record })
    expect(made.ok(), await made.text()).toBe(true)
  }
  return { projectId, collectionId }
}

const NOTES = 'Playtest notes'

/** A page that shows the collection as an embed, the way an agent writes one; made once. */
async function notesPage(context: BrowserContext, page: Page, projectId: string, collectionId: string) {
  const { request, headers } = await writer(context, page)
  const listed = (await (await request.get(`/api/v1/projects/${projectId}/pages`)).json()) as { pages: Array<{ id: string; title: string }> }
  const found = listed.pages.find((entry) => entry.title === NOTES)
  if (found) return found.id
  const created = await request.post(`/api/v1/projects/${projectId}/pages`, {
    headers: headers('page-playtest-notes'),
    data: { title: NOTES, body: `What players said after 0.4.2:\n\n[${NAME}](/projects/${projectId}/collections/${collectionId} "embed")\n\nWe fix the high ones first.\n` },
  })
  expect(created.ok(), await created.text()).toBe(true)
  return ((await created.json()) as { id: string }).id
}

test('a collection edited in place, filtered into its view, and a record with its history', async ({ page, context }) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.setViewportSize({ width: 1440, height: 900 })
  const { projectId, collectionId } = await seed(context, page)

  // The sidebar leads to the list, and the list to the collection.
  await page.goto(`/projects/${projectId}`)
  await page.getByRole('link', { name: 'Collections' }).first().click()
  await expect(page.getByRole('heading', { name: 'Collections' })).toBeVisible()
  await page.getByRole('link', { name: NAME }).click()
  await expect(page).toHaveURL(new RegExp(`/collections/${collectionId}`))
  const table = page.getByRole('grid', { name: NAME })
  await expect(table.getByText('Menu music loops too soon')).toBeVisible()

  // A value changes in place.
  await page.getByRole('button', { name: 'Severity of Menu music loops too soon: Low. Change' }).click()
  await page.getByRole('option', { name: 'High' }).click()
  await expect(page.getByRole('button', { name: 'Severity of Menu music loops too soon: High. Change' })).toBeVisible()

  // A record is added from the last row.
  await page.getByRole('textbox', { name: 'New record' }).fill('Save point too far from the boss')
  await page.getByRole('textbox', { name: 'New record' }).press('Enter')
  await expect(table.getByText('Save point too far from the boss')).toBeVisible()

  // A filter narrows the table, then is saved into the view.
  await page.getByRole('button', { name: 'Filter' }).click()
  await page.getByRole('menuitem', { name: 'Severity' }).click()
  await page.getByRole('checkbox', { name: 'High' }).check()
  await page.getByRole('button', { name: 'Apply' }).click()
  await expect(page.getByRole('button', { name: 'Severity is High', exact: true })).toBeVisible()
  await expect(table.getByText('Boss fight: unclear where to hit')).toBeHidden()
  await page.getByRole('button', { name: 'Save view' }).click()
  await expect(page.getByRole('button', { name: 'Save view' })).toBeHidden()
  await page.reload()
  await expect(page.getByRole('button', { name: 'Severity is High', exact: true })).toBeVisible()
  await expect(page.getByRole('grid', { name: NAME }).getByText('Boss fight: unclear where to hit')).toBeHidden()

  // A record opens beside the table, with what changed and who changed it.
  await page.getByRole('button', { name: 'Open Menu music loops too soon' }).click()
  const panel = page.getByRole('complementary', { name: 'Menu music loops too soon' })
  await expect(panel.getByRole('heading', { name: 'History' })).toBeVisible()
  await expect(panel.getByText('Low → High')).toBeVisible()
  await page.screenshot({ path: path.join(screenshotDir, 'collections-flow-desktop.png'), animations: 'disabled' })

  // Take the filter back off, for the screenshots after this.
  await page.getByRole('button', { name: 'Remove: Severity is High' }).click()
  await page.getByRole('button', { name: 'Save view' }).click()
  await expect(page.getByRole('button', { name: 'Save view' })).toBeHidden()
  expect(errors, 'uncaught errors on the page').toEqual([])
})

test('a page shows the collection live, and the editor adds another view of it', async ({ page, context }) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.setViewportSize({ width: 1440, height: 900 })
  const { projectId, collectionId } = await seed(context, page)
  const pageId = await notesPage(context, page, projectId, collectionId)
  const { request, headers } = await writer(context, page)

  await page.goto(`/projects/${projectId}/pages/${pageId}`)
  const doc = page.locator('.aw-live__doc')
  const embed = doc.locator('.aw-cembed').first()
  await expect(embed.getByRole('link', { name: NAME })).toBeVisible()
  await expect(embed.getByRole('link', { name: 'Menu music loops too soon' })).toBeVisible()
  await expect(doc).toContainText('We fix the high ones first.')

  // A record an agent adds shows up in the open page.
  const added = await request.post(`/api/v1/projects/${projectId}/collections/${collectionId}/records`, {
    headers: headers(`collection-record-live-${Date.now()}`),
    data: { title: `Checkpoint before the bridge ${Date.now()}`, values: { Severity: 'Medium' } },
  })
  expect(added.ok(), await added.text()).toBe(true)
  const title = ((await added.json()) as { record: { title: string } }).record.title
  await expect(embed.getByText(title)).toBeVisible()

  // The toolbar adds another view of it below the first paragraph.
  await doc.getByText('What players said after 0.4.2:').click()
  await page.keyboard.press('End')
  await page.getByRole('toolbar', { name: 'Formatting' }).getByRole('button', { name: 'Embed' }).click()
  await page.getByRole('form', { name: 'Embed in the page' }).getByRole('button', { name: 'Collection view…' }).click()
  const picker = page.getByRole('form', { name: 'Add a collection view' })
  await picker.getByLabel('Collection').selectOption({ label: NAME })
  await picker.getByRole('button', { name: 'Add to page' }).click()
  await expect(doc.locator('.aw-cembed')).toHaveCount(2)
  await expect(page.locator('.aw-live__save')).toHaveText('Saved')
  const saved = (await (await request.get(`/api/v1/pages/${pageId}`)).json()) as { body: string }
  expect(saved.body.match(/ "embed"\)/g)).toHaveLength(2)
  await page.screenshot({ path: path.join(screenshotDir, 'collections-embed-desktop.png'), animations: 'disabled' })

  // Put the page back as the agent wrote it, for the screenshots after this.
  const current = (await (await request.get(`/api/v1/pages/${pageId}`)).json()) as { version: number }
  const restored = await request.patch(`/api/v1/pages/${pageId}`, {
    headers: headers(`page-playtest-notes-restore-${Date.now()}`),
    data: {
      expectedVersion: current.version,
      body: `What players said after 0.4.2:\n\n[${NAME}](/projects/${projectId}/collections/${collectionId} "embed")\n\nWe fix the high ones first.\n`,
    },
  })
  expect(restored.ok(), await restored.text()).toBe(true)
  expect(errors, 'uncaught errors on the page').toEqual([])
})

const schemes = [
  { scheme: 'light', theme: 'daylight' },
  { scheme: 'dark', theme: 'midnight' },
] as const
const widths = [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'phone', width: 390, height: 844 },
] as const

for (const { scheme, theme } of schemes) {
  test.describe(`collections, ${scheme}`, () => {
    test.use({ colorScheme: scheme })

    test.beforeEach(async ({ page }) => {
      await page.addInitScript(() => {
        window.localStorage.setItem(
          'agent-workspace:appearance:v2',
          JSON.stringify({ theme: 'system', overrides: {}, density: 'comfortable', textSize: 'default' }),
        )
      })
    })

    for (const { name: size, width, height } of widths) {
      test(`list, table and record at ${width} px`, async ({ page, context }) => {
        const errors: string[] = []
        page.on('pageerror', (error) => errors.push(error.message))
        await page.setViewportSize({ width, height })
        const { projectId, collectionId } = await seed(context, page)
        const records = await (await page.request.get(`/api/v1/projects/${projectId}/collections/${collectionId}/records?search=floaty`)).json()
        const recordId = (records as { records: Array<{ id: string }> }).records[0].id
        const pageId = await notesPage(context, page, projectId, collectionId)
        const shots = [
          { name: 'list', path: `/projects/${projectId}/collections`, ready: () => expect(page.getByRole('link', { name: NAME })).toBeVisible() },
          {
            name: 'table',
            path: `/projects/${projectId}/collections/${collectionId}`,
            ready: () => expect(page.getByText('Jump feels floaty on the second level').first()).toBeVisible(),
          },
          {
            name: 'record',
            path: `/projects/${projectId}/collections/${collectionId}?record=${recordId}`,
            ready: () => expect(page.getByRole('heading', { name: 'History' })).toBeVisible(),
          },
          {
            name: 'page',
            path: `/projects/${projectId}/pages/${pageId}`,
            ready: () => expect(page.locator('.aw-cembed').getByRole('link', { name: 'Menu music loops too soon' })).toBeVisible(),
          },
        ]
        for (const shot of shots) {
          await page.goto(shot.path)
          await shot.ready()
          await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
          await page.screenshot({
            path: path.join(screenshotDir, `collections-${shot.name}-${scheme}-${size}.png`),
            fullPage: true,
            animations: 'disabled',
          })
          // The table scrolls sideways inside itself by design; the page never does.
          const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
          expect(overflow, `${shot.name}: horizontal scroll at ${width} px`).toBeLessThanOrEqual(0)
        }
        expect(errors, 'uncaught errors on the page').toEqual([])
      })
    }
  })
}
