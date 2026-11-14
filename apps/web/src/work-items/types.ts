import type { FieldReference, FieldValue, FieldValues } from '../fields/fieldModel'

export type WorkItemCapabilities = {
  view: boolean
  edit: boolean
  archive: boolean
  /** Manages the project's code, so pull request cards offer Merge. */
  mergePullRequests?: boolean
}

export type WorkItemType = { id: string; name: string }

export type WorkItemStatus = {
  id: string
  name: string
  semanticCategory: string
}

export type WorkItemAssignee = {
  actorId: string
  displayName: string
  handle: string
  kind: string
  active: boolean
}

/** A label on a work item. `color` is always `#rrggbb`. */
export type WorkItemLabel = { id: string; name: string; color: string }

export type AssignableActor = {
  id: string
  displayName: string
  handle: string
  kind: string
}

export type WorkItem = {
  id: string
  projectId: string
  key: string
  number: number
  title: string
  description: string
  itemType: WorkItemType
  status: WorkItemStatus
  priority: WorkItemPriority
  assignee: WorkItemAssignee | null
  version: number
  archivedAt: string | null
  createdAt: string
  updatedAt: string
  labels: WorkItemLabel[]
  acceptanceCriteria: AcceptanceCriterion[]
  linkedThreads: LinkedThread[]
  linkedRepositories: LinkedRepository[]
  linkedPullRequests: LinkedPullRequest[]
  capabilities: WorkItemCapabilities
  parent: TaskRef | null
  milestone: MilestoneRef | null
  startDate: string | null
  dueDate: string | null
  estimate: number | null
  blocked: { reason: string; at: string; by: ActorRef | null } | null
  blockedBy: TaskRef[]
  blocks: TaskRef[]
  ready: boolean
  relations: TaskRelation[]
  subtasks: Array<TaskRef & { assignee: ActorRef | null }>
  claim: TaskClaim | null
  /** The last note left when someone stopped working on it: where it got to and what's left. */
  handoff: TaskHandoff | null
  watchers: ActorRef[]
  watching: boolean
  estimateUnit: EstimateUnit
  /** Custom field values by field ID (AWD-89). */
  fieldValues: FieldValues
  /** The people, tasks and pages those values name. */
  fieldReferences: FieldReference[]
  /** The collection that acts like a task it's a record of (AWD-110), whose fields it has; null for a plain task. */
  collectionId?: string | null
}

export type EstimateUnit = 'points' | 'hours'

export type ActorRef = {
  id: string
  displayName: string
  handle: string
  kind: string
  avatarUrl: string | null
}

/** A small reference to another task, possibly in another project the viewer can see. */
export type TaskRef = {
  id: string
  projectId: string
  key: string
  title: string
  priority: WorkItemPriority
  statusName: string
  statusCategory: string
  finished: boolean
  archived: boolean
  href: string
}

export type MilestoneRef = { id: string; name: string; targetDate: string | null }

export type TaskRelationKind = 'relates_to' | 'duplicates' | 'duplicated_by'

export type TaskRelation = { kind: TaskRelationKind; task: TaskRef }

export type TaskClaim = { actor: ActorRef; note: string | null; claimedAt: string; expiresAt: string }

export type TaskHandoff = { actor: ActorRef; note: string; at: string; via: string | null }

export type LinkedThread = {
  id: string
  name: string
  parentChannelId: string
  parentChannelName: string
  archivedAt: string | null
  newestRunState: string | null
  href: string
}

export type LinkedRepository = {
  id: string
  owner: string
  name: string
  cloneUrl: string
  role: 'primary' | 'related'
  preparationState: string
  trusted: boolean
  available: boolean
  archivedAt: string | null
  href: string
}

export type LinkedPullRequest = {
  id: string
  number: number
  title: string
  url: string
  state: string
  draft: boolean
  source: string
  confirmed: boolean
  checksState: string
  reviewState: string
  mergeabilityState: string
  syncState: string
  syncFailureCategory: string | null
  automationState: string
  automationReason: string | null
  headRef: string
  /** The head commit last read from GitHub; Merge merges only this one. */
  headSha: string
  baseRef: string
  authorLogin: string
  requestedReviewers: string[]
  /** The checks on its head commit, failing ones first (at most 30). */
  checks: PullRequestCheck[]
}

