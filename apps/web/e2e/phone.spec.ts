import { readdirSync, readFileSync, existsSync } from 'node:fs'
import path from 'node:path'

import { expect, test, type Page } from '@playwright/test'

import { seededValue, storageStatePath } from './environment'
import { at } from './addresses'

// W1.29, phone navigation at 390 px: every bottom-bar destination opens,
// the back button closes a drawer before it leaves the page, and no
// registered page scrolls sideways. The route list is read from the
// registrations, as the accessibility gate reads it.

test.use({ storageState: storageStatePath, viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true })

const SRC = path.join(process.cwd(), 'src')

function registeredPaths(): string[] {
  const found = new Set<string>(['/home', '/settings'])
  for (const folder of readdirSync(SRC)) {
    const file = path.join(SRC, folder, 'module.ts')
    if (!existsSync(file)) continue
    const source = readFileSync(file, 'utf8')
    for (const match of source.matchAll(/\{\s*path:\s*'([^']+)',\s*space:\s*'(\w+)'/g)) {
      const [, route, space] = match
      if (space === 'none') continue
      const address = route.replace(':projectId', seededValue('E2E_PROJECT_ID')).replace(/\/\*$/, '')
      if (!address.includes(':')) found.add(address)
    }
  }
  return [...found].sort()
}

/** Elements wider than the screen, outside anything that scrolls sideways on purpose. */
async function sideways(page: Page) {
  return page.evaluate(() => {
    const limit = document.documentElement.clientWidth
    const found: string[] = []
    if (document.documentElement.scrollWidth > limit + 1) found.push('the page itself')
    for (const element of document.querySelectorAll<HTMLElement>('main *')) {
      if (element.closest('[data-scroll-x]')) continue
      const style = getComputedStyle(element)
      if (style.position === 'fixed' || style.visibility === 'hidden') continue
      const box = element.getBoundingClientRect()
      if (box.width > 0 && box.right > limit + 1 && !element.parentElement?.closest('[data-scroll-x], pre, table')) {
        // A child of something that clips or scrolls is the parent's business.
        let clipped = false
        for (let parent = element.parentElement; parent && parent.tagName !== 'MAIN'; parent = parent.parentElement) {
          const overflow = getComputedStyle(parent).overflowX
          if (overflow !== 'visible') {
            clipped = true
            break
          }
        }
        if (!clipped) {
          found.push(`<${element.tagName.toLowerCase()} class="${element.className}">`)
          if (found.length > 3) break
        }
      }
    }
    return found
  })
}

test('every bottom-bar destination opens', async ({ page }) => {
  // Visit the seeded space first, so its tabs have a space to open.
  await page.goto(`/projects/${seededValue('E2E_PROJECT_ID')}`)
  await page.goto('/w/-/home')
  const bar = page.getByRole('navigation', { name: 'Main' })
  await expect(bar).toBeVisible()
  const names = (await bar.getByRole('link').allInnerTexts()).map((text) => text.split('\n')[0].trim())
  expect(names[0]).toBe('Home')
  expect(names.length).toBeGreaterThan(1)
  for (const name of names) {
    const tab = bar.getByRole('link', { name: new RegExp(`^${name}`) })
    const to = new URL((await tab.getAttribute('href')) ?? '/', page.url()).pathname
    await tab.click()
    await expect(page).toHaveURL((url) => url.pathname === to)
    await expect(page.locator('#main-workspace')).toBeVisible()
    // A chat (a new message, a DM) gives the bottom of the screen to its box,
    // with no bar; everywhere else the bar marks where you are.
    await expect
      .poll(async () =>
        (await page.locator('html').getAttribute('data-chrome')) === 'conversation'
          ? (await bar.isVisible())
            ? 'chat with a bar'
            : 'chat'
          : await tab.getAttribute('aria-current'),
      )
      .toMatch(/^(chat|page)$/)
    if (!(await bar.isVisible())) await page.goto('/w/-/home')
  }
})

test('back closes the navigation drawer before it leaves the page', async ({ page }) => {
  await page.goto('/w/-/inbox')
  // `-` (the workspace used last) becomes its slug.
  await expect(page).toHaveURL(/\/w\/(?!-\/)[^/]+\/inbox$/)
  const slug = new URL(page.url()).pathname.split('/')[2]
  await page.goto(`/w/${slug}/home`)
  await page.getByRole('button', { name: /open navigation/i }).first().click()
  await expect(page.locator('.aw-shell')).toHaveAttribute('data-drawer', 'nav')
  // The back-button handling loads after the first paint: wait for the drawer's history entry.
  await expect.poll(() => page.evaluate(() => (window.history.state as { awSheet?: string } | null)?.awSheet ?? null)).not.toBeNull()
  await page.goBack()
  await expect(page.locator('.aw-shell')).not.toHaveAttribute('data-drawer', 'nav')
  await expect(page).toHaveURL(/\/home$/)
})

test('no registered page scrolls sideways at 390 px', async ({ page }) => {
  test.setTimeout(240_000)
  const found: string[] = []
  for (const address of registeredPaths()) {
    await page.goto(at(address))
    await page.waitForLoadState('networkidle')
    for (const element of await sideways(page)) found.push(`${address}: ${element}`)
  }
  expect(found).toEqual([])
})
