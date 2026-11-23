import { expect, test, type BrowserContext, type Page } from '@playwright/test'

import { parseServerFrame } from '../src/realtime/live/protocol'
import { parseRealtimeEnvelope } from '../src/realtime/realtimeModel'
import { seededValue, storageStatePath } from './environment'
import { recordFrames, type Recorder } from './support/recordFrames'

// W1.7: the real server's frames for a chat burst, task moves, page edits and
// a reconnect, recorded from the page's live connection. Each run checks that
// every frame is one the client reads; RECORD_FRAMES=1 also writes them as the
// fixtures src/realtime/live/replay.test.ts replays.

test.use({ storageState: storageStatePath })

async function writer(context: BrowserContext, page: Page) {
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  let sent = 0
  const headers = () => ({ 'x-csrf-token': csrf, 'idempotency-key': `live-streams-${Date.now()}-${sent++}`, origin: new URL(page.url()).origin })
  return { request: page.request, headers }
}

/** The durable event a protocol 2 `event` frame carries, if it is one. */
const eventType = (frame: unknown) => (frame as { type?: string; event?: { type?: string } }).type === 'event' ? (frame as { event?: { type?: string } }).event?.type : undefined

/** Waits for `count` durable events of `type` since the recorder was cleared. */
async function received(recorder: Recorder, type: string, count: number) {
  await expect
    .poll(() => recorder.frames().filter((frame) => eventType(frame) === type).length, { timeout: 15_000 })
    .toBeGreaterThanOrEqual(count)
}

/** Every recorded frame is a socket marker or a protocol 2 frame the client reads, events carrying an envelope. */
function expectReadable(recorder: Recorder) {
  for (const frame of recorder.frames()) {
    if ((frame as { $?: string }).$) continue
    const wire = parseServerFrame(frame)
    expect(wire && wire.type !== 'other', JSON.stringify(frame)).toBe(true)
    if (wire?.type === 'event') expect(parseRealtimeEnvelope(wire.event), JSON.stringify(frame)).not.toBeNull()
  }
}

const text = (value: string) => ({ content: { version: 1, nodes: [{ type: 'text', text: value }] } })

test('records the live frames of a chat burst, task moves, page edits and a reconnect', async ({ context, page }) => {
  const projectId = seededValue('E2E_PROJECT_ID')
  const channelId = seededValue('E2E_CHANNEL_ID')
  const recorder = recordFrames(page)
  await page.goto(`/projects/${projectId}/channels/${channelId}`)
  await expect(page.getByRole('main')).toBeVisible()
  const { request, headers } = await writer(context, page)
  const say = async (value: string) => {
    const sent = await request.post('/api/v2/ops/message_send', { headers: headers(), data: { conversationId: channelId, ...text(value) } })
    expect(sent.ok(), await sent.text()).toBe(true)
  }
  // The socket is open before anything is recorded.
  await expect.poll(() => recorder.frames().length, { timeout: 15_000 }).toBeGreaterThanOrEqual(0)
  await page.waitForTimeout(500)

  // A burst of messages in one channel.
  recorder.clear()
  for (const line of ['Build 14 is up', 'Smoke tests pass', 'Two flaky screenshots', 'Rerunning them', 'All green']) await say(line)
  await received(recorder, 'conversation.message.created', 5)
  expectReadable(recorder)
  recorder.save('chat-burst', { project: projectId, channel: channelId })

  // Two tasks moved to the next column.
  const workflow = (await (await request.get(`/api/v1/projects/${projectId}/work-item-workflow`)).json()) as { statuses: Array<{ id: string }> }
  const target = workflow.statuses[1]?.id ?? workflow.statuses[0].id
  const tasks: Array<{ id: string; version: number }> = []
  for (const title of ['Record the frames', 'Replay the frames']) {
    const created = await request.post('/api/v2/ops/task_create', { headers: headers(), data: { projectId, title } })
    expect(created.ok(), await created.text()).toBe(true)
    const { workItemId, version } = ((await created.json()) as { value: { workItemId: string; version: number } }).value
    tasks.push({ id: workItemId, version })
  }
  await page.waitForTimeout(500)
  recorder.clear()
  for (const task of tasks) {
    const moved = await request.post('/api/v2/ops/task_update', {
      headers: headers(),
      data: { taskId: task.id, expectedVersion: task.version, status: target, position: 'top' },
    })
    expect(moved.ok(), await moved.text()).toBe(true)
  }
  await received(recorder, 'work_item.updated', 2)
  await page.waitForTimeout(500)
  expectReadable(recorder)
  recorder.save('task-board-moves', { project: projectId, taskA: tasks[0].id, taskB: tasks[1].id })

  // An agent writing a page in three steps.
  const created = await request.post('/api/v2/ops/page_create', { headers: headers(), data: { projectId, title: 'Release notes', body: 'Draft\n' } })
  expect(created.ok(), await created.text()).toBe(true)
  const pageId = ((await created.json()) as { value: { id: string } }).value.id
  await page.waitForTimeout(500)
  recorder.clear()
  let body = 'Draft\n'
  for (const line of ['Faster sign-in.', 'Boards move cards live.', 'Fewer refetches.']) {
    const current = (await (await request.get(`/api/v1/pages/${pageId}`)).json()) as { version: number }
    body = `${body}${line}\n`
    const saved = await request.post('/api/v2/ops/page_update', { headers: headers(), data: { pageId, body, expectedVersion: current.version } })
    expect(saved.ok(), await saved.text()).toBe(true)
  }
  await received(recorder, 'page.updated', 3)
  expectReadable(recorder)
  recorder.save('page-edits', { project: projectId, page: pageId })

  // The connection drops and comes back: one message on each side.
  recorder.clear()
  await say('Before the drop')
  await received(recorder, 'conversation.message.created', 1)
  await page.reload()
  await expect(page.getByRole('main')).toBeVisible()
  await expect.poll(() => recorder.frames().some((frame) => (frame as { $?: string }).$ === 'open'), { timeout: 15_000 }).toBe(true)
  await page.waitForTimeout(500)
  await say('After the drop')
  await received(recorder, 'conversation.message.created', 2)
  expectReadable(recorder)
  recorder.save('reconnect', { project: projectId, channel: channelId })
})
