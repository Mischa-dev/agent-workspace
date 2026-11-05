import { createRequire } from 'node:module'

import { expect, test, type Page } from '@playwright/test'

import { storageStatePath } from './environment'

// AWD-75's pieces guard: every piece a module's manifest declares, drawn by
// the app at 1440 px (desktop) and 390 px (phone), in light and dark, with
// nothing cut off sideways and no axe errors. A board scrolls sideways inside
// itself (data-scroll-x); everything else has to fit.

const axePath = createRequire(import.meta.url).resolve('axe-core/axe.min.js')

const widths = [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'phone', width: 390, height: 844 },
] as const

test.use({ storageState: storageStatePath })

/** Samples with something that runs past the right edge of the page, or of its piece. */
async function cutOff(page: Page) {
  return page.evaluate(() => {
    const limit = document.documentElement.clientWidth
    const found: string[] = []
    for (const sample of document.querySelectorAll<HTMLElement>('[data-sample]')) {
      const piece = sample.querySelector<HTMLElement>('[data-piece]')
      if (!piece) continue
      const edge = Math.min(limit, piece.getBoundingClientRect().right)
      for (const element of piece.querySelectorAll<HTMLElement>('*')) {
        // Inside a sideways scroller, running past the edge is how it works.
        if (element.closest('[data-scroll-x]') && !element.matches('[data-scroll-x]')) continue
        const box = element.getBoundingClientRect()
        if (box.width > 0 && box.right > edge + 1) {
          found.push(`${sample.dataset.sample}: <${element.tagName.toLowerCase()} class="${element.className}">`)
          break
        }
      }
    }
    return found
  })
}

for (const scheme of ['light', 'dark'] as const) {
  test.describe(`${scheme} scheme`, () => {
    test.use({ colorScheme: scheme })

    for (const size of widths) {
      test(`every declared piece fits on ${size.name}`, async ({ page }) => {
        const errors: string[] = []
        page.on('pageerror', (error) => errors.push(error.message))
        await page.setViewportSize({ width: size.width, height: size.height })
        await page.goto('/design-system/pieces')
        await expect(page.getByRole('heading', { name: 'Pieces', exact: true })).toBeVisible()
        await expect(page.locator('[data-sample]').first()).toBeVisible()

        expect(await cutOff(page), 'content cut off sideways').toEqual([])
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
        expect(overflow, 'the page scrolls sideways').toBeLessThanOrEqual(0)

        await page.addScriptTag({ path: axePath })
        const violations = await page.evaluate(async () => {
          const axe = (window as unknown as { axe: { run: (context: Element, options: object) => Promise<{ violations: { id: string; nodes: unknown[] }[] }> } }).axe
          const main = document.querySelector('main') ?? document.body
          const result = await axe.run(main, {})
          return result.violations.map((violation) => `${violation.id} (${violation.nodes.length})`)
        })
        expect(violations, 'axe errors').toEqual([])
        expect(errors, 'uncaught errors on the page').toEqual([])
      })
    }
  })
}
