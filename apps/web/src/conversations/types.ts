export type MessageTextNode = {
  type: 'text'
  text: string
}

export type MessageMentionNode = {
  type: 'mention'
  actorId: string
}

export type MessageWorkItemReferenceNode = {
  type: 'work_item_ref'
  workItemId: string
}

export type MessageNode =
  | MessageTextNode
  | MessageMentionNode
  | MessageWorkItemReferenceNode

export type MessageContentV1 = {
  version: 1
  nodes: MessageNode[]
}

export type ConversationActor = {
  id: string
  kind: 'human' | 'external_agent' | 'execution_worker' | 'integration' | 'system'
  displayName: string
  handle: string
  avatarUrl: string | null
}

export type ConversationMessage = {
  id: string
  author: ConversationActor
  content: MessageContentV1
  plainText: string
  mentionedActors: ConversationActor[]
  referencedWorkItems: ConversationWorkItemReference[]
  createdAt: string
}

export type ConversationWorkItemReference = {
  id: string
  key: string
  title: string
  statusCategory: string
  archivedAt: string | null
  href: string
}

export type ConversationWorkerEvent = {
  id: string
  agentRunId: string
  kind: string
  summary: string
  createdAt: string
}

export type ConversationTimelineEntry = {
  id: string
  sequence: number
  kind: 'message' | 'worker_event' | 'system'
  createdAt: string
  message: ConversationMessage | null
  workerEvent: ConversationWorkerEvent | null
}

export type ConversationKind = 'channel' | 'thread' | 'dm' | 'group'

export type ConversationTimelinePage = {
  conversation: {
    id: string
    projectId: string
    kind: ConversationKind
    parentChannelId: string | null
    originMessageId: string | null
    name: string
    archivedAt: string | null
    version: number
  }
  linkedWorkItems: ConversationWorkItemReference[]
  entries: ConversationTimelineEntry[]
  nextCursor: string | null
  highestSequence: number
  readState: {
    highestSeenSequence: number
    version: number
  }
  capabilities: {
    view: boolean
    sendMessage: boolean
    archive: boolean
    markSeen: boolean
  }
  /** DMs and groups: who has been asked to answer, and how it's going. */
  replyRequests: ReplyRequest[]
  /** Groups: agent messages left before a person has to say Continue. */
  agentTurnsRemaining: number | null
}

export type ReplyRequestStatus =
  | 'pending'
  | 'claimed'
  | 'replied'
  | 'passed'
  | 'released'
  | 'expired'

export type ReplyRequest = {
  id: string
  messageId: string
  agent: ConversationActor
  required: boolean
  status: ReplyRequestStatus
  reason: string | null
  updatedAt: string
  agentLastSeenAt: string | null
}

export type MentionSearchPage = {
  actors: ConversationActor[]
  nextCursor: string | null
}

export type SidebarThread = {
  id: string
  parentChannelId: string
  originMessageId: string | null
  name: string
  archivedAt: string | null
  parentArchivedAt: string | null
  highestSequence: number
  highestSeenSequence: number
  attention:
    | 'response_required'
    | 'active'
    | 'failed'
    | 'completed'
    | 'unread'
    | 'settled'
  attentionLabel: string
}

export type SidebarChannel = {
  id: string
  name: string
  archivedAt: string | null
  highestSequence: number
  highestSeenSequence: number
  unread: boolean
  threads: SidebarThread[]
  hasMoreThreads: boolean
}

export type ProjectConversationSidebar = {
  projectId: string
  channels: SidebarChannel[]
  capabilities: {
    createChannel: boolean
  }
}
