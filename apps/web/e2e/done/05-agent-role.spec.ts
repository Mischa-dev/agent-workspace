import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { seededValue, storageStatePath } from '../environment'
import { workspaceId, writer } from '../support/done'

// Test for "done", item 5: give an agent exactly the access a new hire would
// get, or exactly the access you have yourself, by picking a role. In
// Members, the agent's role is picked like a person's; what it can then do
// is that role's permissions, nothing added or taken away for being an
// agent.

test.use({ storageState: storageStatePath })

test('picking a role in Members gives an agent exactly that role’s access', async ({ page, context }) => {
  await page.goto('/w/-/settings/members')
  const { request, headers } = await writer(context, page)
  const workspace = await workspaceId(request)
  const op = async <T,>(name: string, data: Record<string, unknown>) => {
    const response = await request.post(`/api/v2/w/${workspace}/ops/${name}`, { headers: headers(randomUUID()), data })
    expect(response.ok(), await response.text()).toBe(true)
    const body = (await response.json()) as { value?: T } & T
    return body.value ?? body
  }
  const handle = `hire-${randomUUID().slice(0, 6)}`
  const made = await request.post(`/api/v1/projects/${seededValue('E2E_PROJECT_ID')}/external-agents`, {
    headers: headers(),
    data: { connectionName: `${handle} runner`, displayName: `Hire ${handle}`, handle, role: 'viewer', scopes: ['read'] },
  })
  expect(made.ok(), await made.text()).toBe(true)
  const agent = ((await made.json()) as { connection: { actorId: string } }).connection.actorId

  type Roles = { roles: Array<{ id: string; name: string; key: string; permissions: string[] }> }
  const roles = await op<Roles>('role_list', {})
  type Permissions = { workspace: { permissions: string[]; roleName: string } }
  const mine = await op<Permissions>('permissions_get', {})

  // A new hire gets the role invitations default to (Member); you have your own.
  const newHire = roles.roles.find((role) => role.key === 'member')
  const yours = roles.roles.find((role) => role.name === mine.workspace.roleName && role.key !== 'owner') ?? roles.roles.find((role) => role.key === 'admin')
  for (const role of [newHire, yours]) {
    expect(role).toBeTruthy()
    await page.goto('/w/-/settings/members')
    await page.getByLabel(`Role for Hire ${handle}`).selectOption({ label: role?.name ?? '' })
    await expect(page.getByLabel(`Role for Hire ${handle}`)).toHaveValue(role?.id ?? '')
    const its = await op<Permissions>('permissions_get', { account: agent })
    expect(its.workspace.roleName).toBe(role?.name)
    expect([...its.workspace.permissions].sort()).toEqual([...(role?.permissions ?? [])].sort())
  }
})
