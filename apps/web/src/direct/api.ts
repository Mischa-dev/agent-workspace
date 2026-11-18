import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import { useWorkspaceId } from '../app/workspace'
import { normalizeConversationActor } from '../conversations/sidebarApi'
import type { ConversationActor } from '../conversations/types'

export type DirectConversation = {
  id: string
  kind: 'dm' | 'group'
  name: string
  version: number
  members: ConversationActor[]
  highestSequence: number
  unread: boolean
  /** Others' unread messages, at most 99: a DM row shows the number (C2.3). */
  unreadMessages: number
  mentions: number
  /** Set to tell you nothing: still unread, but never bold or counted. */
  muted: boolean
  lastActivityAt: string
  /** Kept by the current workspace rather than the people's own. */
  kept: boolean
  /** Tells apart a pair's DMs when there is more than one in the list. */
  label?: string
}

export type DirectOverview = {
  projectId: string
  /** The workspace the list was asked for: the one the person is in. */
  organizationId: string
  /** That workspace's own space, where an agent added without a project lives. */
  workspaceSpaceId: string
  conversations: DirectConversation[]
  people: ConversationActor[]
  capabilities: { startConversation: boolean; manageAgents: boolean }
}

export const directQueryKey = queryKeys.direct

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as WireRecord)
    : {}
}

function actors(value: unknown): ConversationActor[] {
  return Array.isArray(value)
    ? value
        .map(normalizeConversationActor)
        .filter((actor): actor is ConversationActor => actor !== null)
    : []
}

function normalizeConversation(value: unknown): DirectConversation | null {
  const record = asRecord(value)
  if (
    typeof record.id !== 'string' ||
    (record.kind !== 'dm' && record.kind !== 'group')
  ) {
    return null
  }
  return {
    id: record.id,
    kind: record.kind,
    name: typeof record.name === 'string' ? record.name : '',
    version: typeof record.version === 'number' ? record.version : 1,
    members: actors(record.members),
    highestSequence:
      typeof record.highestSequence === 'number' ? record.highestSequence : 0,
    unread: record.unread === true,
    unreadMessages: typeof record.unreadMessages === 'number' ? Math.max(0, record.unreadMessages) : 0,
    mentions: typeof record.mentions === 'number' ? Math.max(0, record.mentions) : 0,
    muted: record.muted === true,
    lastActivityAt:
      typeof record.lastActivityAt === 'string' ? record.lastActivityAt : '',
    kept: record.kept === true,
    ...(typeof record.label === 'string' && record.label ? { label: record.label } : {}),
  }
}

/** Names the workspace the person is in: a workspace-level request never leaves the server to guess (K4.6). */
function inWorkspace(path: string, workspaceId: string | null | undefined) {
  return workspaceId ? `${path}?workspaceId=${encodeURIComponent(workspaceId)}` : path
}

export async function getDirectOverview(workspaceId?: string | null): Promise<DirectOverview> {
  const root = asRecord(await requestJson<unknown>(inWorkspace('/api/v1/direct', workspaceId)))
  const capabilities = asRecord(root.capabilities)
  if (typeof root.projectId !== 'string') {
    throw new Error('Direct messages are unavailable.')
  }
  return {
    projectId: root.projectId,
    organizationId: typeof root.organizationId === 'string' ? root.organizationId : '',
    workspaceSpaceId: typeof root.workspaceSpaceId === 'string' ? root.workspaceSpaceId : '',
    conversations: Array.isArray(root.conversations)
      ? root.conversations
          .map(normalizeConversation)
          .filter((item): item is DirectConversation => item !== null)
      : [],
    people: actors(root.people),
    capabilities: {
      startConversation: capabilities.startConversation === true,
      manageAgents: capabilities.manageAgents === true,
    },
  }
}

function conversationId(value: unknown) {
  const id = asRecord(asRecord(value).conversation).id
  if (typeof id !== 'string') throw new Error('Conversation was not created.')
  return id
}

/** Opens the pair's DM, started in the workspace the person is in (a workspace that keeps DMs keeps it). */
export async function openDm(actorId: string, workspaceId?: string | null) {
  return conversationId(
    await requestJson<unknown>('/api/v1/direct/dms', {
      method: 'POST',
      json: workspaceId ? { actorId, workspaceId } : { actorId },
    }),
  )
}

export async function createGroup(input: {
  name?: string
  memberIds: string[]
  idempotencyKey: string
  workspaceId?: string | null
}) {
  return conversationId(
    await requestJson<unknown>('/api/v1/direct/groups', {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: {
        name: input.name?.trim() ? input.name.trim() : null,
        memberIds: input.memberIds,
        ...(input.workspaceId ? { workspaceId: input.workspaceId } : {}),
      },
    }),
  )
}

export async function addGroupMember(conversationId: string, actorId: string) {
  await requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(conversationId)}/members`,
    { method: 'POST', json: { actorId } },
  )
}

export async function removeGroupMember(
  conversationId: string,
  actorId: string,
) {
  await requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(conversationId)}/members/${encodeURIComponent(actorId)}/remove`,
    { method: 'POST' },
  )
}

export function useDirectOverview() {
  const workspaceId = useWorkspaceId()
  return useQuery({
    queryKey: queryKeys.directIn(workspaceId),
    queryFn: () => getDirectOverview(workspaceId),
    staleTime: 15_000,
    retry: false,
  })
}

/**
 * DMs are named after the other person; groups keep their own name. A pair
 * with more than one DM in the list says whose each is.
 */
export function conversationTitle(
  conversation: Pick<DirectConversation, 'kind' | 'name' | 'members'> & { label?: string },
  viewerId: string,
) {
  const suffix = conversation.label ? ` (${conversation.label})` : ''
  if (conversation.kind === 'group') return conversation.name + suffix
  const other = conversation.members.find((member) => member.id !== viewerId)
  // A DM with only yourself in it is your note to self (C2.32).
  if (!other && conversation.members.length > 0) return 'Note to self'
  return (other?.displayName ?? conversation.name) + suffix
}
