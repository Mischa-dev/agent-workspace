import { spawnSync } from 'node:child_process'

import { expect, test, type Page } from '@playwright/test'

import { seeded, seededValue, webOrigin } from './environment'
import { workOsStandIn } from './workos-stand-in'

// The whole single sign-on round trip, through a local stand-in for WorkOS on
// another site: link it to an account, sign in with it, stay signed in past
// the server's periodic check with WorkOS, and sign out.

async function signInWithPassword(page: Page) {
  await page.goto('/sign-in')
  await page.getByLabel('Username').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/home$/)
}

/** Makes the server ask WorkOS again on the next request, as it does every five minutes. */
function expireWorkOsChecks() {
  const database = `${(process.env.E2E_DATABASE_BASE ?? process.env.CI_DATABASE_BASE ?? '').replace(/\/+$/, '')}/${process.env.E2E_DATABASE_NAME ?? 'aw_e2e'}`
  const result = spawnSync(
    'psql',
    [database, '-v', 'ON_ERROR_STOP=1', '-c', "UPDATE sessions SET workos_checked_at = now() - interval '10 minutes' WHERE workos_session_id IS NOT NULL"],
    { encoding: 'utf8' },
  )
  if (result.status !== 0) throw new Error(`psql failed: ${result.stderr}`)
}

test('links single sign-on, signs in with it, stays signed in, and signs out', async ({ browser }) => {
  // Link it from a password session.
  const linking = await browser.newContext()
  const page = await linking.newPage()
  await signInWithPassword(page)
  await page.goto('/settings/account/profile')
  await page.getByRole('button', { name: 'Link single sign-on account' }).click()
  await expect(page).toHaveURL(/\/settings\/account/)
  await expect(page.getByText(`Single sign-on linked as ${workOsStandIn.user.email}.`)).toBeVisible()
  await linking.close()

  // Sign in with it from a fresh, signed-out browser.
  const context = await browser.newContext()
  const signIn = await context.newPage()
  await signIn.goto('/sign-in')
  await signIn.getByRole('button', { name: 'Continue with single sign-on' }).click()
  await expect(signIn).toHaveURL(/\/settings\/account/)
  await expect(signIn.getByText(`Single sign-on linked as ${workOsStandIn.user.email}.`)).toBeVisible()

  // Still signed in after a reload, on another page, and after the server checks with WorkOS again.
  await signIn.goto('/home')
  await expect(signIn).toHaveURL(/\/home$/)
  expireWorkOsChecks()
  await signIn.reload()
  await expect(signIn).toHaveURL(/\/home$/)
  await expect(
    signIn.getByRole('navigation', { name: 'Spaces' }).getByRole('link', { name: seeded.projectName }),
  ).toBeVisible()

  // Signing out ends it here and at WorkOS, so the next check signs this browser out too.
  const session = await signIn.request.get('/api/v1/auth/session')
  expect(session.ok()).toBe(true)
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const signOut = await signIn.request.post('/api/v1/auth/logout', { headers: { 'x-csrf-token': csrf, origin: webOrigin } })
  expect(signOut.ok()).toBe(true)
  await signIn.goto('/home')
  await expect(signIn).toHaveURL(/\/sign-in/)
  await context.close()
})

test('explains when no account is linked to the single sign-on', async ({ browser }) => {
  // The stand-in always answers with the linked user, so unlink it first through the database.
  const database = `${(process.env.E2E_DATABASE_BASE ?? process.env.CI_DATABASE_BASE ?? '').replace(/\/+$/, '')}/${process.env.E2E_DATABASE_NAME ?? 'aw_e2e'}`
  const unlink = spawnSync('psql', [database, '-v', 'ON_ERROR_STOP=1', '-c', 'DELETE FROM workos_identities'], { encoding: 'utf8' })
  if (unlink.status !== 0) throw new Error(`psql failed: ${unlink.stderr}`)

  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto('/sign-in')
  await page.getByRole('button', { name: 'Continue with single sign-on' }).click()
  await expect(page).toHaveURL(/\/sign-in\?workosError=unlinked/)
  await expect(page.getByText('That single sign-on account isn’t linked to an account here yet.', { exact: false })).toBeVisible()
  await context.close()
})
