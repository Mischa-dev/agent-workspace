import { expect, test } from '@playwright/test'

import { seededValue, storageStatePath } from './environment'

// W1.42, the automation builder: a person makes "when a task moves to a
// status, comment on it" from the pickers and the operation's own form (no
// JSON), with an operation listed from the registry, then tries it, which
// shows what it would do and writes nothing.

test.use({ storageState: storageStatePath })

test('a rule built from pickers and an operation’s form, tried without writing', async ({ page }) => {
  const projectId = seededValue('E2E_PROJECT_ID')
  const name = `Check reviews ${Date.now() % 100000}`
  await page.goto(`/settings/projects/${projectId}/automations`)
  await page.getByRole('button', { name: 'New rule' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'New rule' })
  await dialog.getByLabel('Name').fill(name)
  await dialog.getByLabel('Trigger').selectOption({ label: 'A task starts matching the view' })
  const status = dialog.getByLabel('Is in')
  const statuses = await status.locator('option').allTextContents()
  await status.selectOption({ label: statuses.find((label) => label !== 'Any status') ?? '' })
  await dialog.getByLabel('Action 1').selectOption({ label: 'Run an operation' })
  await dialog.getByLabel('Operation').selectOption({ label: 'Comment on task' })
  await dialog.getByLabel(/^Task ID/).fill('{{task.key}}')
  await dialog.getByLabel(/^Text/).fill('Please check {{task.title}}')
  await dialog.getByRole('button', { name: 'Create rule' }).click()
  await expect(dialog).toBeHidden()

  const row = page.getByRole('listitem').filter({ hasText: name })
  await row.getByRole('button', { name: `${name} actions` }).click()
  await page.getByRole('menuitem', { name: 'Try it' }).click()
  const trying = page.getByRole('dialog', { name: `Try ${name}` })
  await expect(trying.getByText('Run “Comment on task”')).toBeVisible()
  await expect(trying.getByText(/^Please check/)).toBeVisible()
  await trying.getByRole('button', { name: 'Done' }).click()

  // Trying it ran nothing.
  const rules = await page.evaluate(async ({ projectId, name }) => {
    const api = await import('/src/automations/api.ts')
    const listed = await api.listRules(projectId)
    const rule = listed.rules.find((entry) => entry.name === name)!
    return { rule, runs: (await api.listRuns(projectId, rule.id)).runs.length }
  }, { projectId, name })
  expect(rules.runs).toBe(0)
  expect(rules.rule.actions[0]).toMatchObject({ kind: 'run_operation', operation: 'task_comment_create', input: { taskId: '{{task.key}}' } })
  expect(rules.rule.conditions.filters?.statusId).toBeTruthy()
})
