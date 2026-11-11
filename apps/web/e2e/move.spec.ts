import { expect, test, type BrowserContext, type Page } from '@playwright/test'

import { seededValue, storageStatePath } from './environment'

// AWD-102: moving a task to another project through the web app, against the
// real server: the task menu, the server's list of projects it can move to,
// the private preview, settling its one issue, one confirmation, and landing
// on the new task. The server then holds the carried value on the new task
// and every value on the archived original.

test.use({ storageState: storageStatePath })

type Field = { id: string; name: string; options?: Array<{ id: string; name: string }> }
type Project = { id: string; key: string; name: string; organizationId: string }

async function writer(context: BrowserContext, page: Page) {
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const headers = (key: string) => ({ 'x-csrf-token': csrf, 'idempotency-key': key, origin: new URL(page.url()).origin })
  return { request: page.request, headers }
}

test('a task moves to another project after a private preview and one confirmation', async ({ page, context }) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  const source = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${source}/work-items`)
  const { request, headers } = await writer(context, page)
  const run = Date.now().toString(36).toUpperCase().slice(-4)

  const projects = (await (await request.get('/api/v1/projects')).json()) as { projects: Project[] }
  const home = projects.projects.find((project) => project.id === source)
  expect(home).toBeTruthy()
  const madeTarget = await request.post('/api/v1/projects', {
    headers: headers(`move-target-${run}`),
    data: { organizationId: home?.organizationId, key: `MV${run}`, name: `Moved here ${run}` },
  })
  expect(madeTarget.ok(), await madeTarget.text()).toBe(true)
  const target = (await madeTarget.json()) as Project

  // Notes fits the target's Notes; the target's Tier has no Gold.
  const field = async (projectId: string, body: Record<string, unknown>, key: string) => {
    const made = await request.post(`/api/v1/projects/${projectId}/fields`, { headers: headers(key), data: body })
    expect(made.ok(), await made.text()).toBe(true)
    return (await made.json()) as Field
  }
  await field(source, { name: `Notes ${run}`, type: 'text' }, `move-notes-src-${run}`)
  await field(source, { name: `Tier ${run}`, type: 'single_choice', options: [{ name: 'Gold' }] }, `move-tier-src-${run}`)
  const targetNotes = await field(target.id, { name: `Notes ${run}`, type: 'text' }, `move-notes-dst-${run}`)
  await field(target.id, { name: `Tier ${run}`, type: 'single_choice', options: [{ name: 'Silver' }] }, `move-tier-dst-${run}`)
  const created = await request.post(`/api/v1/projects/${source}/work-items`, {
    headers: headers(`move-task-${run}`),
    data: { title: `Move me ${run}`, fieldValues: { [`Notes ${run}`]: 'carried words', [`Tier ${run}`]: 'Gold' } },
  })
  expect(created.ok(), await created.text()).toBe(true)
  const task = (await created.json()) as { id: string; key: string; fieldValues: Record<string, unknown> }

  await page.goto(`/projects/${source}/work-items/${task.id}`)
  await page.getByRole('button', { name: 'Task actions' }).first().click()
  await page.getByRole('menuitem', { name: 'Move to project…' }).click()
  const dialog = page.getByRole('dialog')
  await expect(dialog.getByRole('option', { name: `Moved here ${run} (MV${run})` })).toBeAttached()
  await dialog.getByLabel('Project').selectOption(target.id)
  await dialog.getByRole('button', { name: 'Review the move' }).click()

  // The private preview: Notes carries; Tier needs a choice.
  await expect(dialog.getByRole('heading', { name: 'Needs a choice' })).toBeVisible()
  await expect(dialog.getByText('carried words')).toBeVisible()
  const move = dialog.getByRole('button', { name: 'Move task' })
  await expect(move).toBeDisabled()
  await dialog.getByLabel(`What happens to Tier ${run} on ${task.key}`).selectOption('leave')
  await expect(move).toBeEnabled()
  await move.click()

  // The new task opens; the original stays archived with every value.
  await page.waitForURL(new RegExp(`/projects/${target.id}/work-items/[0-9a-f-]+$`))
  const newId = page.url().split('/').pop() ?? ''
  const moved = (await (await request.get(`/api/v1/projects/${target.id}/work-items/${newId}`)).json()) as {
    fieldValues: Record<string, unknown>
  }
  expect(moved.fieldValues).toEqual({ [targetNotes.id]: 'carried words' })
  const original = (await (await request.get(`/api/v1/projects/${source}/work-items/${task.id}`)).json()) as {
    archivedAt: string | null
    fieldValues: Record<string, unknown>
  }
  expect(original.archivedAt).not.toBeNull()
  expect(original.fieldValues).toEqual(task.fieldValues)
  expect(errors).toEqual([])
})
