import { expect, test, type Page } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath } from './environment'
import { writer } from './support/done'

// T1.27: views work without the mouse. Twenty tasks are triaged with J and
// the decision keys alone, then a change made with a letter key is taken
// back with ⌘Z (Ctrl+Z here).
test.use({ storageState: storageStatePath })

type Project = { id: string; organizationId: string }
type Answer<T> = { outcome: string; value?: T; message?: string }

/** The keyboard's own way to a button: Tab until it has focus, then Enter. */
async function tabTo(page: Page, name: string) {
  for (let step = 0; step < 8; step += 1) {
    await page.keyboard.press('Tab')
    const focused = await page.evaluate(() => document.activeElement?.textContent?.trim() ?? '')
    if (focused === name) return page.keyboard.press('Enter')
  }
  throw new Error(`Tab never reached ${name}`)
}

test('triages twenty tasks without the mouse, then undoes a change with ⌘Z', async ({ page, context }) => {
  test.setTimeout(240_000)
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  const seededProject = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${seededProject}/work-items`)
  const { request, headers } = await writer(context, page)
  const run = Date.now().toString(36).toUpperCase().slice(-4)
  let calls = 0
  const op = async <T>(name: string, input: Record<string, unknown>) => {
    calls += 1
    const response = await request.post(`/api/v2/ops/${name}`, { headers: headers(`keys-${name}-${run}-${calls}`), data: input })
    const answer = (await response.json()) as Answer<T>
    expect(answer.outcome, JSON.stringify(answer)).toBe('ok')
    return answer.value as T
  }

  // A project of its own, so turning on triage leaves the others alone.
  const projects = (await (await request.get('/api/v1/projects')).json()) as { projects: Project[] }
  const home = projects.projects.find((project) => project.id === seededProject)
  const made = await request.post(`/api/v2/w/${home?.organizationId}/ops/space_create`, {
    headers: headers(`keys-project-${run}`),
    data: { key: `KB${run}`, name: `Keyboard ${run}` },
  })
  expect(made.ok(), await made.text()).toBe(true)
  const projectId = ((await made.json()) as { value: Project }).value.id
  await op('triage_enable', { spaceId: projectId })
  // One task to point duplicates at, then twenty waiting in triage.
  const original = await op<{ tasks: Array<{ id?: string | null; key: string }> }>('task_create_many', {
    projectId,
    tasks: [{ title: `Original ${run}` }],
  })
  const created = await op<{ tasks: Array<{ id?: string | null; key: string }> }>('task_create_many', {
    projectId,
    tasks: Array.from({ length: 20 }, (_, index) => ({ title: `Arrived ${run} #${String(index + 1).padStart(2, '0')}`, status: 'Triage' })),
  })
  expect(created.tasks).toHaveLength(20)

  // The built-in Triage view, opened the way its link opens it.
  const collections = (await (await request.get(`/api/v1/projects/${projectId}/collections`)).json()) as { collections: Array<{ id: string; builtin?: string | null }> }
  const tasksCollection = collections.collections.find((collection) => collection.builtin === 'tasks')?.id ?? ''
  const views = await op<{ views: Array<{ id: string; builtin?: string | null; listParams?: Record<string, string> | null }> }>('view_list', { collection: tasksCollection })
  const triageView = views.views.find((view) => view.builtin === 'triage')
  expect(triageView).toBeTruthy()
  const address = new URLSearchParams(triageView?.listParams ?? { viewId: triageView?.id ?? '' })
  address.set('savedView', triageView?.id ?? '')
  await page.goto(`/projects/${projectId}/work-items?${address.toString()}`)

  const toolbar = page.getByRole('toolbar', { name: 'Triage' })
  await expect(toolbar).toBeVisible()
  const waiting = page.locator('[data-row-key]').filter({ hasText: `Arrived ${run}` })
  await expect(waiting).toHaveCount(20)
  await page.locator('body').click({ position: { x: 1, y: 1 } })

  const openTask = () => new URL(page.url()).searchParams.get('task')
  const seen = new Set<string>()
  for (let decided = 0; decided < 20; decided += 1) {
    await page.keyboard.press('j')
    await expect(toolbar.getByText('Decide on the open task')).toBeVisible()
    // J opened the next task waiting, not one already decided.
    await expect.poll(openTask).not.toBeNull()
    const open = openTask() ?? ''
    expect(seen.has(open), `J reopened ${open}`).toBe(false)
    seen.add(open)
    await expect(toolbar.getByRole('button', { name: /Accept/ })).toBeEnabled()
    if (decided === 3) {
      // 2 declines, with a reason.
      await page.keyboard.press('2')
      const dialog = page.getByRole('dialog', { name: 'Decline this task?' })
      await expect(dialog).toBeVisible()
      await page.keyboard.type('Not something we do')
      await tabTo(page, 'Decline')
    } else if (decided === 4) {
      // 3 marks it a duplicate of a task found by its key.
      await page.keyboard.press('3')
      await expect(page.getByRole('dialog', { name: 'The task this one duplicates' }).or(page.getByRole('combobox')).first()).toBeVisible()
      await page.keyboard.type(original.tasks[0].key)
      await expect(page.getByRole('option').filter({ hasText: original.tasks[0].key }).first()).toBeVisible()
      await page.keyboard.press('Enter')
    } else if (decided === 5) {
      // H snoozes it: the first choice has focus.
      await page.keyboard.press('h')
      await expect(page.getByRole('dialog', { name: 'Snooze until' })).toBeVisible()
      await page.keyboard.press('Enter')
    } else {
      await page.keyboard.press('1')
    }
    await expect(waiting).toHaveCount(19 - decided)
    // The panel closes once the decision lands.
    await expect(toolbar.getByText('Open a task to decide on it (↓ or J)')).toBeVisible()
  }
  await page.screenshot({ path: `${screenshotDir}/keyboard-triage-empty.png`, fullPage: true })

  // Back in the full list: S changes the first accepted task's status, ⌘Z takes it back.
  await page.goto(`/projects/${projectId}/work-items?q=${encodeURIComponent(`Arrived ${run} #01`)}`)
  const row = page.locator('[data-row-key]').filter({ hasText: `Arrived ${run} #01` })
  await expect(row).toHaveCount(1)
  const first = created.tasks[0]
  const statusOf = async () => {
    const task = (await (await request.get(`/api/v1/projects/${projectId}/work-items/${first.id}`)).json()) as { status?: { name?: string } }
    return task.status?.name ?? ''
  }
  const accepted = await statusOf()
  expect(accepted).not.toBe('Triage')
  await page.locator('body').click({ position: { x: 1, y: 1 } })
  await page.keyboard.press('j')
  await page.keyboard.press('s')
  await expect(page.getByPlaceholder('Status')).toBeFocused()
  await page.keyboard.type('In Progress')
  await page.keyboard.press('Enter')
  await expect.poll(statusOf).toBe('In Progress')
  await page.keyboard.press('ControlOrMeta+z')
  await expect(page.getByText(`Undone: Changed the status of ${first.key}`)).toBeVisible()
  await expect.poll(statusOf).toBe(accepted)
  // Undo again has nothing left to take back.
  await page.keyboard.press('ControlOrMeta+z')
  await expect(page.getByText('Nothing to undo')).toBeVisible()
  expect(errors).toEqual([])
})
