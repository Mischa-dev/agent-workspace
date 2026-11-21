import { spawnSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { seeded, seededValue, serverOrigin, webOrigin } from './environment'

// W1.14a: the workspace is in the address. A person in two workspaces opens
// the second by its address and sees its layout and modules, and the rail's
// switcher takes them back. The API can't make a second workspace yet, so the
// spec adds one with SQL (values passed as psql variables, never spliced in)
// and takes the person out of it again at the end.

function sql(statement: string, variables: Record<string, string>) {
  const args = [seededValue('E2E_DATABASE_URL'), '-X', '-q', '-t', '-A', '-v', 'ON_ERROR_STOP=1']
  for (const [name, value] of Object.entries(variables)) args.push('-v', `${name}=${value}`)
  const result = spawnSync('psql', args, { input: statement, encoding: 'utf8' })
  if (result.status !== 0) throw new Error(result.stderr || result.error?.message || 'psql failed')
  return result.stdout.trim()
}

test('opens a second workspace by its address, with its own modules', async ({ page }) => {
  await page.goto('/sign-in')
  await page.getByLabel('Username').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/w\/(?!-\/)[^/]+\/home$/)
  const first = new URL(page.url()).pathname.split('/')[2]

  const session = await (await page.request.get(`${serverOrigin}/api/v1/auth/session`)).json()
  const workspaceId = randomUUID()
  const slug = `zeta-${workspaceId.slice(0, 8)}`
  sql(
    `INSERT INTO organizations (id, name, slug) VALUES (:'id', 'Zeta Workspace', :'slug');
     INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES (:'id', :'actor', 'owner');`,
    { id: workspaceId, slug, actor: session.user.actorId },
  )
  try {
    // Whiteboards off in the second workspace only.
    const csrf = (await page.context().cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
    const switched = await page.request.put(`${serverOrigin}/api/v1/organizations/${workspaceId}/modules`, {
      data: { switches: { whiteboards: false } },
      headers: { origin: webOrigin, 'x-csrf-token': csrf, 'idempotency-key': randomUUID() },
    })
    expect(switched.ok(), await switched.text()).toBe(true)

    const home = page.getByRole('complementary', { name: 'Home navigation' })
    await page.goto(`/w/${slug}/home`)
    await expect(page.getByRole('button', { name: /^Workspace: Zeta Workspace/ })).toBeVisible()
    await expect(home.getByRole('link', { name: 'Inbox' })).toHaveAttribute('href', `/w/${slug}/inbox`)
    await expect(home.getByRole('link', { name: 'Whiteboards' })).toHaveCount(0)
    // The first workspace's project isn't in this one's rail.
    await expect(page.getByRole('navigation', { name: 'Spaces' }).getByRole('link', { name: seeded.projectName })).toHaveCount(0)

    // An address that names no workspace opens in the workspace used last.
    await page.goto('/w/-/inbox')
    await expect(page).toHaveURL(new RegExp(`/w/${slug}/inbox$`))

    await page.getByRole('button', { name: /^Workspace: Zeta Workspace/ }).click()
    await page.getByRole('menuitemcheckbox', { name: /^Smoke Org/ }).click()
    await expect(page).toHaveURL(new RegExp(`/w/${first}/home$`))
    await expect(home.getByRole('link', { name: 'Whiteboards' })).toBeVisible()
    await expect(page.getByRole('navigation', { name: 'Spaces' }).getByRole('link', { name: seeded.projectName })).toBeVisible()

    // A workspace the person isn't in says so.
    await page.goto('/w/nowhere-at-all/home')
    await expect(page.getByRole('heading', { name: 'You’re not in this workspace' })).toBeVisible()
  } finally {
    sql(`UPDATE organization_memberships SET enabled = false WHERE organization_id = :'id';`, { id: workspaceId })
  }
})
