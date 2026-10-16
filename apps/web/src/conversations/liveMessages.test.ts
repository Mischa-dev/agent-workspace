import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { normalizeTimelinePage } from './api'
import { buildMessageContent, draftFromContent } from './composerModel'
import {
  applyLiveMessages,
  dropRemovedMessage,
  needsRefresh,
  NEUTRAL_THREAD_NAME,
  refreshMemoryCaches,
  rememberMessageUpdate,
  type MessageUpdates,
} from './liveMessages'
import { threadsQueryKey } from './threadModel'
import type { ConversationActor, ConversationTimelineEntry } from './types'

const projectId = '019fb600-0000-7000-8000-000000000001'
const conversationId = '019fb600-0000-7000-8000-000000000002'
const authorId = '019fb600-0000-7000-8000-000000000003'

const author: ConversationActor = {
  id: authorId,
  kind: 'human',
  displayName: 'Bob',
  handle: 'bob',
  avatarUrl: null,
}

function messageId(sequence: number) {
  return `019fb601-0000-7000-8000-${String(sequence).padStart(12, '0')}`
}

function wireEntry(sequence: number, text: string, extra: Record<string, unknown> = {}) {
  return {
    id: `019fb602-0000-7000-8000-${String(sequence).padStart(12, '0')}`,
    sequence,
    kind: 'message',
    createdAt: '2026-10-01T10:00:00Z',
    message: {
      id: messageId(sequence),
      author,
      content: { version: 1, nodes: [{ type: 'text', text }] },
      plainText: text,
      mentionedActors: [],
      referencedWorkItems: [],
      createdAt: '2026-10-01T10:00:00Z',
      ...extra,
    },
    workerEvent: null,
  }
}

function page(entries: unknown[]) {
  return normalizeTimelinePage({
    conversation: { id: conversationId, projectId, kind: 'channel', name: 'general', version: 1 },
    entries,
    nextCursor: null,
    highestSequence: entries.length,
    readState: { highestSeenSequence: 0, version: 0 },
    capabilities: { view: true, sendMessage: true, editMessages: true, removeOwnMessages: true },
  })
}

function entries(...list: unknown[]): ConversationTimelineEntry[] {
  return page(list).entries
}

