// The conversation calls every page needs (the sidebar, live updates, the
// thread list under a channel), apart from ./api so they load with the app
// shell while the rest loads with the conversation pages. ./api re-exports
// them; import from here only in code the shell loads.
import { requestJson } from '../api/client'
import type {
  ConversationActor,
  MessageContentV1,
  ProjectConversationSidebar,
  SidebarChannel,
  SidebarThread,
  ThreadSummary,
} from './types'

type WireRecord = Record<string, unknown>

export function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as WireRecord)
    : {}
}

export function string(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

export function nullableString(value: unknown) {
  return typeof value === 'string' ? value : null
}

export function integer(value: unknown, fallback = 0) {
  return Number.isSafeInteger(value) ? (value as number) : fallback
}

export function capability(value: unknown, name: string) {
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

/** The empty content a removed message carries. */
export const REMOVED_MESSAGE_CONTENT: MessageContentV1 = { version: 1, nodes: [] }

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
    hiddenThreadCount: Math.max(0, integer(item.hiddenThreadCount)),
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
    participants: Array.isArray(item.participants)
      ? item.participants.flatMap((value) => {
          const actor = asRecord(value)
          return typeof actor.id === 'string' && typeof actor.displayName === 'string'
            ? [{ id: actor.id, kind: string(actor.kind, 'human'), displayName: actor.displayName }]
            : []
        })
      : [],
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
