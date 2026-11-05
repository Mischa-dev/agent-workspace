import path from 'node:path'

import { expect, test, type APIRequestContext, type BrowserContext, type Page } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath } from './environment'

// Custom fields (AWD-89) at a desktop and a phone width, in light and dark:
// Settings › Fields, a task with every kind of value, the list with field
// columns and the board's cards. The PNGs land in e2e-output/screenshots for
// a person to look at; each page must render without sideways scrolling.

test.use({ storageState: storageStatePath })

type Field = { id: string; name: string; options?: Array<{ id: string; name: string }> }

async function writer(context: BrowserContext, page: Page) {
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const headers = (key: string) => ({ 'x-csrf-token': csrf, 'idempotency-key': key, origin: new URL(page.url()).origin })
  return { request: page.request, headers }
}

/** The project's field with this name, made once however many tests ask. */
async function ensureField(request: APIRequestContext, headers: (key: string) => Record<string, string>, projectId: string, body: Record<string, unknown>) {
  const listed = (await (await request.get(`/api/v1/projects/${projectId}/fields`)).json()) as { fields: Field[] }
  const found = listed.fields.find((field) => field.name === body.name)
  if (found) return found
  const created = await request.post(`/api/v1/projects/${projectId}/fields`, { headers: headers(`field-${String(body.name).replace(/[^A-Za-z0-9]+/g, '-')}`), data: body })
  expect(created.ok(), await created.text()).toBe(true)
  return (await created.json()) as Field
}

async function seed(context: BrowserContext, page: Page) {
  const projectId = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${projectId}/work-items`)
  const { request, headers } = await writer(context, page)
  const risk = await ensureField(request, headers, projectId, {
    name: 'Risk',
    type: 'single_choice',
    description: 'How likely this is to slip.',
    options: [{ name: 'Low', color: 'green' }, { name: 'Medium', color: 'yellow' }, { name: 'High', color: 'red' }],
  })
  const areas = await ensureField(request, headers, projectId, {
    name: 'Areas',
    type: 'multiple_choice',
    options: [{ name: 'Web' }, { name: 'Server' }, { name: 'Agents' }],
  })
  const points = await ensureField(request, headers, projectId, { name: 'Story points', type: 'number' })
  await ensureField(request, headers, projectId, { name: 'Owner', type: 'person' })
  await ensureField(request, headers, projectId, { name: 'Shipped', type: 'checkbox' })
  await ensureField(request, headers, projectId, { name: 'Launch', type: 'date' })
  await ensureField(request, headers, projectId, { name: 'Customer', type: 'text' })
  await ensureField(request, headers, projectId, { name: 'Spec', type: 'url' })
  await ensureField(request, headers, projectId, { name: 'Follows', type: 'link', target: 'task' })

  const title = 'Custom fields on a phone and a desktop'
  const existing = (await (await request.get(`/api/v1/projects/${projectId}/work-items?q=${encodeURIComponent(title)}`)).json()) as {
    items: Array<{ id: string }>
  }
  let taskId = existing.items[0]?.id
  if (!taskId) {
    const created = await request.post(`/api/v1/projects/${projectId}/work-items`, {
      headers: headers('fields-task'),
      data: {
        title,
        fieldValues: {
          Risk: 'High',
          Areas: ['Web', 'Agents'],
          'Story points': 5,
          Owner: 'me',
          Shipped: true,
          Launch: '2026-12-01',
          Customer: 'Acme Interplanetary Logistics and Very Long Company Names Incorporated',
          Spec: 'https://example.com/specs/custom-fields-on-tasks-with-a-long-path',
        },
      },
    })
    expect(created.ok(), await created.text()).toBe(true)
    taskId = ((await created.json()) as { id: string }).id
  }
  const columns = ['priority', 'key', 'status', 'details', 'due', `field:${risk.id}`, `field:${areas.id}`, `field:${points.id}`, 'assignee']
  return { projectId, taskId, title, columns }
}

const schemes = [
  { scheme: 'light', theme: 'daylight' },
  { scheme: 'dark', theme: 'midnight' },
] as const
const widths = [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'phone', width: 390, height: 844 },
] as const

for (const { scheme, theme } of schemes) {
  test.describe(`custom fields, ${scheme}`, () => {
    test.use({ colorScheme: scheme })

    test.beforeEach(async ({ page }) => {
      await page.addInitScript(() => {
        window.localStorage.setItem(
          'agent-workspace:appearance:v2',
          JSON.stringify({ theme: 'system', overrides: {}, density: 'comfortable', textSize: 'default' }),
        )
      })
    })

    for (const { name: size, width, height } of widths) {
      test(`settings, task, list and board at ${width} px`, async ({ page, context }) => {
        const errors: string[] = []
        page.on('pageerror', (error) => errors.push(error.message))
        await page.setViewportSize({ width, height })
        const { projectId, taskId, title, columns } = await seed(context, page)
        const shots = [
          {
            name: 'settings',
            path: `/settings/projects/${projectId}/fields`,
            ready: () => expect(page.getByRole('heading', { name: 'Fields', exact: true }).first()).toBeVisible(),
          },
          {
            name: 'task',
            path: `/projects/${projectId}/work-items/${taskId}`,
            ready: () => expect(page.getByRole('button', { name: /^Risk: High/ }).first()).toBeVisible(),
          },
          {
            name: 'list',
            path: `/projects/${projectId}/work-items?fields=${encodeURIComponent(columns.join(','))}`,
            ready: () => expect(page.getByText(title).first()).toBeVisible(),
          },
          {
            name: 'board',
            path: `/projects/${projectId}/work-items?view=board`,
            ready: () => expect(page.getByText(title).first()).toBeVisible(),
          },
        ]
        for (const shot of shots) {
          await page.goto(shot.path)
          await shot.ready()
          await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
          await page.screenshot({
            path: path.join(screenshotDir, `fields-${shot.name}-${scheme}-${size}.png`),
            fullPage: true,
            animations: 'disabled',
          })
          // The board scrolls sideways inside itself by design; the page never does.
          const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
          expect(overflow, `${shot.name}: horizontal scroll at ${width} px`).toBeLessThanOrEqual(0)
        }
        // The task's editor for a choice field opens and lists its options.
        await page.goto(`/projects/${projectId}/work-items/${taskId}`)
        await page.getByRole('button', { name: /^Risk: High/ }).first().click()
        await expect(page.getByRole('option', { name: 'Medium' })).toBeVisible()
        await page.screenshot({ path: path.join(screenshotDir, `fields-editor-${scheme}-${size}.png`), animations: 'disabled' })
        await page.keyboard.press('Escape')
        expect(errors, 'uncaught errors on the page').toEqual([])
      })
    }
  })
}