describe('removed and edited messages', () => {
  it('keeps a removed message as a tombstone and never keeps its text', () => {
    const normalized = page([
      wireEntry(1, 'ghp_secret', {
        content: { version: 1, nodes: [] },
        plainText: 'ghp_secret',
        mentionedActors: [author],
        revision: 3,
        editedAt: '2026-10-01T10:05:00Z',
        removed: { removedAt: '2026-10-01T10:06:00Z', removedBy: { ...author, id: projectId, displayName: 'Mara' } },
      }),
      wireEntry(2, 'Fine', { revision: 2, editedAt: '2026-10-01T10:07:00Z' }),
    ])
    expect(normalized.entries).toHaveLength(2)
    const [removed, edited] = normalized.entries
    expect(removed.message?.removed?.removedBy?.displayName).toBe('Mara')
    expect(removed.message?.plainText).toBe('')
    expect(removed.message?.content.nodes).toEqual([])
    expect(removed.message?.mentionedActors).toEqual([])
    expect(JSON.stringify(removed)).not.toContain('ghp_secret')
    expect(edited.message).toMatchObject({ revision: 2, editedAt: '2026-10-01T10:07:00Z', removed: null })
    expect(normalized.capabilities).toMatchObject({ editMessages: true, removeOwnMessages: true, removeAnyMessage: false })
  })

  it('reads a message without the new fields as revision 1, never edited', () => {
    const [entry] = entries(wireEntry(1, 'Hello'))
    expect(entry.message).toMatchObject({ revision: 1, editedAt: null, removed: null })
  })

  it('blanks a live-removed message during render and applies newer replacements only', () => {
    const list = entries(wireEntry(1, 'ghp_secret'), wireEntry(2, 'Draft'))
    const updates: MessageUpdates = {
      [messageId(1)]: { conversationId, messageId: messageId(1), revision: 1, removed: true },
    }
    const [newer] = entries(wireEntry(2, 'Final', { revision: 2 }))
    const [stale] = entries(wireEntry(2, 'Stale', { revision: 1 }))

    const patched = applyLiveMessages(list, updates, { [messageId(2)]: newer }, conversationId)
    expect(patched[0].message?.removed).not.toBeNull()
    expect(patched[0].message?.plainText).toBe('')
    expect(JSON.stringify(patched)).not.toContain('ghp_secret')
    expect(patched[1].message?.plainText).toBe('Final')
    expect(patched[1].sequence).toBe(2)

    expect(applyLiveMessages(list, {}, { [messageId(2)]: stale }, conversationId)).toBe(list)
    // An update for a message in another conversation never touches this one.
    const elsewhere = { [messageId(1)]: { ...updates[messageId(1)], conversationId: projectId } }
    expect(applyLiveMessages(list, elsewhere, {}, conversationId)).toBe(list)
  })

  it('asks for a refetch only when the server has something newer', () => {
    const [entry] = entries(wireEntry(1, 'Hello', { revision: 2 }))
    const message = entry.message!
    const update = { conversationId, messageId: message.id, revision: 2, removed: false }
    expect(needsRefresh(message, update)).toBe(false)
    expect(needsRefresh(message, { ...update, revision: 3 })).toBe(true)
    expect(needsRefresh(message, { ...update, removed: true })).toBe(true)
    expect(needsRefresh(message, undefined)).toBe(false)
  })

  it('remembers a bounded set of updates and never forgets a removal for an older edit', () => {
    let updates: MessageUpdates = {}
    for (let index = 1; index <= 5; index += 1) {
      updates = rememberMessageUpdate(
        updates,
        { conversationId, messageId: messageId(index), revision: 1, removed: false },
        3,
      )
    }
    expect(Object.keys(updates)).toEqual([messageId(3), messageId(4), messageId(5)])
    updates = rememberMessageUpdate(updates, { conversationId, messageId: messageId(3), revision: 1, removed: true }, 3)
    const repeated = rememberMessageUpdate(updates, { conversationId, messageId: messageId(3), revision: 1, removed: false }, 3)
    expect(repeated).toBe(updates)
    expect(updates[messageId(3)].removed).toBe(true)
    expect(Object.keys(updates).at(-1)).toBe(messageId(3))
  })

  it('drops removed text from cached timelines, history, pins and search at once', () => {
    const queryClient = new QueryClient()
    const timelineKey = queryKeys.timeline(projectId, conversationId)
    const historyKey = queryKeys.messageRevisions(projectId, conversationId, messageId(1))
    queryClient.setQueryData(timelineKey, page([wireEntry(1, 'ghp_secret'), wireEntry(2, 'Keep me')]))
    queryClient.setQueryData(historyKey, {
      messageId: messageId(1),
      revision: 2,
      removed: false,
      revisions: [{ revision: 1, content: { version: 1, nodes: [] }, plainText: 'ghp_secret', redacted: false }],
    })
    queryClient.setQueryData(['project', projectId, 'memory', 'pins', conversationId], {
      pins: [
        { kind: 'message', id: messageId(1), title: 'Bob', text: 'ghp_secret' },
        { kind: 'page', id: messageId(9), title: 'Guide', text: 'Read me' },
      ],
    })
    queryClient.setQueryData(['search', 'ghp', ''], {
      hits: [{ kind: 'message', id: messageId(1), snippet: '**ghp_secret**' }],
    })

    dropRemovedMessage(queryClient, { conversationId, messageId: messageId(1) })

    const cached = JSON.stringify([
      queryClient.getQueryData(timelineKey),
      queryClient.getQueryData(historyKey),
      queryClient.getQueryData(['project', projectId, 'memory', 'pins', conversationId]),
      queryClient.getQueryData(['search', 'ghp', '']),
    ])
    expect(cached).not.toContain('ghp_secret')
    expect(cached).toContain('Keep me')
    expect(cached).toContain('Read me')
  })

  it('renames threads started from a removed message on boards and sidebars at once', () => {
    const queryClient = new QueryClient()
    const thread = (id: number, name: string, originMessageId: string | null) => ({
      id: messageId(100 + id),
      name,
      status: 'Open',
      outcome: null,
      originMessageId,
      messageCount: 1,
      unread: false,
      archived: false,
      lastActivityAt: '2026-10-01T10:00:00Z',
      statusChangedAt: null,
    })
    const boardKey = threadsQueryKey(projectId, conversationId)
    const otherBoardKey = threadsQueryKey(projectId, projectId)
    const sidebarKey = queryKeys.conversationSidebar(projectId, conversationId, `expanded:${conversationId}`)
    queryClient.setQueryData(boardKey, [thread(1, 'ghp_secret', messageId(1)), thread(2, 'Deploy plan', messageId(2))])
    const otherBoard = [thread(3, 'Elsewhere', messageId(3))]
    queryClient.setQueryData(otherBoardKey, otherBoard)
    queryClient.setQueryData(sidebarKey, {
      projectId,
      channels: [
        {
          id: conversationId,
          name: 'general',
          threads: [
            { id: messageId(101), parentChannelId: conversationId, originMessageId: messageId(1), name: 'ghp_secret' },
            { id: messageId(104), parentChannelId: conversationId, originMessageId: null, name: 'Standup' },
          ],
        },
      ],
      capabilities: { createChannel: true },
    })
    const otherUpdatedAt = queryClient.getQueryState(otherBoardKey)?.dataUpdatedAt

    dropRemovedMessage(queryClient, { conversationId, messageId: messageId(1) })

    const board = queryClient.getQueryData<{ name: string }[]>(boardKey)
    expect(board?.map((item) => item.name)).toEqual([NEUTRAL_THREAD_NAME, 'Deploy plan'])
    const sidebar = JSON.stringify(queryClient.getQueryData(sidebarKey))
    expect(sidebar).not.toContain('ghp_secret')
    expect(sidebar).toContain('Standup')
    // Caches with nothing to change keep their data and their freshness.
    expect(queryClient.getQueryData(otherBoardKey)).toBe(otherBoard)
    expect(queryClient.getQueryState(otherBoardKey)?.dataUpdatedAt).toBe(otherUpdatedAt)

    // Boards on no screen are forgotten, so a return refetches names.
    refreshMemoryCaches(queryClient)
    expect(queryClient.getQueryData(boardKey)).toBeUndefined()
  })

  it('turns a message back into composer text that rebuilds the same content', () => {
    const agent: ConversationActor = { id: projectId, kind: 'external_agent', displayName: 'Codex', handle: 'codex', avatarUrl: null }
    const workItem = {
      id: messageId(7),
      key: 'AW-12',
      title: 'Ship it',
      statusCategory: 'todo',
      archivedAt: null,
      href: `/projects/${projectId}/work-items/AW-12`,
    }
    const content = {
      version: 1 as const,
      nodes: [
        { type: 'text' as const, text: 'Ask ' },
        { type: 'mention' as const, actorId: agent.id },
        { type: 'text' as const, text: ' about ' },
        { type: 'work_item_ref' as const, workItemId: workItem.id },
        { type: 'text' as const, text: ' today' },
      ],
    }
    const draft = draftFromContent({ content, mentionedActors: [agent], referencedWorkItems: [workItem] })
    expect(draft.text).toBe('Ask @codex about AW-12 today')
    expect(buildMessageContent(draft.text, draft.mentions, draft.workItems)).toEqual(content)
  })
})
