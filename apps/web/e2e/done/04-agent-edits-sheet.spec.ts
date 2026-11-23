import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { seededValue, storageStatePath, webOrigin } from '../environment'
import { asAgent, workspaceId, writer } from '../support/done'

// Test for "done", item 4: an agent puts last week's numbers in the budget
// sheet while you watch on your phone: the cells change live, the edit
// carries the agent's name, and one tap undoes it. (Asking the agent is a
// message like any other; what's checked here is what you then see.)

test.use({ storageState: storageStatePath })

test('an agent’s sheet edit shows live, with its name, and one tap undoes it', async ({ page, context, playwright }, info) => {
  const space = seededValue('E2E_PROJECT_ID')
  await page.goto('/w/smoke-org/home')
  const { request, headers } = await writer(context, page)
  const workspace = await workspaceId(request)
  const tag = randomUUID().slice(0, 6)
  const made = await request.post(`/api/v2/w/${workspace}/ops/sheet_create`, {
    headers: headers(randomUUID()),
    data: {
      projectId: space,
      name: `Budget ${tag}`,
      cells: [
        { cell: 'A1', input: 'Week' },
        { cell: 'B1', input: 'Spend' },
        { cell: 'A2', input: 'Last week' },
      ],
    },
  })
  expect(made.ok(), await made.text()).toBe(true)
  const sheet = ((await made.json()) as { value?: { id: string }; id?: string }).value?.id ?? ''

  const agentName = `Bookkeeper${tag}`
  const connected = await request.post(`/api/v1/projects/${space}/external-agents`, {
    headers: headers(),
    data: { connectionName: `${agentName} runner`, displayName: agentName, handle: agentName.toLowerCase(), role: 'member', scopes: ['read', 'write'] },
  })
  expect(connected.ok(), await connected.text()).toBe(true)
  const token = ((await connected.json()) as { oneTimeCredential: { token: string } }).oneTimeCredential.token
  const bare = await playwright.request.newContext({ baseURL: webOrigin, storageState: { cookies: [], origins: [] } })
  const agent = asAgent(bare, token)

  await page.goto(`/projects/${space}/sheets/${sheet}`)
  // A grid at a desk, a card per row on a phone.
  const row = info.project.name === 'phone'
    ? page.getByRole('list', { name: 'Rows' }).getByRole('listitem').filter({ hasText: 'Row 2' })
    : page.getByRole('grid').getByRole('row').nth(2)
  await expect(row).toContainText('Last week')
  await expect(row).not.toContainText('1240')

  const wrote = await agent.call('sheet_update', { projectId: space, sheet, cells: [{ cell: 'B2', input: '1240' }] }, workspace)
  expect(wrote.outcome, JSON.stringify(wrote)).toBe('ok')

  // Live: no reload.
  await expect(row).toContainText(/1,?240/)
  const recent = page.getByRole('status').filter({ hasText: `${agentName} changed` })
  await expect(recent).toBeVisible()
  await recent.getByRole('button', { name: 'Undo' }).click()
  await expect(row).not.toContainText(/1,?240/)
})
