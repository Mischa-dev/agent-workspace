import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { QueryClient, type QueryKey } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'

import { aiKeys } from '../ai/keys'
import { agentKeys } from '../agents/keys'
import { queryKeys } from '../api/queryKeys'
import { collectionKeys } from '../collections/keys'
import { threadsQueryKey } from '../conversations/threadModel'
import { laterKeys } from '../later/keys'
import { memoryKeys } from '../memory/keys'
import { runKeys } from '../runs/keys'
import { sheetKeys } from '../sheets/keys'
import { cacheContext, RECONNECT, SELF_HANDLED } from './handlers'
import { HANDLERS, touchesFor } from './handlers/table'
import type { RealtimeEnvelope } from './realtimeModel'

// W1.3: one event of each type into a seeded cache, and exactly which cached
// queries it makes stale.

const P = '019fb800-0000-7000-8000-000000000001'
const OTHER = '019fb800-0000-7000-8000-000000000002'
const CHANNEL = '019fb800-0000-7000-8000-000000000011'
const THREAD = '019fb800-0000-7000-8000-000000000012'
const DM = '019fb800-0000-7000-8000-000000000013'
const TASK = '019fb800-0000-7000-8000-000000000021'
const RUN = '019fb800-0000-7000-8000-000000000031'
const WS = '019fb800-0000-7000-8000-000000000041'
const SHEET = '019fb800-0000-7000-8000-000000000051'
const COLLECTION = '019fb800-0000-7000-8000-000000000061'

const timeline = (kind: string, parentKind: string | null, extra: Record<string, unknown> = {}) => ({
  conversation: { kind, parentKind },
  linkedWorkItems: [],
  entries: [],
  ...extra,
})

/** What a person with a channel, a thread, a DM, tasks, runs and documents open has cached. */
const SEEDED: Array<[string, QueryKey, unknown?]> = [
  ['rail', queryKeys.projects],
  ['project', queryKeys.project(P)],
  ['direct', queryKeys.direct],
  ['inbox', ['inbox', 'list', 'inbox', 'all']],
  ['my work', queryKeys.myWork],
  ['search', queryKeys.search('launch', P)],
  ['later', laterKeys.list('open')],
  ['sidebar', queryKeys.conversationSidebar(P, CHANNEL)],
  ['other sidebar', queryKeys.conversationSidebar(OTHER, CHANNEL)],
  ['channel timeline', queryKeys.timeline(P, CHANNEL), timeline('channel', null, { linkedWorkItems: [{ id: TASK }] })],
  ['thread timeline', queryKeys.timeline(P, THREAD), timeline('thread', 'channel')],
  ['dm timeline', queryKeys.timeline(P, DM), timeline('dm', null)],
  ['thread board', threadsQueryKey(P, CHANNEL), []],
  ['pins', memoryKeys.pins(P, CHANNEL)],
  ['decisions', memoryKeys.decisions(P)],
  ['page list', memoryKeys.pages(P)],
  ['sheet list', sheetKeys.list(P)],
  ['sheet', sheetKeys.one(P, SHEET)],
  ['collection records', collectionKeys.records(P, COLLECTION, '')],
  ['task list', queryKeys.workItems(P, {})],
  ['task detail', queryKeys.workItem(P, TASK)],
  ['task history', queryKeys.workItemTimeline(P, TASK)],
  ['planning', aiKeys.suggestions(P, {})],
  ['command menu', queryKeys.commandMenuTasks(P, 'lau')],
  ['channel card', queryKeys.thing('channel', CHANNEL)],
  ['task card', queryKeys.thing('task', TASK)],
  ['referenced in', queryKeys.thingReferences('page', SHEET)],
  ['run', runKeys.detail(RUN)],
  ['runs', runKeys.list({})],
  ['usage', runKeys.usage({} as never)],
  ['members', queryKeys.members(P)],
  ['workspace members', ['organization', WS, 'members']],
  ['agent profiles', agentKeys.profiles(P)],
]

function seeded() {
  const queryClient = new QueryClient()
  for (const [, key, data] of SEEDED) queryClient.setQueryData(key, data ?? {})
  return queryClient
}

function event(type: string, payload: Record<string, unknown>, conversationId: string | null = null): RealtimeEnvelope {
  return {
    eventId: '019fb800-0000-7000-8000-0000000000ee',
    eventCursor: 1,
    version: 1,
    projectId: P,
    conversationId,
    type,
    occurredAt: '2026-11-16T22:00:00Z',
    payload,
  }
}

/** Names of the seeded queries the event makes stale. */
async function replay(envelope: RealtimeEnvelope, queryClient = seeded()) {
  for (const touch of touchesFor(envelope, cacheContext(queryClient))) {
    await queryClient.invalidateQueries(touch.filters)
  }
  return SEEDED.filter(([, key]) => queryClient.getQueryState(key)?.isInvalidated).map(([name]) => name).sort()
}

const sorted = (names: string[]) => [...names].sort()

