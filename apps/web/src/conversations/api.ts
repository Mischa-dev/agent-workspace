import { requestJson } from '../api/client'
import {
  isSafeMessageText,
  parseMessageContent,
} from './messageContentModel'
import type {
  ConversationActor,
  ConversationKind,
  ReplyRequest,
  ReplyRequestStatus,
  ThreadSummary,
  ConversationMessage,
  ConversationTimelineEntry,
  ConversationTimelinePage,
  ConversationWorkItemReference,
  MentionSearchPage,
  MessageContentV1,
  MessageReaction,
  MessageRemoval,
  MessageRevision,
  MessageRevisions,
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

/** The empty content a removed message carries. */
export const REMOVED_MESSAGE_CONTENT: MessageContentV1 = { version: 1, nodes: [] }

function normalizeRemoval(value: unknown): MessageRemoval | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const removal = asRecord(value)
  return {
    removedAt: string(removal.removedAt),
    removedBy: normalizeConversationActor(removal.removedBy),
    via: typeof removal.via === 'string' ? removal.via.slice(0, 80) : null,
  }
}

export function normalizeMessage(value: unknown): ConversationMessage | null {
  const message = asRecord(value)
  const author = normalizeConversationActor(message.author)
  const removed = normalizeRemoval(message.removed)
  if (typeof message.id !== 'string' || !author) return null
  const common = {
    id: message.id,
    author,
    createdAt: string(message.createdAt),
    via: typeof message.via === 'string' ? message.via.slice(0, 80) : null,
    revision: Math.max(1, integer(message.revision, 1)),
    editedAt: nullableString(message.editedAt),
  }
  // A removed message has no nodes, which parseMessageContent rejects. Keep it
  // as a tombstone, and never keep any text that came with it.
  if (removed) {
    return {
      ...common,
      content: REMOVED_MESSAGE_CONTENT,
      plainText: '',
      mentionedActors: [],
      referencedWorkItems: [],
      removed,
      reactions: [],
    }
  }
  const content = parseMessageContent(message.content)
  if (
    !content ||
    typeof message.plainText !== 'string' ||
    !isSafeMessageText(message.plainText)
  ) {
    return null
  }
  return {
    ...common,
    removed: null,
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
    reactions: normalizeReactions(message.reactions),
  }
}

const REACTION_NAME = /^[a-z0-9_]{1,32}$/
const MAX_REACTION_ACTORS = 20

function normalizeReaction(value: unknown): MessageReaction | null {
  const reaction = asRecord(value)
  if (
    typeof reaction.emoji !== 'string' ||
    reaction.emoji.length === 0 ||
    reaction.emoji.length > 16 ||
    typeof reaction.name !== 'string' ||
    !REACTION_NAME.test(reaction.name) ||
    !Number.isSafeInteger(reaction.count) ||
    (reaction.count as number) < 1 ||
    typeof reaction.byViewer !== 'boolean'
  ) {
    return null
  }
  return {
    emoji: reaction.emoji,
    name: reaction.name,
    count: reaction.count as number,
    byViewer: reaction.byViewer,
    actors: Array.isArray(reaction.actors)
      ? reaction.actors
          .slice(0, MAX_REACTION_ACTORS)
          .map(normalizeConversationActor)
          .filter((actor): actor is ConversationActor => actor !== null)
      : [],
  }
}

/** Reactions from the wire; malformed ones are dropped rather than failing the message. */
export function normalizeReactions(value: unknown): MessageReaction[] {
  if (!Array.isArray(value)) return []
  return value.map(normalizeReaction).filter((reaction): reaction is MessageReaction => reaction !== null)
}

export function normalizeTimelineEntry(
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

const conversationKindLabel: Record<ConversationKind, string> = {
  channel: 'Channel',
  thread: 'Thread',
  dm: 'Direct message',
  group: 'Group',
}

export function normalizeTimelinePage(value: unknown): ConversationTimelinePage {
  const root = asRecord(value)
  const conversation = asRecord(root.conversation)
  const readState = asRecord(root.readState)
  const kind = conversation.kind
  if (
    typeof conversation.id !== 'string' ||
    typeof conversation.projectId !== 'string' ||
    (kind !== 'channel' && kind !== 'thread' && kind !== 'dm' && kind !== 'group')
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
      name: string(conversation.name, conversationKindLabel[kind]),
      archivedAt: nullableString(conversation.archivedAt),
      version: Math.max(1, integer(conversation.version, 1)),
      threadStatus: nullableString(conversation.threadStatus),
      threadOutcome: nullableString(conversation.threadOutcome),
      parentKind: ['channel', 'dm', 'group'].includes(String(conversation.parentKind))
        ? (conversation.parentKind as ConversationKind)
        : null,
    },
    linkedWorkItems: Array.isArray(root.linkedWorkItems)
      ? root.linkedWorkItems
          .map(normalizeWorkItemReference)
          .filter((item): item is ConversationWorkItemReference => item !== null)
      : [],
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
      editMessages: capability(root.capabilities, 'editMessages'),
      removeOwnMessages: capability(root.capabilities, 'removeOwnMessages'),
      removeAnyMessage: capability(root.capabilities, 'removeAnyMessage'),
    },
    replyRequests: Array.isArray(root.replyRequests)
      ? root.replyRequests
          .map(normalizeReplyRequest)
          .filter((request): request is ReplyRequest => request !== null)
      : [],
    agentTurnsRemaining: Number.isSafeInteger(root.agentTurnsRemaining)
      ? (root.agentTurnsRemaining as number)
      : null,
  }
}

