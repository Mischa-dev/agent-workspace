import { expect, test, type Page } from '@playwright/test'

import { seededValue, storageStatePath } from './environment'

// W1.31, reading offline: a person turns it on for themselves, opens a page
// and a channel, loses the connection and opens the app again, and both
// still read. The connection is "lost" by refusing every API call and saying
// the browser is offline; the dev server still serves the app itself, as the
// service worker does for an installed app.

test.use({ storageState: storageStatePath })

async function writer(page: Page) {
  const csrf = (await page.context().cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  return (key: string) => ({ 'x-csrf-token': csrf, 'idempotency-key': key, origin: new URL(page.url()).origin })
}

/** Whether this device's offline copy holds `text` yet (it's written shortly after answers arrive). */
const kept = (page: Page, text: string) =>
  page.evaluate(
    (text) =>
      new Promise<boolean>((resolve) => {
        const opening = indexedDB.open('agent-workspace-offline', 1)
        opening.onerror = () => resolve(false)
        opening.onsuccess = () => {
          const db = opening.result
          if (!db.objectStoreNames.contains('copies')) return resolve(false)
          const all = db.transaction('copies').objectStore('copies').getAll()
          all.onsuccess = () => resolve(JSON.stringify(all.result).includes(text))
          all.onerror = () => resolve(false)
        }
      }),
    text,
  )

test('a page and a channel opened earlier still read with no connection', async ({ page }) => {
  const projectId = seededValue('E2E_PROJECT_ID')
  const channelId = seededValue('E2E_CHANNEL_ID')
  const stamp = Date.now() % 100000
  const pageText = `Kept for the train ${stamp}`
  const messageText = `Read this on the plane ${stamp}`

  await page.goto('/settings/account/security')
  const toggle = page.getByRole('switch', { name: 'Read offline' })
  if (!(await toggle.isChecked())) await toggle.click()
  await expect(toggle).toBeChecked()

  const headers = await writer(page)
  const created = await page.request.post(`/api/v1/projects/${projectId}/pages`, {
    headers: headers(`offline-page-${stamp}`),
    data: { title: `Offline ${stamp}`, body: pageText },
  })
  expect(created.ok(), await created.text()).toBe(true)
  const { id: pageId } = (await created.json()) as { id: string }
  const sent = await page.request.post('/api/v2/ops/message_send', {
    headers: headers(`offline-message-${stamp}`),
    data: { conversationId: channelId, text: messageText },
  })
  expect(sent.ok(), await sent.text()).toBe(true)

  const pageHref = `/projects/${projectId}/pages/${pageId}`
  await page.goto(pageHref)
  await expect(page.getByText(pageText).first()).toBeVisible()
  await expect.poll(() => kept(page, pageText), { timeout: 15_000 }).toBe(true)
  await page.goto(`/projects/${projectId}/channels/${channelId}`)
  await expect(page.getByText(messageText).first()).toBeVisible()
  await expect.poll(() => kept(page, messageText), { timeout: 15_000 }).toBe(true)
  // Opening the app again kept what was kept before, not just this page.
  await expect.poll(() => kept(page, pageText), { timeout: 15_000 }).toBe(true)

  // The connection goes.
  await page.context().route((url) => url.pathname.startsWith('/api/'), (route) => route.abort('internetdisconnected'))
  await page.context().addInitScript(() => {
    Object.defineProperty(Navigator.prototype, 'onLine', { configurable: true, get: () => false })
  })

  await page.goto(pageHref)
  await expect(page.getByText(pageText).first()).toBeVisible()
  await expect(page.getByText('You’re offline,')).toBeVisible()
  await page.goto(`/projects/${projectId}/channels/${channelId}`)
  await expect(page.getByText(messageText).first()).toBeVisible()
})

test('turning it off forgets what this device kept', async ({ page }) => {
  await page.goto('/settings/account/security')
  const toggle = page.getByRole('switch', { name: 'Read offline' })
  if (!(await toggle.isChecked())) await toggle.click()
  await expect(toggle).toBeChecked()
  await expect.poll(() => page.evaluate(() => localStorage.getItem('agent-workspace:offline-reading'))).not.toBeNull()
  await toggle.click()
  await expect(toggle).not.toBeChecked()
  await expect.poll(() => page.evaluate(() => localStorage.getItem('agent-workspace:offline-reading'))).toBeNull()
})
