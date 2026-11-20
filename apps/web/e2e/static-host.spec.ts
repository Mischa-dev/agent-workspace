import { expect, test } from '@playwright/test'

import { seeded, seededValue, serverOrigin, staticOrigin } from './environment'
import { startStaticHost } from './support/staticHost'

// W1.38 (R24.1): the built app, served as plain files from one origin, signs
// in and works against the API on another. The static host knows nothing of
// the app: it serves files, index.html for any other path, and a
// client-config file pointing at the API (docs/self-host/static-web-app.md).
// CI passes the build it just made (E2E_STATIC_DIST); otherwise this builds one.

let stop: (() => Promise<void>) | undefined

test.beforeAll(async () => {
  test.setTimeout(240_000)
  stop = await startStaticHost()
})

test.afterAll(async () => {
  await stop?.()
})

test('the built app from a static host signs in and talks to the API on another origin', async ({ page }) => {
  const toApi: string[] = []
  page.on('request', (request) => {
    if (request.url().startsWith(`${serverOrigin}/api/`)) toApi.push(request.url())
  })
  await page.goto(`${staticOrigin}/sign-in`)
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible()
  await page.getByLabel('Username').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()

  await expect(page).toHaveURL(new RegExp(`^${staticOrigin}/(?:w/[^/]+/)?home$`))
  await expect(
    page.getByRole('navigation', { name: 'Spaces' }).getByRole('link', { name: seeded.projectName }),
  ).toBeVisible()
  // Every call went to the API's origin, not the static host.
  expect(toApi.some((url) => url.includes('/api/v1/auth/login'))).toBe(true)
})