const replyStatuses: ReplyRequestStatus[] = [
  'pending',
  'claimed',
  'replied',
  'passed',
  'released',
  'expired',
]

function normalizeReplyRequest(value: unknown): ReplyRequest | null {
  const record = asRecord(value)
  const agent = normalizeConversationActor(record.agent)
  if (
    typeof record.id !== 'string' ||
    typeof record.messageId !== 'string' ||
    !agent ||
    !replyStatuses.includes(record.status as ReplyRequestStatus)
  ) {
    return null
  }
  return {
    id: record.id,
    messageId: record.messageId,
    agent,
    required: record.required === true,
    status: record.status as ReplyRequestStatus,
    reason: nullableString(record.reason),
    updatedAt: string(record.updatedAt),
    agentLastSeenAt: nullableString(record.agentLastSeenAt),
  }
}

export async function continueConversation(conversationId: string) {
  await requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(conversationId)}/continue`,
    { method: 'POST' },
  )
}

export async function retryReplyRequest(requestId: string) {
  await requestJson<unknown>(
    `/api/v1/reply-requests/${encodeURIComponent(requestId)}/retry`,
    { method: 'POST' },
  )
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
  kind: ConversationKind
  content: MessageContentV1
  idempotencyKey: string
}) {
  const collection =
    input.kind === 'channel'
      ? 'channels'
      : input.kind === 'thread'
        ? 'threads'
        : 'conversations'
  return requestJson<unknown>(
    `/api/v1/${collection}/${encodeURIComponent(input.conversationId)}/messages`,
    {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: { content: input.content },
    },
  )
}

function messagePath(conversationId: string, messageId: string) {
  return `/api/v1/conversations/${encodeURIComponent(conversationId)}/messages/${encodeURIComponent(messageId)}`
}

/** The entry a message edit or removal returns, checked against the conversation and message asked for. */
function changedEntry(value: unknown, conversationId: string, messageId: string) {
  const root = asRecord(value)
  const entry = normalizeTimelineEntry(root.entry)
  if (
    (typeof root.conversationId === 'string' && root.conversationId !== conversationId) ||
    !entry?.message ||
    entry.message.id !== messageId
  ) {
    throw new Error('The changed message came back malformed. Refresh to see it.')
  }
  return entry
}

/** Edit your own message. The old text stays in its history. */
export async function editConversationMessage(input: {
  conversationId: string
  messageId: string
  content: MessageContentV1
  expectedRevision: number
  idempotencyKey: string
}) {
  const root = asRecord(
    await requestJson<unknown>(messagePath(input.conversationId, input.messageId), {
      method: 'PATCH',
      idempotencyKey: input.idempotencyKey,
      json: { content: input.content, expectedRevision: input.expectedRevision },
    }),
  )
  return {
    entry: changedEntry(root, input.conversationId, input.messageId),
    changed: root.changed !== false,
  }
}

/** Remove a message for everyone. Its text and earlier versions are deleted. */
export async function removeConversationMessage(input: { conversationId: string; messageId: string }) {
  const root = asRecord(
    await requestJson<unknown>(messagePath(input.conversationId, input.messageId), { method: 'DELETE' }),
  )
  return {
    entry: changedEntry(root, input.conversationId, input.messageId),
    alreadyRemoved: root.alreadyRemoved === true,
  }
}

/**
 * Put your reaction on a message, or take it away. It states the result
 * rather than toggling, so a retry is safe. Returns every reaction on the
 * message now.
 */
export async function setMessageReaction(input: {
  conversationId: string
  messageId: string
  name: string
  reacted: boolean
}) {
  if (!REACTION_NAME.test(input.name)) throw new Error('That reaction isn’t one of the choices.')
  const root = asRecord(
    await requestJson<unknown>(
      `${messagePath(input.conversationId, input.messageId)}/reactions/${encodeURIComponent(input.name)}`,
      { method: input.reacted ? 'PUT' : 'DELETE' },
    ),
  )
  if (root.conversationId !== input.conversationId || root.messageId !== input.messageId) {
    throw new Error('The reaction came back for a different message. Refresh to see it.')
  }
  return { changed: root.changed !== false, reactions: normalizeReactions(root.reactions) }
}

/** One timeline entry as it is now, for refreshing a message on an older page. */
export async function getConversationMessage(conversationId: string, messageId: string) {
  return changedEntry(
    await requestJson<unknown>(messagePath(conversationId, messageId)),
    conversationId,
    messageId,
  )
}

function normalizeRevision(value: unknown): MessageRevision | null {
  const record = asRecord(value)
  const writtenBy = normalizeConversationActor(record.writtenBy)
  if (!Number.isSafeInteger(record.revision) || !writtenBy) return null
  const redacted = record.redacted === true
  const content = redacted ? null : parseMessageContent(record.content)
  const plainText =
    !redacted && typeof record.plainText === 'string' && isSafeMessageText(record.plainText)
      ? record.plainText
      : null
  return {
    revision: record.revision as number,
    // A version whose text didn't parse shows as redacted rather than as raw data.
    content,
    plainText: content ? plainText : null,
    writtenBy,
    writtenAt: string(record.writtenAt),
    via: typeof record.via === 'string' ? record.via.slice(0, 80) : null,
    redacted: redacted || !content,
  }
}

/** Every version of a message, oldest first. */
export async function getMessageRevisions(
  conversationId: string,
  messageId: string,
): Promise<MessageRevisions> {
  const root = asRecord(await requestJson<unknown>(`${messagePath(conversationId, messageId)}/revisions`))
  if (typeof root.messageId === 'string' && root.messageId !== messageId) {
    throw new Error('The message history came back for a different message.')
  }
  const removed = root.removed === true
  return {
    messageId,
    revision: Math.max(1, integer(root.revision, 1)),
    removed,
    revisions: Array.isArray(root.revisions)
      ? root.revisions
          .map(normalizeRevision)
          .filter((revision): revision is MessageRevision => revision !== null)
          // A removed message's versions never show text, whatever came back.
          .map((revision) => (removed ? { ...revision, content: null, plainText: null, redacted: true } : revision))
          .sort((left, right) => left.revision - right.revision)
      : [],
  }
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

/** Undo an archive. `expectedVersion` is the version the archive returned. */
export async function restoreConversation(input: {
  conversationId: string
  kind: 'channel' | 'thread'
  expectedVersion: number
}) {
  return requestJson<unknown>(
    `/api/v1/${input.kind === 'channel' ? 'channels' : 'threads'}/${encodeURIComponent(input.conversationId)}/restore`,
    {
      method: 'POST',
      json: { expectedVersion: input.expectedVersion },
    },
  )
}

/** The version a conversation has after an archive or restore, from its response. */
export function conversationVersionFrom(response: unknown, fallback: number) {
  const conversation = response && typeof response === 'object' ? (response as { conversation?: { version?: unknown } }).conversation : undefined
  return typeof conversation?.version === 'number' ? conversation.version : fallback
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

/** Make `messageId` and everything after it unread for you again (never marks anything read). */
export async function markConversationUnread(input: { conversationId: string; messageId: string | null }) {
  return requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(input.conversationId)}/mark-unread`,
    {
      method: 'POST',
      json: { messageId: input.messageId },
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

function normalizeThreadSummary(value: unknown): ThreadSummary | null {
  const item = asRecord(value)
  if (typeof item.id !== 'string' || typeof item.name !== 'string') return null
  return {
    id: item.id,
    name: item.name,
    status: string(item.status, 'Open'),
    outcome: nullableString(item.outcome),
    originMessageId: nullableString(item.originMessageId),
    messageCount: Math.max(0, integer(item.messageCount)),
    unread: item.unread === true,
    archived: item.archived === true,
    lastActivityAt: string(item.lastActivityAt),
    statusChangedAt: nullableString(item.statusChangedAt),
  }
}

/** The threads under a channel, DM, or group, most recent activity first. */
export async function listThreads(conversationId: string): Promise<ThreadSummary[]> {
  const root = asRecord(
    await requestJson<unknown>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/threads`),
  )
  return Array.isArray(root.threads)
    ? root.threads.map(normalizeThreadSummary).filter((thread): thread is ThreadSummary => thread !== null)
    : []
}

export async function updateThread(
  threadId: string,
  patch: { name?: string; status?: string; outcome?: string; clearOutcome?: boolean },
) {
  return requestJson<unknown>(`/api/v1/threads/${encodeURIComponent(threadId)}`, { method: 'PATCH', json: patch })
}
