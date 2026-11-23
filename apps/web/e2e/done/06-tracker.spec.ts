import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { seededValue, storageStatePath, webOrigin } from '../environment'
import { asAgent, workspaceId, writer } from '../support/done'

// Test for "done", item 6: a "Playtest feedback" tracker with its own
// fields, a board and a public form, with no code, and an agent triaging
// what comes in. The tracker's form is made in the collection's Forms
// dialog; someone with no account answers it; an agent reads the new
// record and sets its severity, and the board shows it in that column.
// (Making a collection and its fields in the browser is walked in
// collections.spec.ts.)

test.use({ storageState: storageStatePath })

test('a tracker’s public form brings in a record, an agent triages it, and the board shows where it went', async ({ page, context, browser, playwright }, info) => {
  test.skip(info.project.name === 'phone', 'The form builder is used at a desk; the form itself is checked on a phone in collections.spec.ts.')
  const space = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${space}/collections`)
  const { request, headers } = await writer(context, page)
  const workspace = await workspaceId(request)
  const tag = randomUUID().slice(0, 6)
  const ok = async (response: Awaited<ReturnType<typeof request.post>>) => {
    expect(response.ok(), await response.text()).toBe(true)
    const body = (await response.json()) as Record<string, unknown>
    return (body.value ?? body) as Record<string, unknown>
  }
  const made = (await ok(
    await request.post(`/api/v2/w/${workspace}/ops/collection_create`, {
      headers: headers(randomUUID()),
      data: { projectId: space, name: `Playtest feedback ${tag}`, titleName: 'Note' },
    }),
  )) as { collection: { id: string } }
  const collection = made.collection.id
  const base = `/api/v1/projects/${space}/collections/${collection}`
  await ok(
    await request.post(`${base}/fields`, {
      headers: headers(randomUUID()),
      data: { name: 'Severity', type: 'single_choice', options: [{ name: 'Low', color: 'green' }, { name: 'High', color: 'red' }] },
    }),
  )
  const fields = ((await (await request.get(base)).json()) as { fields: Array<{ id: string; name: string }> }).fields
  const board = (await ok(
    await request.post(`/api/v2/w/${workspace}/ops/view_create`, {
      headers: headers(randomUUID()),
      data: { name: 'Triage', type: 'board', config: { groupBy: fields.find((field) => field.name === 'Severity')?.id }, collection, shared: true },
    }),
  )) as { id: string }

  // The form, in the collection's own dialog.
  await page.goto(`/projects/${space}/collections/${collection}`)
  await page.getByRole('button', { name: 'Collection options' }).click()
  await page.getByRole('menuitem', { name: 'Forms…' }).click()
  const dialog = page.getByRole('dialog')
  await dialog.getByRole('button', { name: 'New form' }).click()
  await dialog.getByLabel('Form name').fill(`Tell us about the build ${tag}`)
  await dialog.getByRole('button', { name: 'Make form' }).click()
  const url = (await dialog.locator('code').textContent())?.trim() ?? ''
  expect(url).toContain('/forms/')

  // Someone with no account answers it.
  const outside = await browser.newContext({ storageState: { cookies: [], origins: [] } })
  const form = await outside.newPage()
  await form.goto(url)
  const note = `The checkpoint after the bridge is skipped ${tag}`
  await form.getByLabel('Note (required)').fill(note)
  await form.getByRole('button', { name: 'Send' }).click()
  await expect(form.getByRole('heading', { name: 'Sent' })).toBeVisible()
  await outside.close()

  // An agent finds what came in and triages it.
  const connected = (await ok(
    await request.post(`/api/v1/projects/${space}/external-agents`, {
      headers: headers(),
      data: { connectionName: `Triage ${tag}`, displayName: `Triager${tag}`, handle: `triager${tag}`, role: 'member', scopes: ['read', 'write'] },
    }),
  )) as { oneTimeCredential: { token: string } }
  const bare = await playwright.request.newContext({ baseURL: webOrigin, storageState: { cookies: [], origins: [] } })
  const agent = asAgent(bare, connected.oneTimeCredential.token)
  const listed = await agent.call('record_list', { projectId: space, collection, search: tag }, workspace)
  expect(listed.outcome, JSON.stringify(listed)).toBe('ok')
  const record = (listed.value as { records: Array<{ id: string; title: string }> }).records.find((entry) => entry.title === note)
  expect(record, 'the answer is a record the agent can read').toBeTruthy()
  const triaged = await agent.call('record_update', { record: record?.id, fields: { Severity: 'High' } }, workspace)
  expect(triaged.outcome, JSON.stringify(triaged)).toBe('ok')

  await page.goto(`/projects/${space}/collections/${collection}?view=${board.id}`)
  await expect(page.getByRole('region', { name: 'High', exact: true }).getByRole('link', { name: note })).toBeVisible()
})
