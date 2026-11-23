import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { storageStatePath } from '../environment'

// Test for "done", item 1: name a workspace and invite a teammate. Naming
// the first workspace happens on the first-run page (`/setup`), which a
// server shows only until its first account exists; the integration test
// `first_run.rs` walks it. Here the admin invites someone from Members, and
// they join with a new account from the link alone.

test('invite a teammate from Members: they join from the link and land in the workspace', async ({ browser, page }, info) => {
  const context = await browser.newContext({ storageState: storageStatePath, ...info.project.use })
  const admin = await context.newPage()
  await admin.goto('/w/-/settings/members')
  await admin.getByRole('button', { name: 'Create invitation link' }).click()
  const card = admin.getByRole('complementary', { name: 'Invitation link ready' })
  await expect(card).toBeVisible()
  const link = new URL((await card.locator('code').textContent())?.trim() ?? '')
  expect(link.pathname).toBe('/join')
  await context.close()

  // A stranger's browser: no session, only the link.
  const name = `mate${randomUUID().slice(0, 6)}`
  await page.goto(`${link.pathname}${link.hash}`)
  await page.getByLabel('Create an account').check()
  await page.getByLabel('Display name').fill(`Teammate ${name}`)
  await page.getByLabel('Username').fill(name)
  await page.getByLabel('Handle').fill(name)
  await page.getByLabel('Password', { exact: true }).fill('correct horse battery staple')
  await page.getByLabel('Confirm password').fill('correct horse battery staple')
  await page.getByRole('button', { name: 'Create account and join' }).click()
  await expect(page).toHaveURL(/\/w\/[^/]+\//)
  const session = (await (await page.request.get('/api/v1/auth/session')).json()) as {
    user?: { displayName?: string }
    organizations?: unknown[]
  }
  expect(session.user?.displayName).toBe(`Teammate ${name}`)
  expect(session.organizations?.length).toBe(1)
})
