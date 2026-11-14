import { readdirSync, readFileSync, existsSync } from 'node:fs'
import path from 'node:path'

import { expect, test, type Page } from '@playwright/test'

import { seededValue, storageStatePath } from './environment'
import { writer } from './support/done'

// W1.36, the accessibility gate: every page a module registers, at a
// desktop and a phone width, in the light and the dark theme, with no
// serious or critical axe finding; and the task grid driven by keys alone.
// The route list is read from the registrations (src/<folder>/module.ts),
// so a module's new page joins the sweep with no change here. The theme
// contrast of every preset is checked in src/theme/presets.test.ts.

test.use({ storageState: storageStatePath })

const SRC = path.join(process.cwd(), 'src')
const AXE = readFileSync(path.join(process.cwd(), 'node_modules/axe-core/axe.min.js'), 'utf8')

/** Every registered route that needs no id but the seeded project's. */
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

async function axeFindings(page: Page) {
  await page.addScriptTag({ content: AXE })
  return page.evaluate(async () => {
    const axe = (window as unknown as { axe: { run: (context: Document, options: unknown) => Promise<{ violations: { id: string; impact: string | null; nodes: { target: unknown }[] }[] }> } }).axe
    const result = await axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa'] } })
    return result.violations
      .filter((violation) => violation.impact === 'serious' || violation.impact === 'critical')
      .map((violation) => `${violation.id} (${violation.impact}): ${violation.nodes.map((node) => JSON.stringify(node.target)).join(', ')}`)
  })
}

const SIZES = [
  { name: 'desktop', viewport: { width: 1440, height: 900 } },
  { name: 'phone', viewport: { width: 390, height: 844 } },
] as const

for (const scheme of ['light', 'dark'] as const) {
  for (const size of SIZES) {
    test.describe(`${size.name}, ${scheme}`, () => {
      test.use({ colorScheme: scheme, viewport: size.viewport })

      test('every registered page passes axe', async ({ page }) => {
        test.setTimeout(240_000)
        const findings: string[] = []
        for (const address of registeredPaths()) {
          await page.goto(address)
          await page.waitForLoadState('networkidle')
          for (const finding of await axeFindings(page)) findings.push(`${address}: ${finding}`)
        }
        expect(findings).toEqual([])
      })
    })
  }
}

test('the task grid works from the keyboard alone', async ({ page, context }) => {
  const project = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${project}/work-items`)
  const { request, headers } = await writer(context, page)
  for (const title of ['Grid keys first', 'Grid keys second']) {
    const created = await request.post(`/api/v1/projects/${project}/work-items`, {
      headers: headers(`grid-${title}-${Date.now()}`),
      data: { title },
    })
    expect(created.ok(), await created.text()).toBe(true)
  }
  await page.goto(`/projects/${project}/work-items?view=list`)
  const grid = page.getByRole('grid').first()
  const first = grid.getByRole('row').filter({ hasText: 'Grid keys' }).first()
  await first.focus()
  await page.keyboard.press('ArrowDown')
  const focused = page.locator(':focus')
  await expect(focused).toHaveAttribute('data-row-key', /.+/)
  expect(await focused.getAttribute('data-row-key')).not.toBe(await first.getAttribute('data-row-key'))
  await page.keyboard.press('ArrowUp')
  await expect(page.locator(':focus')).toHaveAttribute('data-row-key', (await first.getAttribute('data-row-key')) ?? '')
})
