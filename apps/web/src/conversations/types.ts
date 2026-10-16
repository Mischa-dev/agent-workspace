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
  /** The connected app a person sent this through, such as Claude Code. */
  via: string | null
  /** 1 until the message is edited; each edit adds one. */
  revision: number
  editedAt: string | null
  /** Set once the message is removed. Its content and text are then empty. */
  removed: MessageRemoval | null
  /** Emoji reactions in the order first added. Always empty once removed. */
  reactions?: MessageReaction[]
}

/** One emoji on a message: how many reacted, whether you did, and the first 20 who did. */
export type MessageReaction = {
  emoji: string
  /** The name the server uses for it, such as `eyes`. */
  name: string
  count: number
  byViewer: boolean
  actors: ConversationActor[]
}

export type MessageRemoval = {
  removedAt: string
  /** Null until the server confirms who removed it (a live removal seen before the refetch). */
  removedBy: ConversationActor | null
  via: string | null
}

/** One version of a message, as the history shows it. */
export type MessageRevision = {
  revision: number
  /** Null once the message was removed: every version's text is deleted. */
  content: MessageContentV1 | null
  plainText: string | null
  writtenBy: ConversationActor
  writtenAt: string
  via: string | null
  redacted: boolean
}

export type MessageRevisions = {
  messageId: string
  revision: number
  removed: boolean
  revisions: MessageRevision[]
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
    /** Threads: where it stands, such as Open or Decided. */
    threadStatus: string | null
    threadOutcome: string | null
    /** Threads: the kind of conversation it lives under. */
    parentKind: ConversationKind | null
  }
  linkedWorkItems: ConversationWorkItemReference[]
  entries: ConversationTimelineEntry[]
  nextCursor: string | null
  highestSequence: number
  readState: {
    highestSeenSequence: number
    version: number
  }
  capabilities: ConversationCapabilities
  /** Which agents have been asked to answer, and how it's going. A thread also
   * lists the channel tags answered in it. */
  replyRequests: ReplyRequest[]
  /** Groups: agent messages left before a person has to say Continue. */
  agentTurnsRemaining: number | null
}

export type ConversationCapabilities = {
  view: boolean
  sendMessage: boolean
  archive: boolean
  markSeen: boolean
  /** Edit your own messages here (the same right as sending). */
  editMessages: boolean
  /** Remove your own messages, also once the conversation is archived. */
  removeOwnMessages: boolean
  /** Remove anyone's message (project owners and admins). */
  removeAnyMessage: boolean
}

export type ReplyRequestStatus =
  | 'pending'
  | 'claimed'
  | 'replied'
  | 'passed'
  | 'released'
  | 'expired'
  | 'cancelled'

export type ReplyRequest = {
  id: string
  messageId: string
  agent: ConversationActor
  required: boolean
  status: ReplyRequestStatus
  reason: string | null
  /** What the agent says it is doing, while it holds the claim. */
  progress?: string | null
  progressAt?: string | null
  updatedAt: string
  agentLastSeenAt: string | null
  /** A channel tag is answered in this thread, once the agent picks it up. */
  threadId?: string | null
  /** A follow-up queued behind the request the agent is working on. */
  followsRequestId?: string | null
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

/** A thread as it appears on a conversation's thread board. */
export type ThreadSummary = {
  id: string
  name: string
  status: string
  outcome: string | null
  originMessageId: string | null
  messageCount: number
  unread: boolean
  archived: boolean
  lastActivityAt: string
  statusChangedAt: string | null
}
