import { requestJson } from '../api/client'
import {
  isSafeMessageText,
  parseMessageContent,
} from './messageContentModel'
import type {
  ConversationActor,
  ConversationMessage,
  ConversationTimelineEntry,
  ConversationTimelinePage,
  ConversationWorkItemReference,
  MentionSearchPage,
  MessageContentV1,
  ProjectConversationSidebar,
  SidebarChannel,
  SidebarThread,
} from './types'

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as WireRecord)
    : {}
}

function string(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

function nullableString(value: unknown) {
  return typeof value === 'string' ? value : null
}

function integer(value: unknown, fallback = 0) {
  return Number.isSafeInteger(value) ? (value as number) : fallback
}

function capability(value: unknown, name: string) {
  return asRecord(value)[name] === true
}

export function normalizeConversationActor(
  value: unknown,
): ConversationActor | null {
  const actor = asRecord(value)
  const kind = actor.kind
  if (
    typeof actor.id !== 'string' ||
    ![
      'human',
      'external_agent',
      'execution_worker',
      'integration',
      'system',
    ].includes(String(kind)) ||
    typeof actor.displayName !== 'string' ||
    typeof actor.handle !== 'string'
  ) {
    return null
  }
  return {
    id: actor.id,
    kind: kind as ConversationActor['kind'],
    displayName: actor.displayName.slice(0, 120),
    handle: actor.handle.slice(0, 64),
    avatarUrl: nullableString(actor.avatarUrl),
  }
}

function normalizeWorkItemReference(
  value: unknown,
): ConversationWorkItemReference | null {
  const item = asRecord(value)
  if (
    typeof item.id !== 'string' ||
    typeof item.key !== 'string' ||
    typeof item.title !== 'string' ||
    typeof item.statusCategory !== 'string' ||
    typeof item.href !== 'string' ||
    !item.href.startsWith('/projects/')
  ) {
    return null
  }
  return {
    id: item.id,
    key: item.key.slice(0, 64),
    title: item.title.slice(0, 240),
    statusCategory: item.statusCategory.slice(0, 40),
    archivedAt: nullableString(item.archivedAt),
    href: item.href,
  }
}

function normalizeMessage(value: unknown): ConversationMessage | null {
  const message = asRecord(value)
  const author = normalizeConversationActor(message.author)
  const content = parseMessageContent(message.content)
  if (
    typeof message.id !== 'string' ||
    !author ||
    !content ||
    typeof message.plainText !== 'string' ||
    !isSafeMessageText(message.plainText)
  ) {
    return null
  }
  return {
    id: message.id,
    author,
    content,
    plainText: message.plainText,
    mentionedActors: Array.isArray(message.mentionedActors)
      ? message.mentionedActors
          .map(normalizeConversationActor)
          .filter((actor): actor is ConversationActor => actor !== null)
      : [],
    referencedWorkItems: Array.isArray(message.referencedWorkItems)
      ? message.referencedWorkItems
          .map(normalizeWorkItemReference)
          .filter(
            (item): item is ConversationWorkItemReference => item !== null,
          )
      : [],
    createdAt: string(message.createdAt),
  }
}

function normalizeTimelineEntry(
  value: unknown,
): ConversationTimelineEntry | null {
  const entry = asRecord(value)
  const kind = entry.kind
  if (
    typeof entry.id !== 'string' ||
    !Number.isSafeInteger(entry.sequence) ||
    (entry.sequence as number) < 1 ||
    (kind !== 'message' && kind !== 'worker_event' && kind !== 'system')
  ) {
    return null
  }
  const message = entry.message ? normalizeMessage(entry.message) : null
  const worker = asRecord(entry.workerEvent)
  const workerEvent =
    typeof worker.id === 'string' &&
    typeof worker.agentRunId === 'string' &&
    typeof worker.kind === 'string' &&
    typeof worker.summary === 'string'
      ? {
          id: worker.id,
          agentRunId: worker.agentRunId,
          kind: worker.kind.slice(0, 120),
          summary: worker.summary.slice(0, 2_000),
          createdAt: string(worker.createdAt, string(entry.createdAt)),
        }
      : null
  if ((kind === 'message' && !message) || (kind === 'worker_event' && !workerEvent)) {
    return null
  }
  return {
    id: entry.id,
    sequence: entry.sequence as number,
    kind,
    createdAt: string(entry.createdAt),
    message,
    workerEvent,
  }
}

export function normalizeTimelinePage(value: unknown): ConversationTimelinePage {
  const root = asRecord(value)
  const conversation = asRecord(root.conversation)
  const readState = asRecord(root.readState)
  const kind = conversation.kind
  if (
    typeof conversation.id !== 'string' ||
    typeof conversation.projectId !== 'string' ||
    (kind !== 'channel' && kind !== 'thread')
  ) {
    throw new Error('Timeline response is missing its Conversation boundary.')
  }
  const entries = Array.isArray(root.entries)
    ? root.entries
        .map(normalizeTimelineEntry)
        .filter((entry): entry is ConversationTimelineEntry => entry !== null)
        .sort((left, right) => left.sequence - right.sequence)
    : []
  return {
    conversation: {
      id: conversation.id,
      projectId: conversation.projectId,
      kind,
      parentChannelId: nullableString(conversation.parentChannelId),
      originMessageId: nullableString(conversation.originMessageId),
      name: string(conversation.name, kind === 'channel' ? 'Channel' : 'Thread'),
      archivedAt: nullableString(conversation.archivedAt),
      version: Math.max(1, integer(conversation.version, 1)),
    },
    entries,
    nextCursor: nullableString(root.nextCursor),
    highestSequence: Math.max(0, integer(root.highestSequence)),
    readState: {
      highestSeenSequence: Math.max(
        0,
        integer(readState.highestSeenSequence),
      ),
      version: Math.max(0, integer(readState.version)),
    },
    capabilities: {
      view: capability(root.capabilities, 'view'),
      sendMessage: capability(root.capabilities, 'sendMessage'),
      archive: capability(root.capabilities, 'archive'),
      markSeen: capability(root.capabilities, 'markSeen'),
    },
  }
}

export async function getConversationTimeline(
  conversationId: string,
  cursor?: string,
  limit = 50,
  includeArchived = false,
) {
  const parameters = new URLSearchParams({ limit: String(limit) })
  if (cursor) parameters.set('cursor', cursor)
  if (includeArchived) parameters.set('includeArchived', 'true')
  return normalizeTimelinePage(
    await requestJson<unknown>(
      `/api/v1/conversations/${encodeURIComponent(conversationId)}/timeline?${parameters}`,
    ),
  )
}

export async function sendConversationMessage(input: {
  conversationId: string
  kind: 'channel' | 'thread'
  content: MessageContentV1
  idempotencyKey: string
}) {
  const collection = input.kind === 'channel' ? 'channels' : 'threads'
  return requestJson<unknown>(
    `/api/v1/${collection}/${encodeURIComponent(input.conversationId)}/messages`,
    {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: { content: input.content },
    },
  )
}

export async function createChannel(input: {
  projectId: string
  name: string
  idempotencyKey: string
}) {
  const payload = asRecord(await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(input.projectId)}/channels`,
    {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: { name: input.name },
    },
  ))
  const conversation = asRecord(payload.conversation ?? payload.channel ?? payload)
  if (typeof conversation.id !== 'string') {
    throw new Error('Created Channel response is malformed.')
  }
  return { id: conversation.id, name: string(conversation.name, input.name) }
}

export async function createThread(input: {
  channelId: string
  name: string
  originMessageId?: string
  idempotencyKey: string
}) {
  const payload = asRecord(await requestJson<unknown>(
    `/api/v1/channels/${encodeURIComponent(input.channelId)}/threads`,
    {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: {
        name: input.name,
        ...(input.originMessageId
          ? { originMessageId: input.originMessageId }
          : {}),
      },
    },
  ))
  const conversation = asRecord(payload.conversation ?? payload.thread ?? payload)
  if (typeof conversation.id !== 'string') {
    throw new Error('Created Thread response is malformed.')
  }
  return { id: conversation.id, name: string(conversation.name, input.name) }
}

export async function archiveConversation(input: {
  conversationId: string
  kind: 'channel' | 'thread'
  expectedVersion: number
}) {
  return requestJson<unknown>(
    `/api/v1/${input.kind === 'channel' ? 'channels' : 'threads'}/${encodeURIComponent(input.conversationId)}/archive`,
    {
      method: 'POST',
      json: { expectedVersion: input.expectedVersion },
    },
  )
}

export async function markConversationSeen(input: {
  conversationId: string
  highestSeenSequence: number
}) {
  return requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(input.conversationId)}/mark-seen`,
    {
      method: 'POST',
      json: { highestSeenSequence: input.highestSeenSequence },
    },
  )
}

