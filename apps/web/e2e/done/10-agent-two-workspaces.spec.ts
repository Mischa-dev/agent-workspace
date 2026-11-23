import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { seededValue, storageStatePath } from '../environment'
import { secondWorkspace, workspaceId, writer } from '../support/done'

// Test for "done", item 10: add your own agent to a second workspace in one
// step, and DM it once, not once per workspace. The agent is made in the
// first workspace; in the second, Settings › Agents › "Add one you already
// have" adds it with one click. Its DM is then the same conversation from
// either workspace.

test.use({ storageState: storageStatePath })

test('an agent you manage joins your second workspace in one click, and its DM is one conversation from both', async ({ page, context }) => {
  await page.goto('/w/smoke-org/home')
  const { request, headers } = await writer(context, page)
  const first = await workspaceId(request)
  // The second workspace first: an export taken after the agent exists would bring it along.
  const second = await secondWorkspace(context, page)
  const handle = `helper-${randomUUID().slice(0, 6)}`
  const made = await request.post(`/api/v1/projects/${seededValue('E2E_PROJECT_ID')}/external-agents`, {
    headers: headers(),
    data: { connectionName: `${handle} runner`, displayName: `Helper ${handle}`, handle, role: 'member', scopes: ['read', 'write'] },
  })
  expect(made.ok(), await made.text()).toBe(true)
  const agent = ((await made.json()) as { connection: { actorId: string } }).connection.actorId

  const projects = (await (await request.get('/api/v1/projects?limit=100')).json()) as {
    projects: Array<{ id: string; organizationId: string; archivedAt?: string | null }>
  }
  const space = projects.projects.find((project) => project.organizationId === second.id && !project.archivedAt)
  expect(space, 'the second workspace has a space to add agents in').toBeTruthy()

  await page.goto(`/w/${second.slug}/settings/spaces/${space?.id}/agents`)
  await page.getByRole('radio', { name: 'Add one you already have' }).click()
  const row = page.getByRole('listitem').filter({ hasText: `@${handle}` })
  await row.getByRole('button', { name: 'Add' }).click()
  await expect(page.getByText('Added', { exact: true }).first()).toBeVisible()

  // One DM: opened from either workspace, it is the same conversation.
  const open = async (workspace: string) => {
    const response = await request.post('/api/v1/direct/dms', { headers: headers(randomUUID()), data: { actorId: agent, workspaceId: workspace } })
    expect(response.ok(), await response.text()).toBe(true)
    return ((await response.json()) as { conversation: { id: string } }).conversation.id
  }
  const fromFirst = await open(first)
  const fromSecond = await open(second.id)
  expect(fromSecond).toBe(fromFirst)
})