export type PullRequestCheck = {
  name: string
  /** queued, in_progress or completed. */
  status: string
  /** success, failure, neutral, skipped, cancelled, timed_out… once completed. */
  conclusion: string | null
  url: string | null
}

export type AcceptanceCriterion = {
  id: string
  text: string
  position: number
  completed: boolean
  completedBy: AssignableActor | null
  completedAt: string | null
  createdAt: string
  updatedAt: string
}

export type WorkItemCard = Pick<
  WorkItem,
  | 'id'
  | 'projectId'
  | 'key'
  | 'number'
  | 'title'
  | 'itemType'
  | 'status'
  | 'priority'
  | 'assignee'
  | 'version'
  | 'archivedAt'
  | 'createdAt'
  | 'updatedAt'
  | 'labels'
  | 'capabilities'
  | 'milestone'
  | 'startDate'
  | 'dueDate'
  | 'estimate'
  | 'ready'
  | 'fieldValues'
  | 'fieldReferences'
> & {
  parentId: string | null
  blockedReason: string | null
  openBlockerCount: number
  subtaskCount: number
  subtasksDone: number
  claimedBy: ActorRef | null
  /** Manual order (status, then rank) as a string that sorts the same way. Boards not grouped by status keep it. */
  boardOrder: string
  /** Which group the row is in, when the list was grouped. */
  group?: { key: string; label: string }
}

export type WorkflowType = WorkItemType & {
  position: number
  isDefault: boolean
}

export type WorkflowStatus = WorkItemStatus & {
  position: number
  isDefault: boolean
}

export type WorkItemWorkflow = {
  types: Array<WorkflowType & { taskCount: number }>
  statuses: Array<WorkflowStatus & { taskCount: number }>
  transitions: Array<{ fromStatusId: string; toStatusId: string }>
  canEdit: boolean
  version: number
  estimateUnit: EstimateUnit
}

export type WorkItemPriority =
  | 'no_priority'
  | 'low'
  | 'medium'
  | 'high'
  | 'urgent'

/** A custom field sorts as `field:<id>`, or `field:<id>:desc`. */
export type WorkItemSort = 'newest' | 'oldest' | 'updated' | 'priority' | 'due' | 'title' | 'manual' | `field:${string}`

/** A custom field (choice, person or checkbox) groups as `field:<id>`. */
export type WorkItemGroupBy = 'status' | 'assignee' | 'priority' | 'label' | 'milestone' | 'parent' | 'type' | `field:${string}`

/** A single-choice custom field makes columns as `field:<id>` (AWD-103). */
export type BoardColumnsBy = 'status' | 'assignee' | 'priority' | 'milestone' | 'type' | 'label' | `field:${string}`

/**
 * Query parameters for GET /work-items, /work-items/groups and /work-items/board.
 * Most filters take several values, comma separated (any of them), with a
 * `not…` twin (none of them); "none" stands for no value where a task can
 * have none.
 */
export type WorkItemFilters = {
  q?: string
  typeId?: string
  notTypeId?: string
  statusId?: string
  notStatusId?: string
  semanticCategory?: string
  notSemanticCategory?: string
  priority?: string
  notPriority?: string
  assigneeActorId?: string
  notAssigneeActorId?: string
  /** Older links' "no one"; read as the assignee "none". */
  unassigned?: boolean
  archived?: boolean
  /** All of these labels, or "none" alone for no labels. */
  labelIds?: string
  anyLabelIds?: string
  notLabelIds?: string
  milestoneId?: string
  notMilestoneId?: string
  parentId?: string
  blocked?: 'true' | 'false'
  ready?: boolean
  claimed?: 'true' | 'false'
  dueBefore?: string
  dueAfter?: string
  overdue?: boolean
  includeDone?: 'false'
  /** Custom field filters: a JSON array of `FieldClause`s, all of which must match. */
  fieldFilters?: string
  sort?: WorkItemSort
  groupBy?: WorkItemGroupBy
  cursor?: string
  limit?: number
}

export type WorkItemListResponse = {
  items: WorkItemCard[]
  nextCursor: string | null
  total: number
}

export type WorkItemGroup = { key: string; label: string; count: number; color?: string }

