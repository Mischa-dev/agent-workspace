export type WorkItemCapabilities = {
  view: boolean
  edit: boolean
  archive: boolean
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
  acceptanceCriteria: AcceptanceCriterion[]
  capabilities: WorkItemCapabilities
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

export type WorkItemCard = Omit<
  WorkItem,
  'description' | 'createdAt' | 'acceptanceCriteria'
>

export type WorkflowType = WorkItemType & {
  position: number
  isDefault: boolean
}

export type WorkflowStatus = WorkItemStatus & {
  position: number
  isDefault: boolean
}

export type WorkItemWorkflow = {
  types: WorkflowType[]
  statuses: WorkflowStatus[]
  transitions: Array<{ fromStatusId: string; toStatusId: string }>
}

export type WorkItemPriority =
  | 'no_priority'
  | 'low'
  | 'medium'
  | 'high'
  | 'urgent'

export type WorkItemFilters = {
  q?: string
  typeId?: string
  statusId?: string
  semanticCategory?: string
  priority?: WorkItemPriority
  assigneeActorId?: string
  unassigned?: boolean
  archived?: boolean
  cursor?: string
  limit?: number
}

export type WorkItemListResponse = {
  items: WorkItemCard[]
  nextCursor: string | null
}

export type WorkItemPatch = {
  expectedVersion: number
  title?: string
  description?: string
  typeId?: string
  statusId?: string
  priority?: WorkItemPriority
  assigneeActorId?: string | null
}

export type WorkItemTimelineEntry = {
  sequence: number
  kind: 'activity' | 'comment'
  createdAt: string
  activity: {
    id: string
    actor: AssignableActor
    kind: string
    summary: string
  } | null
  comment: {
    id: string
    author: AssignableActor
    plainText: string
    createdAt: string
  } | null
}