describe('per-event handlers', () => {
  it('a message in a channel refetches its timeline and unread marks, and leaves documents, tasks and other spaces alone', async () => {
    expect(await replay(event('conversation.message.created', { conversationId: CHANNEL }, CHANNEL))).toEqual(
      sorted(['channel timeline', 'sidebar', 'rail', 'channel card', 'referenced in']),
    )
  })

  it("a message in a thread also refetches thread boards (reply counts), and in a DM the Messages overview", async () => {
    expect(await replay(event('conversation.message.created', { conversationId: THREAD }, THREAD))).toEqual(
      sorted(['thread timeline', 'sidebar', 'rail', 'thread board', 'referenced in']),
    )
    expect(await replay(event('conversation.message.created', { conversationId: DM }, DM))).toEqual(
      sorted(['dm timeline', 'sidebar', 'rail', 'direct', 'referenced in']),
    )
  })

  it('an edited message refetches everything that quotes it', async () => {
    expect(await replay(event('conversation.message.updated', { conversationId: CHANNEL }, CHANNEL))).toEqual(
      sorted([
        'channel timeline',
        'sidebar',
        'thread board',
        'pins',
        'decisions',
        'later',
        'search',
        'inbox',
        'referenced in',
        'channel card',
      ]),
    )
  })

  it('reactions, reply requests and read marks touch only what shows them', async () => {
    expect(await replay(event('conversation.message.reactions.updated', { conversationId: CHANNEL }, CHANNEL))).toEqual([
      'channel timeline',
    ])
    expect(await replay(event('conversation.reply_requests.updated', { conversationId: CHANNEL }, CHANNEL))).toEqual([
      'channel timeline',
    ])
    expect(await replay(event('conversation.read_state.updated', { conversationId: CHANNEL }, CHANNEL))).toEqual(
      sorted(['sidebar', 'rail']),
    )
  })

  it('conversations created, renamed, archived and left', async () => {
    expect(await replay(event('conversation.created', { conversationId: THREAD, kind: 'thread' }, THREAD))).toEqual(
      sorted(['sidebar', 'rail', 'thread board']),
    )
    expect(await replay(event('conversation.updated', { conversationId: CHANNEL }, CHANNEL))).toEqual(
      sorted(['channel timeline', 'sidebar', 'channel card', 'inbox']),
    )
    expect(await replay(event('conversation.archived', { conversationId: CHANNEL }, CHANNEL))).toEqual(
      sorted(['channel timeline', 'sidebar', 'channel card', 'inbox', 'rail']),
    )
    expect(await replay(event('conversation.member.removed', { conversationId: DM }))).toEqual(
      sorted(['rail', 'direct', 'sidebar']),
    )
  })

  it('a changed task refetches what shows it besides the patched lists', async () => {
    expect(await replay(event('work_item.updated', { workItemId: TASK, version: 3 }))).toEqual(
      sorted(['task history', 'my work', 'task card', 'referenced in', 'planning', 'command menu', 'channel timeline']),
    )
  })

  it('runs refetch their page, the lists and, for cost changes, usage', async () => {
    expect(await replay(event('run.changed', { runId: RUN, change: 'activity', state: 'running', version: 2 }))).toEqual(
      sorted(['run', 'runs']),
    )
    expect(
      await replay(event('run.changed', { runId: RUN, conversationId: CHANNEL, change: 'finished', state: 'completed', version: 3 }, CHANNEL)),
    ).toEqual(sorted(['run', 'runs', 'usage', 'channel timeline']))
    expect(await replay(event('conversation.agent_run.updated', { conversationId: CHANNEL, agentRunId: RUN }, CHANNEL))).toEqual(
      sorted(['channel timeline', 'run', 'runs']),
    )
  })

  it('notifications, account managers and membership', async () => {
    expect(await replay(event('notifications.updated', { actorId: WS }))).toEqual(sorted(['inbox', 'rail']))
    expect(await replay(event('account.managers.changed', { actorId: WS, accountId: RUN, workspaceId: WS, managing: true }))).toEqual(
      sorted(['workspace members', 'agent profiles']),
    )
    // Who may see what can change: the one event that refetches a whole space.
    const membership = await replay(event('project.membership.updated', { projectId: P, actorId: WS, version: 2 }))
    expect(membership).toContain('sheet list')
    expect(membership).toContain('direct')
    expect(membership).not.toContain('other sidebar')
  })

  it('documents and module switches are left to their own modules, and an unknown event touches nothing', async () => {
    for (const type of SELF_HANDLED) expect(await replay(event(type, {}))).toEqual([])
    expect(await replay(event('something.new', { conversationId: CHANNEL }, CHANNEL))).toEqual([])
  })

  it('reconnecting is the one broad refresh', async () => {
    const queryClient = seeded()
    for (const touch of RECONNECT) await queryClient.invalidateQueries(touch.filters)
    expect(queryClient.getQueryState(sheetKeys.list(P))?.isInvalidated).toBe(true)
    expect(queryClient.getQueryState(queryKeys.conversationSidebar(OTHER, CHANNEL))?.isInvalidated).toBe(true)
  })

  it('every event the server sends has a handler, or a module that handles it', () => {
    const outbox = readFileSync(join(process.cwd(), '..', 'server', 'src', 'outbox.rs'), 'utf8')
    const body = outbox.slice(outbox.indexOf('pub fn event_type(&self)'), outbox.indexOf('pub fn conversation_id(&self)'))
    const types = [...body.matchAll(/=> "([a-z_.]+)"/g)].map((match) => match[1])
    expect(types.length).toBeGreaterThan(10)
    for (const type of types) expect(type in HANDLERS || SELF_HANDLED.has(type), type).toBe(true)
  })
})
