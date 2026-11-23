import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { seededValue, storageStatePath } from '../environment'
import { workspaceId, writer } from '../support/done'

// Test for "done", item 2: add an agent three ways, from one "Add an agent"
// section in a space's Agents settings:
//   - connect one you already run: a setup message to paste into it;
//   - start a new one: this server's assistant, added at once;
//   - start one on a host you've linked, whose own steps open inside the
//     app. That needs a running host; agent_register.rs and
//     a1_host_protocol.rs drive it end to end against a host stand-in.

test.use({ storageState: storageStatePath })

test('connect one you run with a setup message, and start this server’s assistant in one click', async ({ page, context }) => {
  const space = seededValue('E2E_PROJECT_ID')
  await page.goto(`/w/smoke-org/settings/spaces/${space}/agents`)
  const tag = randomUUID().slice(0, 6)

  // One you run: its setup message, to paste into it.
  await page.getByRole('radio', { name: 'Other agent' }).check()
  await page.getByLabel('Display name').fill(`Runner ${tag}`)
  await page.getByLabel('Handle').fill(`runner-${tag}`)
  await page.getByLabel('Connection name').fill(`Laptop ${tag}`)
  await page.getByRole('button', { name: 'Connect agent' }).click()
  await expect(page.getByText(`Setup message ready for Runner ${tag}`)).toBeVisible()

  // This server's assistant thinks with the workspace's model: give it one.
  const { request, headers } = await writer(context, page)
  const workspace = await workspaceId(request)
  const set = await request.post(`/api/v2/w/${workspace}/ops/setting_update`, {
    headers: headers(randomUUID()),
    data: { key: 'ai.provider', scope: 'workspace', value: 'fake' },
  })
  expect(set.ok(), await set.text()).toBe(true)
  await page.reload()
  await page.getByRole('radio', { name: 'Start a new one' }).click()
  const offer = page.getByRole('listitem').filter({ hasText: 'This server' }).first()
  await offer.getByRole('button', { name: 'Add' }).click()
  await expect(page.getByText(/^Added /).first()).toBeVisible()
})
