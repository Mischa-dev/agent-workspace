import { randomUUID } from 'node:crypto'

import { expect, test } from '@playwright/test'

import { seededValue, storageStatePath, webOrigin } from '../environment'
import { asAgent, workspaceId, writer } from '../support/done'

// Test for "done", item 7: see what every agent is doing, what it cost and
// what it's waiting on, in one place, whoever runs it. Two outside agents
// report their runs through operations (one working and spending, one
// waiting on a question); the Runs page shows both, side by side.

test.use({ storageState: storageStatePath })

test('Runs shows each agent’s work, its cost and what it waits on', async ({ page, context, playwright }) => {
  await page.goto('/w/smoke-org/home')
  // Agents call with their own token only: a context with no session.
  const bare = await playwright.request.newContext({ baseURL: webOrigin, storageState: { cookies: [], origins: [] } })
  const { request, headers } = await writer(context, page)
  const workspace = await workspaceId(request)
  const space = seededValue('E2E_PROJECT_ID')
  const tag = randomUUID().slice(0, 6)
  const agent = async (name: string) => {
    const made = await request.post(`/api/v1/projects/${space}/external-agents`, {
      headers: headers(),
      data: { connectionName: `${name} runner`, displayName: name, handle: name.toLowerCase(), role: 'member', scopes: ['read', 'write'] },
    })
    expect(made.ok(), await made.text()).toBe(true)
    const token = ((await made.json()) as { oneTimeCredential: { token: string } }).oneTimeCredential.token
    return asAgent(bare, token)
  }
  const call = async (client: ReturnType<typeof asAgent>, operation: string, input: Record<string, unknown>) => {
    const answer = await client.call(operation, input, workspace)
    expect(answer.outcome, JSON.stringify(answer)).toBe('ok')
    return answer.value as Record<string, unknown>
  }

  const worker = await agent(`Worker${tag}`)
  const working = (await call(worker, 'run_create', { spaceId: space, title: `Tidy the backlog ${tag}`, cause: { kind: 'manual' } })) as {
    run: { runId: string }
  }
  await call(worker, 'run_update', { runId: working.run.runId, statusLine: `Merging duplicates ${tag}` })
  await call(worker, 'run_usage_report', {
    runId: working.run.runId,
    model: 'any-model',
    reportKey: `usage-${tag}`,
    inputTokens: 12000,
    outputTokens: 3000,
    cost: 0.42,
    currency: 'USD',
  })

  const asker = await agent(`Asker${tag}`)
  const waiting = (await call(asker, 'run_create', { spaceId: space, title: `Archive old pages ${tag}`, cause: { kind: 'manual' } })) as {
    run: { runId: string }
  }
  const me = ((await (await request.get('/api/v1/auth/session')).json()) as { user: { actorId: string } }).user.actorId
  await call(asker, 'run_activity_report', {
    runId: waiting.run.runId,
    activities: [{ id: 'q1', kind: 'interrupt', payload: { request: { title: `May I archive 40 pages? ${tag}`, audience: { accounts: [me] } } } }],
  })

  await page.goto('/w/smoke-org/runs')
  const runs = page.getByRole('list', { name: 'Runs' })
  const workingRow = runs.getByRole('listitem').filter({ hasText: `Tidy the backlog ${tag}` })
  await expect(workingRow).toContainText(`Worker${tag}`)
  await expect(workingRow).toContainText(`Merging duplicates ${tag}`)
  await expect(workingRow).toContainText(/\$0\.42|0\.42/)
  const waitingRow = runs.getByRole('listitem').filter({ hasText: `Archive old pages ${tag}` })
  await expect(waitingRow).toContainText(`Asker${tag}`)
  await expect(waitingRow).toContainText(`Waiting on: May I archive 40 pages? ${tag}`)
})