export async function searchMentionableActors(
  projectId: string,
  query: string,
): Promise<MentionSearchPage> {
  const parameters = new URLSearchParams({ q: query, limit: '20' })
  const root = asRecord(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/mentionable-actors?${parameters}`,
    ),
  )
  return {
    actors: Array.isArray(root.actors)
      ? root.actors
          .map(normalizeConversationActor)
          .filter((actor): actor is ConversationActor => actor !== null)
      : [],
    nextCursor: nullableString(root.nextCursor),
  }
}

function normalizeSidebarThread(value: unknown): SidebarThread | null {
  const item = asRecord(value)
  const attention = asRecord(item.attention)
  const attentionKind = attention.kind
  if (
    typeof item.id !== 'string' ||
    typeof item.parentChannelId !== 'string' ||
    typeof item.name !== 'string' ||
    ![
      'response_required',
      'active',
      'failed',
      'completed',
      'unread',
      'settled',
    ].includes(
      String(attentionKind),
    )
  ) {
    return null
  }
  return {
    id: item.id,
    parentChannelId: string(item.parentChannelId),
    originMessageId: nullableString(item.originMessageId),
    name: item.name,
    archivedAt: nullableString(item.archivedAt),
    parentArchivedAt: nullableString(item.parentArchivedAt),
    highestSequence: Math.max(0, integer(item.highestSequence)),
    highestSeenSequence: Math.max(0, integer(item.highestSeenSequence)),
    attention: attentionKind as SidebarThread['attention'],
    attentionLabel: string(attention.label, 'No current attention'),
  }
}

function normalizeSidebarChannel(value: unknown): SidebarChannel | null {
  const item = asRecord(value)
  if (typeof item.id !== 'string' || typeof item.name !== 'string') return null
  return {
    id: item.id,
    name: item.name,
    archivedAt: nullableString(item.archivedAt),
    highestSequence: Math.max(0, integer(item.highestSequence)),
    highestSeenSequence: Math.max(0, integer(item.highestSeenSequence)),
    unread: item.unread === true,
    threads: Array.isArray(item.threads)
      ? item.threads
          .map(normalizeSidebarThread)
          .filter((thread): thread is SidebarThread => thread !== null)
      : [],
    hasMoreThreads:
      item.hasMoreThreads === true || integer(item.hiddenThreadCount) > 0,
  }
}

export async function getProjectConversationSidebar(
  projectId: string,
  selectedChannelId?: string,
  includeArchived = false,
  expandedChannelId?: string,
): Promise<ProjectConversationSidebar> {
  const parameters = new URLSearchParams()
  if (selectedChannelId) parameters.set('selectedChannelId', selectedChannelId)
  if (includeArchived) parameters.set('includeArchived', 'true')
  if (expandedChannelId) parameters.set('expandedChannelId', expandedChannelId)
  const suffix = parameters.size ? `?${parameters}` : ''
  const root = asRecord(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/conversation-sidebar${suffix}`,
    ),
  )
  return {
    projectId: string(root.projectId, projectId),
    channels: Array.isArray(root.channels)
      ? root.channels
          .map(normalizeSidebarChannel)
          .filter((channel): channel is SidebarChannel => channel !== null)
      : [],
    capabilities: {
      createChannel: capability(root.capabilities, 'createChannel'),
    },
  }
}