export type BoardColumn = {
  key: string
  label: string
  /** A choice option's colour, when the columns are a field's options. */
  color?: string
  status: WorkItemStatus | null
  items: WorkItemCard[]
  hasMore: boolean
  total: number
  nextCursor: string | null
  filter: Record<string, string>
  /** False for a column kept only for the tasks that still carry an archived or removed value. */
  droppable: boolean
}

export type WorkItemBoardResponse = {
  columnsBy: BoardColumnsBy
  columns: BoardColumn[]
}

export type ProjectLabel = {
  id: string
  name: string
  color: string
  description: string
  archived: boolean
  taskCount: number
  version: number
}

export type Milestone = {
  id: string
  name: string
  description: string
  targetDate: string | null
  archived: boolean
  /** Tasks in it, not counting canceled ones. */
  total: number
  /** Completed tasks. */
  finished: number
  open: number
  /** Canceled tasks, left out of total and progress. */
  canceled: number
  progress: number
  overdue: boolean
  version: number
}

export type BulkChanges = {
  statusId?: string
  assigneeActorId?: string | null
  priority?: WorkItemPriority
  milestoneId?: string | null
  addLabelIds?: string[]
  removeLabelIds?: string[]
  archived?: boolean
  dueDate?: string | null
  /** Custom fields by ID; null clears one (AWD-101). */
  fieldValues?: Record<string, FieldValue | null>
}

export type BulkProblem = { id: string; key: string | null; code: string; message: string }

export type WorkflowDraftStatus = { id?: string; key?: string; name: string; semanticCategory: string; isDefault: boolean }
export type WorkflowDraftType = { id?: string; key?: string; name: string; isDefault: boolean }

export type WorkflowUpdate = {
  expectedVersion: number
  estimateUnit?: EstimateUnit
  statuses: WorkflowDraftStatus[]
  types: WorkflowDraftType[]
  transitions: 'all' | Array<{ from: string; to: string }>
  replacements?: Record<string, string>
}

export type WorkItemPatch = {
  expectedVersion: number
  title?: string
  description?: string
  typeId?: string
  statusId?: string
  priority?: WorkItemPriority
  assigneeActorId?: string | null
  parentWorkItemId?: string | null
  milestoneId?: string | null
  startDate?: string | null
  dueDate?: string | null
  estimate?: number | null
  blockedReason?: string | null
  addLabelIds?: string[]
  removeLabelIds?: string[]
  /** Custom field values by field ID; null clears one. */
  fieldValues?: Record<string, FieldValue | null>
}

/** One field's before and after in a history entry. Ids are already display text. A custom field's carries its name. */
export type FieldChange = { from: string | number | null; to: string | number | null; label?: string }

export type DescriptionChange = { diff: string; truncated: boolean }

export type ActivityChanges = Record<string, FieldChange | DescriptionChange>

export type WorkItemTimelineEntry = {
  sequence: number
  kind: 'activity' | 'comment'
  createdAt: string
  activity: {
    id: string
    actor: AssignableActor
    kind: string
    summary: string
    /** The connected app it was done through, when the actor shows it. */
    via?: string | null
    changes?: ActivityChanges | null
  } | null
  comment: {
    id: string
    author: AssignableActor
    content: MessageContentV1
    plainText: string
    mentionedActors: AssignableActor[]
    referencedWorkItems: ConversationWorkItemReference[]
    createdAt: string
    /** The connected app it was written through, when the author shows it. */
    via?: string | null
    /** 1 until it's edited; an edit saves against the revision it opened at. */
    revision: number
    editedAt: string | null
    /** Set once it's deleted; its text is gone then. */
    removed: { removedAt: string; removedBy: AssignableActor | null } | null
    /** The comment on this task it answers, read live: no text once that one is deleted. */
    replyTo: { commentId: string; authorName: string | null; text: string | null; removed: boolean } | null
  } | null
}

export type CommentRevision = {
  revision: number
  /** Null once the comment was deleted. */
  content: MessageContentV1 | null
  plainText: string | null
  writtenBy: AssignableActor
  writtenAt: string
  redacted: boolean
}

export type CommentRevisions = {
  commentId: string
  revision: number
  removed: boolean
  /** Oldest first; empty for a comment never edited. */
  revisions: CommentRevision[]
}

export type WorkWithWorkerResponse = {
  threadId: string
  run: {
    runId: string
    state: string
    repositoryId: string
  }
}
import type { ConversationWorkItemReference, MessageContentV1 } from '../conversations/types'
