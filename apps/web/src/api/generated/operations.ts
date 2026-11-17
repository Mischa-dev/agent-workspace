// Generated from the operation registry by `ops::codegen` in the server.
// Do not edit: change the operation, then run
// OPS_BLESS=1 cargo test -p agent-workspace-server --lib generated_files_are_current

export type OpErrorOutcome = 'unauthenticated' | 'not_found' | 'invalid_request' | 'permission_denied' | 'conflict' | 'rate_limited' | 'limit_reached' | 'workspace_suspended' | 'too_large' | 'temporarily_unavailable'

export type OpError = {
  outcome: OpErrorOutcome
  message: string
  retryCategory: string
  field?: string
  code?: string
  module?: string
  details?: Record<string, unknown>
}

export type Envelope<T> =
  | { outcome: 'ok'; value: T; requestId?: string; replayed?: boolean; replaySafe?: boolean; preview?: boolean }
  | OpError

export type AcceptanceCriterionProjection = { completed: boolean; id: string; position: number; text: string }

export type ActivityLine = { kind: string; occurredAt: string; parentRef?: string | null; payload: unknown; seq: number; stepRef?: string | null; summary: string }

export type AgentTemplateView = { definition: unknown; id: string; installId?: string | null; key: string; name: string; purpose: string; spaceId?: string | null }

export type AgentUsageLine = { accountId: string; cachedInputTokens: number; confidence: string; cost?: number | null; currency?: string | null; handle?: string | null; inputTokens: number; name?: string | null; outputTokens: number; runs: number }

export type ArchiveParts = "archive" | "keep"

export type AssigneeProjection = { actorId: string; displayName: string; handle: string }

export type AudienceInput = { accounts?: string[]; roles?: string[] }

export type BlockedProjection = { blockedAt: string; blockedByActorId: string; reason: string }

export type CandidateField = { id: string; name: string; options?: string[]; takesValue: boolean }

export type CarriedValue = { field: string; followsMove?: boolean; targetField: string; taskId: string; taskKey: string; value: unknown }

export type ChangedTask = { finished?: boolean | null; id: string; key: string; status?: string | null; statusGroup?: string | null; title?: string | null }

export type ClaimValue = { claimedAt: string; claimedBy: string; expiresAt: string; handoff?: HandoffNote | null; note?: string | null; taskId: string }

export type ColumnWrite = { field: string; hidden?: boolean; width?: number | null }

export type CommentItem = { at: string; author: string; files?: FileSummary[]; id: string; replyToCommentId?: string | null; text: string; via?: string | null }

export type Content = { body?: Record<string, unknown> | null; key: string; kind: ContentKind; path?: string | null }

export type ContentKind = "switches" | "layout" | "role" | "space" | "channels" | "module" | "collection" | "form" | "automation" | "page" | "skill" | "agent"

export type ConversationSummary = { id: string; kind: string; lastMessage?: LastMessage | null; members?: string | null; name: string; outcome?: string | null; parentId?: string | null; parentName?: string | null; projectId: string; projectName: string; status?: string | null; unreadCount: number; url: string }

export type CreatedTask = { assignee?: string | null; criteriaCount: number; id?: string | null; index: number; key: string; labels?: string[]; parentKey?: string | null; ref?: string | null; status: string; title: string; type: string; url?: string | null; version: number }

export type CriterionOp = "add" | "edit" | "check" | "uncheck" | "remove" | "move"

export type CriterionOperationArgument = { completed?: boolean | null; criterion?: string | null; op: CriterionOp; position?: number | null; text?: string | null }

export type CriterionOperationOutcome = { changed: boolean; criterionId: string; op: CriterionOp }

export type CriterionValue = { completed: boolean; completedAt?: string | null; completedBy?: string | null; id: string; position: number; text: string }

export type DateBucket = "day" | "week" | "month"

export type DecisionLine = { decidedAt: string; decidedBy: string; id: string; title: string }

export type DecisionView = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

export type DependencyChange = { changed: number; key: string; ready?: boolean | null; version: number }

export type DependencyLinks = { blockedBy?: string[] | null; blocks?: string[] | null; taskId: string }

export type DependencyValue = { blockedBy: TaskRef[]; blocks: TaskRef[]; changed?: number; ready?: boolean | null; task: ChangedTask; version?: number }

export type Direction = "asc" | "desc"

export type EditOutcome = { changed: string[]; results: OperationResult[]; savedRevision?: number | null; sceneSeq: number }

export type EntryView = { description: string; entry: string; error?: string | null; id: string; kind: string; name: string; parts: number; shape: boolean; source: string; sourceId?: string | null; target?: string | null; version: string }

export type FieldFilterArgument = { field: string; op: FieldOp; values?: string[] }

export type FieldOp = "is" | "isNot" | "empty" | "notEmpty" | "contains" | "gt" | "gte" | "lt" | "lte" | "within" | "categoryIs" | "categoryIsNot"

export type FieldType = ("text" | "number" | "date" | "single_choice" | "multiple_choice" | "person" | "url" | "checkbox" | "link") | "long_text" | "status" | "people" | "labels" | "email" | "phone" | "created_time" | "edited_time" | "created_by" | "edited_by" | "auto_id"

export type FieldView = { archived: boolean; collection?: string | null; description?: string; id: string; name: string; options?: OptionView[]; position: number; target?: LinkTarget | null; type: FieldType; version: number }

export type FieldWrite = { archived?: boolean | null; description?: string | null; field?: string | null; name?: string | null; options?: OptionInput[] | null; position?: number | null; target?: string | null; type?: string | null }

export type FileSummary = { commentId?: string | null; contentType: string; fileId: string; height?: number | null; idempotentReplay?: boolean; name: string; size: number; width?: number | null }

export type FilterWrite = { field: string; op: FieldOp; values?: string[] }

export type FormFieldWrite = { field: string; required?: boolean }

export type FormWrite = { fields?: FormFieldWrite[]; intro?: string | null; thanks?: string | null }

export type GuidePage = { body: string; pageId: string; stale: boolean; title: string; truncated: boolean; updatedAt: string; version: number }

export type HandoffNote = { at: string; by: string; note: string; via?: string | null }

export type HistoryCaps = { diff: boolean; list: boolean; restore: boolean; showVersion: boolean }

export type HistoryEntry = { actor?: string | null; actorId?: string | null; at: string; changed: string[]; summary: string; version: number; via?: string | null }

export type HostAgentView = { accountId: string; actions: unknown[]; backends: unknown[]; hostAgentRef: string; name?: string | null; status: string }

export type HostOwnerChoice = "person" | "workspace"

export type HostView = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type InboxItem = { conversationId: string; conversationKind: string; conversationName: string; createdAt: string; followsRequestId?: string | null; generation: number; leaseExpiresAt?: string | null; mentioned: boolean; messageAuthor: string; messageExcerpt: string; messageId: string; otherMembers: string; projectId: string; replyConversationId: string; requestId: string; required: boolean; status: string; threadId?: string | null }

export type InlineAutomation = { collection?: string | null; rule: unknown }

export type InlineCollection = { description?: string | null; fields?: FieldWrite[]; name: string; titleName?: string | null; views?: ViewWrite[] }

export type InlineForm = { collection: string; fields?: InlineFormField[]; intro?: string; name: string; thanks?: string }

export type InlineFormField = { field: string; required?: boolean }

export type InstallTarget = { newSpace?: NewSpace | null; spaceId?: string | null }

export type InstallView = { id: string; installedAt: string; name: string; objects: number; packageId: string; spaceId?: string | null; state: string; summary: string[]; version: string }

export type Installed = { changes: string[]; installId: string; objects: InstalledObject[]; packageId: string; skipped: string[]; spaceId?: string | null; version: string }

export type InstalledObject = { action: string; id: string; key: string; kind: string }

export type InviteLinkView = { active: boolean; createdAt: string; createdBy: string; expiresAt: string; id: string; maxUses?: number | null; revokedAt?: string | null; roleId: string; roleName: string; spaceId?: string | null; uses: number }

export type IssueReason = "no_matching_field" | "option_missing" | "person_not_in_target" | "link_outside_move" | "page_link"

export type LabelLine = { archived: boolean; color: string; description?: string; id: string; name: string; taskCount: number; version: number }

export type LastMessage = { at: string; author: string; removed?: boolean; text: string }

export type LayoutLine = { fromWorkspace: boolean; preset: string; tone: string; words?: WordLine[] }

export type Lifecycle = "active" | "importing" | "suspended" | "deleting"

export type LinkArgument = { targetId: string; targetKind: string }

export type LinkTarget = ("task" | "page") | "record"

export type LinkValue = { changed: boolean; key: string; kind: string; linked: boolean; notes?: string[]; role?: string | null; targetId: string; taskId: string; taskUrl: string; version: number }

export type LinkView = { key?: string | null; projectId: string; projectName: string; targetId: string; targetKind: string; title: string; url: string }

export type LinkedItem = { id: string; kind: string; status?: string | null; text: string; title: string }

export type LinkedRepositoryProjection = { repository: RepositoryProjection; role: string }

export type MemberRef = { handle: string; id: string; kind: string; name: string }

export type Mention = { author: string; conversation: string; conversationId: string; messageId: string; project: string; sentAt: string; sequence: number; text: string }

export type MessageContent = { nodes: MessageNode[]; version: number }

export type MessageNode = { text: string; type: "text" } | { actorId: string; type: "mention" } | { type: "work_item_ref"; workItemId: string }

export type MilestoneRef = { id: string; name: string; targetDate?: string | null }

export type MilestoneValue = { archived: boolean; canceled?: number; description: string; finished: number; id: string; idempotentReplay?: boolean; name: string; open?: number; overdue: boolean; progress?: number; targetDate?: string | null; total: number; url: string; version: number }

export type ModelUsageLine = { cachedInputTokens: number; confidence: string; cost?: number | null; currency?: string | null; inputTokens: number; model: string; outputTokens: number }

export type MoveIssue = { candidates?: CandidateField[]; field: string; fieldId: string; fieldType: FieldType; id: string; linkTarget?: LinkTarget | null; message: string; reason: IssueReason; taskId: string; taskKey: string; value: unknown }

export type MovePlan = { archivedFieldValues?: number; carried?: CarriedValue[]; issues?: MoveIssue[]; notes?: string[]; snapshot: string; targetProjectId: string; targetProjectKey: string; tasks: PlannedTask[] }

export type MovedTask = { from: string; id: string; title: string; to: string; url: string }

export type NamedProjection = { id: string; name: string }

export type NavEntry = { icon?: string | null; label: string; target: unknown }

export type NavView = { icon?: string | null; id: string; label: string; target: unknown }

export type NewSpace = { key: string; name: string }

export type NewTask = { acceptanceCriteria?: string[]; assignee?: string | null; blockedByRefs?: string[] | null; description?: string | null; dueDate?: string | null; estimate?: number | null; fields?: Record<string, unknown> | null; labels?: string[]; milestone?: string | null; parentId?: string | null; parentRef?: string | null; priority?: string | null; ref?: string | null; startDate?: string | null; status?: string | null; title: string; type?: string | null }

export type NotificationLine = { approvalId?: string | null; at: string; count: number; id: string; kind: string; preview?: string | null; project: string; read: boolean; snoozedUntil?: string | null; subject: string; subjectId: string; subjectKind: string; title: string; url: string }

export type Operation = { arrowheads?: string | null; axis?: string | null; backgroundColor?: string | null; dataBase64?: string | null; direction?: string | null; dx?: number | null; dy?: number | null; edge?: string | null; fileId?: string | null; fillStyle?: string | null; fontSize?: number | null; frameId?: string | null; from?: string | null; gap?: number | null; height?: number | null; id?: string | null; ids?: string[] | null; link?: string | null; locked?: boolean | null; mimeType?: string | null; name?: string | null; op: string; opacity?: number | null; points?: number[][] | null; position?: string | null; roughness?: number | null; rounded?: boolean | null; strokeColor?: string | null; strokeStyle?: string | null; strokeWidth?: number | null; text?: string | null; textAlign?: string | null; to?: string | null; type?: string | null; width?: number | null; x?: number | null; y?: number | null }

export type OperationResult = { ids: string[]; op: string }

export type OptionInput = { category?: string | null; color?: string | null; id?: string | null; name: string }

export type OptionView = { archived?: boolean; category?: string | null; color: string; id: string; name: string }

export type OwnerView = { id: string; kind: string; name: string }

export type Package = { contents: Content[]; description?: string; digest?: string | null; format: string; id: string; name: string; needsAdmin?: string[]; publisher?: Publisher | null; requires?: Requires; target: PackageTarget; version: string }

export type PackageTarget = "space" | "workspace"

export type PageLine = { id: string; stale: boolean; title: string; updatedAt: string }

export type PartRef = { id: string; kind: string }

export type PartView = { id: string; kind: string; name?: string | null }

export type PinItem = { id: string; kind: string; text: string; title: string }

export type PlanItem = { action: string; existingId?: string | null; key: string; kind: string; name: string; needsAdmin: boolean; reason?: string | null }

export type PlannedTask = { id: string; key: string; parentId?: string | null; title: string }

export type ProjectHeader = { id: string; key: string; name: string; role: string; url: string }

export type ProjectLine = { archived?: boolean; id: string; key: string; messagesSpace: boolean; name: string; openTasks: number; role: string; unreadConversations: number; url: string; version: number }

export type Publisher = { name: string; url?: string | null }

export type PullRequest = { baseRef: string; checksState: string; draft: boolean; headRef: string; id: string; mergedAt?: string | null; number: number; repository: string; repositoryId: string; reviewState: string; spaceId: string; state: string; title: string; updatedAt: string; url: string }

export type PullRequestProjection = { baseRef: string; canonicalUrl: string; checksState: string; draft: boolean; headRef: string; headSha: string; id: string; mergeabilityState: string; number: number; projectId: string; repositoryId: string; reviewState: string; state: string; suggested: boolean; syncState: string; title: string; url: string; version: number }

export type RaisedRequestOutput = { activityId: string; clientRequestId?: string | null; requestId: string }

export type ReactionSummary = { byYou: boolean; count: number; emoji: string; name: string; people: string[] }

export type RelationRef = { kind: string; task: TaskRef }

export type RelationsValue = { changed: boolean; relations: RelationRef[]; task: ChangedTask; version: number }

export type RemoveParts = "archive" | "keep"

export type ReorderedTask = { boardPosition: number; id: string; key: string; version: number }

export type RepositoryLine = { defaultBranch: string; id: string; name: string }

export type RepositoryProjection = { archived: boolean; canonicalUrl: string; cloneUrl: string; defaultBranch: string; enabled: boolean; id: string; name: string; owner: string; preparationState: string; projectId: string; trusted: boolean; version: number }

export type RequestAnswerView = { label?: string | null; optionId?: string | null; text?: string | null }

export type RequestLine = { answeredAt?: string | null; conversationId?: string | null; createdAt: string; id: string; kind: string; status: string; title: string }

export type RequestOption = { id: string; label: string; style?: string | null }

export type RequestOptionInput = { id: string; label: string; style?: string | null }

export type RequestView = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type Requires = { contract?: string | null; modules?: string[] }

export type Resolution = { action: ResolutionAction; issue: string; targetField?: string | null; value?: unknown }

export type ResolutionAction = "leave" | "set"

export type RevisionItem = { redacted?: boolean; revision: number; text: string; truncated?: boolean; via?: string | null; writtenAt: string; writtenBy: string }

export type RunActivityInput = { at?: string | null; id: string; kind: string; parentRef?: string | null; payload?: unknown; stepRef?: string | null; summary?: string | null }

export type RunCauseInput = { conversationId?: string | null; kind: string; messageId?: string | null; ref?: string | null; taskId?: string | null }

export type RunRecord = { activityCount: number; cancelRequestedAt?: string | null; clientRunId?: string | null; externalUrl?: string | null; finishedAt?: string | null; liveness: string; reportIntervalSeconds: number; runId: string; state: string; statusLine?: string | null; title: string; version: number }

export type RunResultInput = { branch?: string | null; id?: string | null; kind: string; number?: number | null; thingKind?: string | null; title?: string | null; url?: string | null }

export type ScheduledLine = { conversationId: string; failure?: string | null; id: string; place: string; sendAt: string; status: string; text: string; url: string }

export type SearchResult = { at: string; author?: string | null; conversation?: string | null; conversationId?: string | null; id: string; kind: string; project: string; projectId?: string | null; snippet: string; title: string; url: string }

export type SentMessage = { conversationId: string; idempotentReplay: boolean; messageId: string; sentAt: string; sequence: number; text: string }

export type SkillLine = { description: string; name: string; pageId: string; stale: boolean; title: string; updatedAt: string; version: number }

export type SortWrite = { direction?: Direction; field: string }

export type SourceView = { error?: string | null; fetchedAt?: string | null; id: string; name: string; url: string }

export type StatusCount = { category: string; count: number; status: string }

export type StatusProjection = { id: string; name: string; semanticCategory: string }

export type TaskFieldValue = { name: string; type: FieldType; value: unknown }

export type TaskHistory = { entries: TaskHistoryEntry[]; olderCursor?: string | null; total: number }

export type TaskHistoryEntry = { at: string; by: string; byHandle: string; byKind: string; changedFields?: string[]; criterionId?: string | null; fromStatus?: string | null; fromTaskId?: string | null; kind: string; sequence: number; text: string; textTruncated?: boolean; toStatus?: string | null; via?: string | null }

export type TaskLine = { claimedBy?: string | null; id: string; key: string; priority: string; project: string; status: string; title: string; updatedAt: string }

export type TaskOverview = { blocked?: TaskLine[]; byStatus: StatusCount[]; claimed?: TaskLine[]; next: TaskLine[] }

export type TaskRef = { finished: boolean; id: string; key: string; status: string; statusGroup?: string; title: string }

export type TaskSummary = { archived?: boolean; assignee?: string | null; blocked: boolean; boardPosition: number; claimedBy?: string | null; criteriaDone?: number | null; criteriaTotal?: number | null; dueDate?: string | null; fields?: Record<string, unknown> | null; id: string; key: string; labels?: string[]; milestone?: string | null; openBlockerCount?: number; overdue?: boolean; priority: string; project: string; ready?: boolean; status: string; statusGroup: string; title: string; type: string; updatedAt: string; version: number }

export type TemplateSubtask = { description?: string | null; title: string }

export type ThreadRef = { id: string; name: string; status: string }

export type ThreadUpdate = { changedAt: string; changedBy?: string | null; name: string; outcome?: string | null; status: string; threadId: string; under: string }

export type UnattributedUsageLine = { accountId: string; cost?: number | null; currency?: string | null; inputTokens: number; name?: string | null; outputTokens: number; reports: number }

export type UpdatedTask = { archived: boolean; changedFields?: string[]; id: string; index: number; key: string; status: string; url: string; version: number }

export type ViewWrite = { columns?: ColumnWrite[] | null; dateField?: string | null; filters?: FilterWrite[] | null; groupBucket?: DateBucket | null; groupBy?: string | null; name?: string | null; position?: number | null; remove?: boolean | null; sort?: SortWrite[] | null; titleWidth?: number | null; type?: string | null; view?: string | null }

export type WatcherRef = { handle: string; kind: string; name: string }

export type WhiteboardLine = { createdAt: string; createdBy?: string | null; deleted: boolean; elementCount: number; id: string; linkCount: number; organizationId: string; organizationName: string; projectId?: string | null; projectName?: string | null; sceneSeq: number; sceneUpdatedAt: string; title: string; updatedAt: string; updatedBy?: string | null; updatedByKind?: string | null; url: string; version: number }

export type WordLine = { many: string; noun: string; one: string }

export type WorkItemProjection = { acceptanceCriteria: AcceptanceCriterionProjection[]; archived: boolean; assignee?: AssigneeProjection | null; blocked?: BlockedProjection | null; canonicalUrl: string; description: string; descriptionTruncated: boolean; id: string; itemType: NamedProjection; key: string; labels: string[]; linkedPullRequests: PullRequestProjection[]; linkedRepositories: LinkedRepositoryProjection[]; parentWorkItemId?: string | null; priority: string; projectId: string; status: StatusProjection; title: string; version: number }

export type You = { access: string; actorId: string; connectedThrough?: string | null; handle: string; kind: string; name: string }

export type AgentCreateInput = { connectionName?: string | null; displayName: string; handle: string; ownerAccountId?: string | null; roleId?: string | null; scopes?: string[] | null }

export type AgentCreateOutput = { accountId: string; connectionId: string; credential?: string | null; credentialExpiresAt?: string | null; spaceId: string }

export type AgentManagersSetInput = { add?: string[]; agent: string; remove?: string[] }

export type AgentManagersSetOutput = { agent: string; managers: string[] }

export type AgentTemplateListInput = { spaceId?: string | null }

export type AgentTemplateListOutput = { templates: AgentTemplateView[] }

export type AgentUsageListInput = { days?: number | null }

export type AgentUsageListOutput = { accounts: AgentUsageLine[]; days: number; notTiedToARun: UnattributedUsageLine[] }

export type BriefingGetInput = { projectId?: string | null }

export type BriefingGetOutput = { claims?: TaskLine[]; mentions: Mention[]; nextSteps: string; notifications: NotificationLine[]; replyRequests?: InboxItem[]; tasks: TaskLine[]; threadUpdates: ThreadUpdate[]; unread: ConversationSummary[]; you: You }

export type BugReportLinkCreateInput = { collection?: string | null; delete?: boolean | null; form?: FormWrite | null; linkId?: string | null; name?: string | null; projectId: string; replace?: boolean | null; state?: string | null }

export type BugReportLinkCreateOutput = unknown

export type BugReportLinkListInput = { projectId: string }

export type BugReportLinkListOutput = unknown

export type BugReportLinkUpdateInput = { collection?: string | null; delete?: boolean | null; form?: FormWrite | null; linkId?: string | null; name?: string | null; projectId: string; replace?: boolean | null; state?: string | null }

export type BugReportLinkUpdateOutput = unknown

export type ChannelCreateInput = { name: string; projectId: string }

export type ChannelCreateOutput = { archived: boolean; created?: boolean | null; id: string; kind: string; name: string; outcome?: string | null; status?: string | null; url: string; version: number }

export type ConversationContinueInput = { conversationId: string }

export type ConversationContinueOutput = { agentTurnsRemaining: number; asked?: MemberRef[]; conversationId: string; idempotentReplay?: boolean; latestMessageId?: string | null }

export type ConversationMarkReadInput = { conversationId: string; fromMessageId?: string | null; unread?: boolean | null; upToSequence?: number | null }

export type ConversationMarkReadOutput = { conversationId: string; lastReadSequence: number; unreadCount: number }

export type ConversationMembersUpdateInput = { add?: string[] | null; conversationId: string; expectedVersion?: number | null; remove?: string[] | null }

export type ConversationMembersUpdateOutput = { added?: MemberRef[]; conversationId: string; idempotentReplay?: boolean; left?: boolean; members: MemberRef[]; name: string; removed?: MemberRef[]; url: string; version: number }

export type ConversationUpdateInput = { archived?: boolean | null; clearOutcome?: boolean | null; conversationId: string; expectedVersion?: number | null; name?: string | null; outcome?: string | null; status?: string | null }

export type ConversationUpdateOutput = { archived: boolean; created?: boolean | null; id: string; kind: string; name: string; outcome?: string | null; status?: string | null; url: string; version: number }

export type DecisionListInput = { includeSuperseded?: boolean; limit?: number | null; spaceId: string }

export type DecisionListOutput = { decisions: DecisionView[] }

export type DecisionRecordInput = { detail?: string | null; pin?: boolean; sourceConversationId?: string | null; sourceMessageId?: string | null; spaceId?: string | null; supersedes?: string | null; title: string }

export type DecisionRecordOutput = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

export type DependencyGraphGetInput = { includeFinished?: boolean | null; label?: string | null; milestoneId?: string | null; parentId?: string | null; projectId: string }

export type DependencyGraphGetOutput = unknown

export type DirectoryListInput = { query?: string | null; shapes?: boolean | null }

export type DirectoryListOutput = { entries: EntryView[]; ownSourcesAllowed: boolean; shape?: string | null; sources: SourceView[] }

export type DirectorySourceCreateInput = { name?: string | null; url: string }

export type DirectorySourceCreateOutput = { error?: string | null; fetchedAt?: string | null; id: string; name: string; url: string }

export type DirectorySourceRemoveInput = { sourceId: string }

export type DirectorySourceRemoveOutput = { id: string }

export type FieldCreateInput = { archived?: boolean | null; description?: string | null; expectedVersion?: number | null; field?: string | null; name?: string | null; options?: OptionInput[] | null; position?: number | null; projectId: string; target?: string | null; type?: string | null }

export type FieldCreateOutput = { changedFields: string[]; created: boolean; field: FieldView; note?: string | null }

export type FieldListInput = { includeArchived?: boolean | null; projectId: string }

export type FieldListOutput = { canManage: boolean; fields: FieldView[] }

export type FieldUpdateInput = { archived?: boolean | null; description?: string | null; expectedVersion?: number | null; field?: string | null; name?: string | null; options?: OptionInput[] | null; position?: number | null; projectId: string; target?: string | null; type?: string | null }

export type FieldUpdateOutput = { changedFields: string[]; created: boolean; field: FieldView; note?: string | null }

export type HistoryListInput = { before?: number | null; limit?: number | null; thing: string }

export type HistoryListOutput = { caps: HistoryCaps; entries: HistoryEntry[]; kind: string }

export type HistoryRestoreInput = { expectedVersion: number; thing: string; version: number }

export type HistoryRestoreOutput = { kind: string; restored: number; version: number }

export type HistoryShowInput = { thing: string; version: number }

export type HistoryShowOutput = { content: unknown; kind: string; version: number }

export type HostAgentBindInput = { accountId: string; hostAgentRef: string; hostId: string }

export type HostAgentBindOutput = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type HostAgentUnbindInput = { accountId: string; hostId: string }

export type HostAgentUnbindOutput = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type HostEnrolmentCreateInput = { label?: string | null; owner: HostOwnerChoice }

export type HostEnrolmentCreateOutput = { code: string; enrolmentId: string; expiresAt: string; owner: OwnerView }

export type HostListInput = { mineOnly?: boolean }

export type HostListOutput = { hosts: HostView[] }

export type HostUnlinkInput = { hostId: string }

export type HostUnlinkOutput = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type InviteLinkCreateInput = { expiresInDays?: number | null; maxUses?: number | null; roleId?: string | null; space?: string | null }

export type InviteLinkCreateOutput = { link: InviteLinkView; token?: string | null; url?: string | null }

export type InviteLinkListInput = { includeInactive?: boolean }

export type InviteLinkListOutput = { links: InviteLinkView[] }

export type InviteLinkRevokeInput = { link: string }

export type InviteLinkRevokeOutput = { active: boolean; createdAt: string; createdBy: string; expiresAt: string; id: string; maxUses?: number | null; revokedAt?: string | null; roleId: string; roleName: string; spaceId?: string | null; uses: number }

export type LabelCreateInput = { archived?: boolean | null; color?: string | null; description?: string | null; expectedVersion?: number | null; label?: string | null; name?: string | null; projectId: string }

export type LabelCreateOutput = { changedFields: string[]; created: boolean; label: LabelLine; note?: string | null }

export type LabelListInput = { includeArchived?: boolean | null; projectId: string }

export type LabelListOutput = { labels: LabelLine[] }

export type LabelUpdateInput = { archived?: boolean | null; color?: string | null; description?: string | null; expectedVersion?: number | null; label?: string | null; name?: string | null; projectId: string }

export type LabelUpdateOutput = { changedFields: string[]; created: boolean; label: LabelLine; note?: string | null }

export type MessageDeleteInput = { messageId: string }

export type MessageDeleteOutput = { alreadyRemoved: boolean; cancelledReplyRequests: number; conversationId: string; messageId: string; note: string; removedAt: string; removedBy: string; threads?: ThreadRef[] }

export type MessageForwardInput = { conversationId: string; messageId: string; text?: string | null }

export type MessageForwardOutput = { conversationId: string; idempotentReplay: boolean; messageId: string; sentAt: string; sequence: number; text: string }

export type MessageHistoryInput = { messageId: string }

export type MessageHistoryOutput = { conversationId: string; messageId: string; removed?: boolean; revision: number; revisions: RevisionItem[] }

export type MessagePreviewHideInput = { messageId: string; url?: string | null }

export type MessagePreviewHideOutput = { hidden: number; messageId: string }

export type MessageReactInput = { emoji: string; messageId: string; reacted?: boolean | null }

export type MessageReactOutput = { changed: boolean; conversationId: string; emoji: string; messageId: string; name: string; reacted: boolean; reactions: ReactionSummary[] }

export type MessageScheduleInput = { content?: MessageContent | null; conversationId: string; sendAt: string; text?: string | null }

export type MessageScheduleOutput = { idempotentReplay?: boolean; scheduled: ScheduledLine }

export type MessageSendInput = { attachmentIds?: string[] | null; content?: MessageContent | null; conversationId: string; replyToMessageId?: string | null; text?: string | null }

export type MessageSendOutput = { conversationId: string; idempotentReplay: boolean; messageId: string; sentAt: string; sequence: number; text: string }

export type MessageUpdateInput = { content?: MessageContent | null; expectedRevision?: number | null; messageId: string; text?: string | null }

export type MessageUpdateOutput = { changed: boolean; conversationId: string; editedAt?: string | null; idempotentReplay: boolean; messageId: string; newlyMentioned?: string[]; revision: number; text: string; url: string }

export type MilestoneCreateInput = { addTasks?: string[] | null; archived?: boolean | null; clientRequestId?: string | null; description?: string | null; expectedVersion?: number | null; milestoneId?: string | null; name?: string | null; projectId: string; removeTasks?: string[] | null; targetDate?: string | null }

export type MilestoneCreateOutput = { archived: boolean; canceled?: number; description: string; finished: number; id: string; idempotentReplay?: boolean; name: string; open?: number; overdue: boolean; progress?: number; targetDate?: string | null; total: number; url: string; version: number }

export type MilestoneListInput = { includeArchived?: boolean | null; projectId: string }

export type MilestoneListOutput = { milestones: MilestoneValue[] }

export type MilestoneUpdateInput = { addTasks?: string[] | null; archived?: boolean | null; clientRequestId?: string | null; description?: string | null; expectedVersion?: number | null; milestoneId?: string | null; name?: string | null; projectId: string; removeTasks?: string[] | null; targetDate?: string | null }

export type MilestoneUpdateOutput = { archived: boolean; canceled?: number; description: string; finished: number; id: string; idempotentReplay?: boolean; name: string; open?: number; overdue: boolean; progress?: number; targetDate?: string | null; total: number; url: string; version: number }

export type ModuleArchiveInput = { moduleId: string; parts: ArchiveParts }

export type ModuleArchiveOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModuleCreateInput = { automations?: InlineAutomation[]; collections?: InlineCollection[]; description?: string | null; forms?: InlineForm[]; icon?: string | null; key: string; name: string; nav?: NavEntry[]; parts?: PartRef[]; spaceId?: string | null }

export type ModuleCreateOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModuleOpenInput = { moduleId: string }

export type ModuleOpenOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModulePartLinkInput = { moduleId: string; part: PartRef }

export type ModulePartLinkOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModulePartUnlinkInput = { moduleId: string; part: PartRef }

export type ModulePartUnlinkOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModuleRestoreInput = { moduleId: string }

export type ModuleRestoreOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModuleSetInput = { moduleId: string; on: boolean }

export type ModuleSetOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModuleUpdateInput = { description?: string | null; expectedRevision: number; icon?: string | null; moduleId: string; name?: string | null; nav?: NavEntry[]; removeNav?: string[] }

export type ModuleUpdateOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type MyWorkGetInput = { projectId?: string | null }

export type MyWorkGetOutput = { assigned: unknown; claimed: unknown; review: unknown }

export type PackageExportInput = { includePages?: boolean; includeSkills?: boolean; module?: string | null; spaceId?: string | null; wholeWorkspace?: boolean }

export type PackageExportOutput = { digest: string; package: Package }

export type PackageInstallInput = { entry?: string | null; fileId?: string | null; package?: Record<string, unknown> | null; planHash: string; target?: InstallTarget }

export type PackageInstallOutput = { changes: string[]; installId: string; objects: InstalledObject[]; packageId: string; skipped: string[]; spaceId?: string | null; version: string }

export type PackageListInput = { spaceId?: string | null }

export type PackageListOutput = { installs: InstallView[] }

export type PackagePreviewInput = { entry?: string | null; fileId?: string | null; package?: Record<string, unknown> | null; target?: InstallTarget }

export type PackagePreviewOutput = { digest: string; items: PlanItem[]; leaves: string[]; name: string; packageId: string; planHash: string; roles: unknown; target: unknown; turnsOff: string[]; turnsOn: string[]; version: string }

export type PackageRemoveInput = { installId: string; parts: RemoveParts }

export type PackageRemoveOutput = { id: string; installedAt: string; name: string; objects: number; packageId: string; spaceId?: string | null; state: string; summary: string[]; version: string }

export type PersonCreateInput = { displayName: string; handle: string; roleId?: string | null }

export type PersonCreateOutput = { accountId: string; claimExpiresAt: string; claimToken?: string | null }

export type PinSetInput = { conversationId: string; decisionId?: string | null; messageId?: string | null; pageId?: string | null; pinned?: boolean | null }

export type PinSetOutput = { conversationId: string; id: string; kind: string; pinned: boolean; pins: PinItem[] }

export type PlanImportInput = { commit?: boolean | null; expectedVersion?: number | null; importId?: string | null; pageId?: string | null; plan?: string | null; projectId: string; title?: string | null }

export type PlanImportOutput = unknown

export type ProjectGetInput = { projectId: string }

export type ProjectGetOutput = { channels: ConversationSummary[]; decisions: DecisionLine[]; guide?: GuidePage | null; labels?: string[]; layout?: LayoutLine | null; modulesOff?: string[]; openThreads: ConversationSummary[]; pages: PageLine[]; project: ProjectHeader; repositories: RepositoryLine[]; skills?: SkillLine[]; taskCollections?: string[]; taskFields?: string[]; tasks: TaskOverview }

export type ProjectListInput = { includeArchived?: boolean | null }

export type ProjectListOutput = { projects: ProjectLine[] }

export type RecurringCreateInput = { assignee?: string | null; clearDueInDays?: boolean | null; delete?: boolean | null; dueInDays?: number | null; enabled?: boolean | null; every?: number | null; expectedVersion?: number | null; frequency?: string | null; monthDay?: number | null; projectId: string; recurring?: string | null; startsOn?: string | null; template?: string | null; timeOfDay?: string | null; timezone?: string | null; weekdays?: number[] | null }

export type RecurringCreateOutput = { created: boolean; deleted: boolean; recurring?: unknown }

export type RecurringUpdateInput = { assignee?: string | null; clearDueInDays?: boolean | null; delete?: boolean | null; dueInDays?: number | null; enabled?: boolean | null; every?: number | null; expectedVersion?: number | null; frequency?: string | null; monthDay?: number | null; projectId: string; recurring?: string | null; startsOn?: string | null; template?: string | null; timeOfDay?: string | null; timezone?: string | null; weekdays?: number[] | null }

export type RecurringUpdateOutput = { created: boolean; deleted: boolean; recurring?: unknown }

export type RequestCancelInput = { requestId: string }

export type RequestCancelOutput = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type RequestCreateInput = { allowText?: boolean | null; audience: AudienceInput; conversationId?: string | null; detail?: string | null; expiresInSeconds?: number | null; kind?: string | null; onExpiry?: string | null; options?: RequestOptionInput[]; spaceId?: string | null; taskId?: string | null; title: string }

export type RequestCreateOutput = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type RequestGetInput = { requestId: string }

export type RequestGetOutput = { mayAnswer: boolean; request: RequestView }

export type RequestListInput = { limit?: number | null; role?: string | null; spaceId?: string | null; status?: string | null }

export type RequestListOutput = { requests: RequestView[] }

export type RequestRespondInput = { optionId?: string | null; requestId: string; text?: string | null }

export type RequestRespondOutput = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type RunActivityListInput = { afterSeq?: number | null; limit?: number | null; runId: string }

export type RunActivityListOutput = { activities: ActivityLine[]; nextAfterSeq?: number | null }

export type RunActivityReportInput = { activities: RunActivityInput[]; runId: string }

export type RunActivityReportOutput = { accepted: number; duplicates: number; lastSeq: number; overLimit?: number; requests?: RaisedRequestOutput[]; state: string; version: number }

export type RunCancelInput = { reason?: string | null; runId: string }

export type RunCancelOutput = { idempotentReplay?: boolean; run: RunRecord }

export type RunCreateInput = { cause: RunCauseInput; clientRunId?: string | null; parentRunId?: string | null; reportIntervalSeconds?: number | null; settings?: unknown; spaceId: string; state?: string | null; title: string }

export type RunCreateOutput = { created: boolean; run: RunRecord }

export type RunDetailGetInput = { runId: string }

export type RunDetailGetOutput = { activities: ActivityLine[]; agentActorId: string; agentName: string; backend?: string | null; branch?: string | null; builtin: boolean; cachedInputTokens: number; causeKind: string; causeRef?: string | null; conversationId?: string | null; cost?: number | null; costConfidence?: string | null; createdAt: string; currency?: string | null; externalUrl?: string | null; failureReason?: string | null; finishedAt?: string | null; gapCount: number; harness?: string | null; harnessName?: string | null; hiddenActivities: number; hostId?: string | null; hostName?: string | null; id: string; inputTokens: number; lastReportAt?: string | null; liveness: string; model?: string | null; organizationId: string; outcomeSummary?: string | null; outputTokens: number; plan?: unknown; projectId?: string | null; projectKey?: string | null; projectName?: string | null; pullRequestNumber?: number | null; pullRequestState?: string | null; pullRequestUrl?: string | null; pullRequests: PullRequest[]; requestedByActorId?: string | null; requestedByName?: string | null; requests: RequestLine[]; results: unknown[]; startedAt?: string | null; state: string; statusLine?: string | null; taskId?: string | null; taskKey?: string | null; taskTitle?: string | null; threadName?: string | null; title: string; updatedAt: string; usageByModel: ModelUsageLine[]; waitingOnRequestId?: string | null; waitingOnTitle?: string | null }

export type RunMarkNotStartedInput = { runId: string }

export type RunMarkNotStartedOutput = { idempotentReplay?: boolean; run: RunRecord }

export type RunResultReportInput = { results: RunResultInput[]; runId: string }

export type RunResultReportOutput = { added: number; run: RunRecord }

export type RunUpdateInput = { backend?: string | null; expectedVersion?: number | null; externalUrl?: string | null; failureReason?: string | null; outcomeSummary?: string | null; runId: string; state?: string | null; statusLine?: string | null; title?: string | null }

export type RunUpdateOutput = { idempotentReplay?: boolean; run: RunRecord }

export type RunUsageReportInput = { cachedInputTokens?: number | null; cost?: number | null; currency?: string | null; inputTokens?: number | null; model: string; outputTokens?: number | null; reportKey: string; runId?: string | null; spaceId?: string | null; windowEnd?: string | null; windowStart?: string | null }

export type RunUsageReportOutput = { duplicate: boolean; reportId: string; runId?: string | null }

export type ScheduledMessageUpdateInput = { cancel?: boolean | null; id: string; sendAt?: string | null; sendNow?: boolean | null; text?: string | null }

export type ScheduledMessageUpdateOutput = { canceled: boolean; scheduled?: ScheduledLine | null }

export type TaskArchiveInput = { expectedVersion: number; taskId: string }

export type TaskArchiveOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskClaimInput = { minutes?: number | null; note?: string | null; taskId: string }

export type TaskClaimOutput = { claimedAt: string; claimedBy: string; expiresAt: string; handoff?: HandoffNote | null; note?: string | null; taskId: string }

export type TaskCommentCreateInput = { attachmentIds?: string[] | null; replyToCommentId?: string | null; taskId: string; text: string }

export type TaskCommentCreateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskCommentDeleteInput = { commentId: string; taskId: string }

export type TaskCommentDeleteOutput = { alreadyDeleted: boolean; commentId: string; note: string; taskId: string }

export type TaskCommentUpdateInput = { commentId: string; expectedRevision?: number | null; taskId: string; text: string }

export type TaskCommentUpdateOutput = { changed: boolean; commentId: string; revision: number; taskId: string }

export type TaskCreateInput = { acceptanceCriteria?: string[] | null; afterTaskId?: string | null; assignee?: string | null; beforeTaskId?: string | null; collection?: string | null; description?: string | null; dueDate?: string | null; estimate?: number | null; fields?: Record<string, unknown> | null; labels?: string[] | null; milestone?: string | null; parentId?: string | null; position?: string | null; priority?: string | null; projectId?: string | null; startDate?: string | null; status?: string | null; title: string; type?: string | null }

export type TaskCreateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskCreateManyInput = { clientRequestId?: string | null; createMissingLabels?: boolean | null; preview?: boolean | null; projectId?: string | null; tasks: NewTask[] }

export type TaskCreateManyOutput = { idempotentReplay?: boolean; labelsCreated?: string[]; notes?: string[]; preview: boolean; projectKey: string; tasks: CreatedTask[] }

export type TaskCriteriaUpdateInput = { expectedVersion: number; operations: CriterionOperationArgument[]; taskId: string }

export type TaskCriteriaUpdateOutput = { completed: number; criteria: CriterionValue[]; idempotentReplay?: boolean; key: string; operations: CriterionOperationOutcome[]; taskId: string; total: number; url: string; version: number }

export type TaskGetInput = { historyBefore?: string | null; historyLimit?: number | null; include?: string[] | null; taskId: string }

export type TaskGetOutput = { blockedBy?: TaskRef[]; blockedReason?: string | null; blocks?: TaskRef[]; claimedBy?: ClaimValue | null; collection?: string | null; comments?: CommentItem[]; conversations?: LinkedItem[]; dueDate?: string | null; estimate?: number | null; estimateUnit?: string; fields?: TaskFieldValue[]; files?: FileSummary[]; guidePageId?: string | null; handoffs?: HandoffNote[]; history?: TaskHistory | null; links?: LinkedItem[]; milestone?: MilestoneRef | null; overdue?: boolean; parent?: TaskRef | null; ready?: boolean; relations?: RelationRef[]; startDate?: string | null; subtasks?: TaskSummary[]; task: WorkItemProjection; watchers?: WatcherRef[] }

export type TaskImportInput = { commit?: boolean | null; githubState?: string | null; headingsAsMilestones?: boolean | null; importId?: string | null; mapping?: Record<string, string> | null; projectId: string; repositoryId?: string | null; source?: string | null; text?: string | null }

export type TaskImportOutput = { import: unknown; nextStep: string }

export type TaskLinkInput = { blockedBy?: string[] | null; blocks?: string[] | null; close?: boolean | null; conversationId?: string | null; decisionId?: string | null; expectedVersion?: number | null; kind?: string | null; messageId?: string | null; otherTaskId?: string | null; pageId?: string | null; pullRequestId?: string | null; remove?: boolean | null; repositoryId?: string | null; role?: string | null; taskId: string; whiteboardId?: string | null }

export type TaskLinkOutput = DependencyValue | RelationsValue | LinkValue

export type TaskLinkManyInput = { links: DependencyLinks[]; projectId?: string | null; remove?: boolean | null }

export type TaskLinkManyOutput = { changed: number; tasks: DependencyChange[] }

export type TaskListInput = { anyLabels?: string[] | null; assignee?: string[] | null; blocked?: boolean | null; claimed?: string | null; collection?: string | null; createdBy?: string | null; cursor?: string | null; dueAfter?: string | null; dueBefore?: string | null; fieldFilters?: FieldFilterArgument[] | null; includeArchived?: boolean | null; includeDone?: boolean | null; labels?: string[] | null; limit?: number | null; milestone?: string[] | null; notAssignee?: string[] | null; notLabels?: string[] | null; notMilestone?: string[] | null; notPriority?: string[] | null; notStatus?: string[] | null; notType?: string[] | null; overdue?: boolean | null; parentId?: string | null; priority?: string[] | null; projectId?: string | null; projectIds?: string[] | null; query?: string | null; ready?: boolean | null; sort?: string | null; status?: string[] | null; type?: string[] | null; updatedSince?: string | null; view?: string | null }

export type TaskListOutput = { nextCursor?: string | null; tasks: TaskSummary[]; total: number }

export type TaskMoveInput = { clientRequestId?: string | null; leaveUnresolved?: boolean | null; preview?: boolean | null; resolutions?: Resolution[] | null; snapshot?: string | null; taskId: string; toProjectId: string }

export type TaskMoveOutput = { created: boolean; leftBehind?: number; moved: MovedTask[]; notes?: string[]; preview?: MovePlan | null; replayed?: boolean }

export type TaskReadyListInput = { limit?: number | null; milestoneId?: string | null; offset?: number | null; projectId: string; scope?: string | null; taskId?: string | null }

export type TaskReadyListOutput = unknown

export type TaskReleaseInput = { anyone?: boolean | null; handoff?: string | null; taskId: string }

export type TaskReleaseOutput = { handoffSaved?: boolean; released: boolean; taskId: string }

export type TaskReorderInput = { afterTaskId?: string | null; beforeTaskId?: string | null; position?: string | null; taskIds: string[] }

export type TaskReorderOutput = { changed: boolean; column: string[]; status: string; tasks: ReorderedTask[] }

export type TaskRestoreInput = { expectedVersion: number; taskId: string }

export type TaskRestoreOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskUpdateInput = { addLabels?: string[] | null; afterTaskId?: string | null; archived?: boolean | null; assignee?: string | null; beforeTaskId?: string | null; blockedReason?: string | null; clearAssignee?: boolean | null; clearEstimate?: boolean | null; clearParent?: boolean | null; description?: string | null; dueDate?: string | null; estimate?: number | null; expectedVersion: number; fields?: Record<string, unknown> | null; milestone?: string | null; parentId?: string | null; position?: string | null; priority?: string | null; readyForReview?: boolean | null; removeLabels?: string[] | null; startDate?: string | null; status?: string | null; taskId: string; title?: string | null; type?: string | null }

export type TaskUpdateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskUpdateManyInput = { addLabels?: string[] | null; archived?: boolean | null; assignee?: string | null; clearAssignee?: boolean | null; clientRequestId?: string | null; dueDate?: string | null; expectedVersions?: Record<string, number> | null; fields?: Record<string, unknown> | null; milestone?: string | null; preview?: boolean | null; priority?: string | null; projectId?: string | null; removeLabels?: string[] | null; status?: string | null; taskIds: string[]; type?: string | null }

export type TaskUpdateManyOutput = { changedCount: number; idempotentReplay?: boolean; notes?: string[]; preview: boolean; tasks: UpdatedTask[]; unchangedCount: number }

export type TaskWatchSetInput = { taskId: string; watching?: boolean | null }

export type TaskWatchSetOutput = { changed: boolean; task: TaskRef; watchers: WatcherRef[]; watching: boolean }

export type TemplateCreateInput = { acceptanceCriteria?: string[] | null; archived?: boolean | null; clearEstimate?: boolean | null; description?: string | null; estimate?: number | null; expectedVersion?: number | null; labels?: string[] | null; name?: string | null; priority?: string | null; projectId: string; subtasks?: TemplateSubtask[] | null; template?: string | null; title?: string | null; type?: string | null }

export type TemplateCreateOutput = { created: boolean; template: unknown }

export type TemplateListInput = { projectId: string }

export type TemplateListOutput = { recurring: unknown[]; templates: unknown[] }

export type TemplateUpdateInput = { acceptanceCriteria?: string[] | null; archived?: boolean | null; clearEstimate?: boolean | null; description?: string | null; estimate?: number | null; expectedVersion?: number | null; labels?: string[] | null; name?: string | null; priority?: string | null; projectId: string; subtasks?: TemplateSubtask[] | null; template?: string | null; title?: string | null; type?: string | null }

export type TemplateUpdateOutput = { created: boolean; template: unknown }

export type ThreadStartInput = { conversationId: string; fromMessageId?: string | null; name?: string | null; text?: string | null }

export type ThreadStartOutput = { created?: boolean | null; id: string; message?: SentMessage | null; name: string; outcome?: string | null; parentId: string; status: string; url: string }

export type ThreadUpdateInput = { clearOutcome?: boolean | null; name?: string | null; outcome?: string | null; status?: string | null; threadId: string }

export type ThreadUpdateOutput = { created?: boolean | null; id: string; message?: SentMessage | null; name: string; outcome?: string | null; parentId: string; status: string; url: string }

export type ViewCreateInput = { delete?: boolean | null; expectedVersion?: number | null; fields?: string[] | null; layout?: string | null; name?: string | null; pinned?: boolean | null; projectId: string; query?: Record<string, string> | null; shared?: boolean | null; view?: string | null }

export type ViewCreateOutput = { created: boolean; deleted: boolean; view?: unknown }

export type ViewListInput = { projectId: string }

export type ViewListOutput = { howToUse: string; views: unknown[] }

export type ViewUpdateInput = { delete?: boolean | null; expectedVersion?: number | null; fields?: string[] | null; layout?: string | null; name?: string | null; pinned?: boolean | null; projectId: string; query?: Record<string, string> | null; shared?: boolean | null; view?: string | null }

export type ViewUpdateOutput = { created: boolean; deleted: boolean; view?: unknown }

export type WhiteboardCreateInput = { linkTo?: LinkArgument | null; operations?: Operation[] | null; projectId?: string | null; title: string; workspaceId?: string | null }

export type WhiteboardCreateOutput = { edit?: EditOutcome | null; idempotentReplay?: boolean; whiteboard: WhiteboardLine }

export type WhiteboardDuplicateInput = { title?: string | null; whiteboardId: string }

export type WhiteboardDuplicateOutput = { edit?: EditOutcome | null; idempotentReplay?: boolean; whiteboard: WhiteboardLine }

export type WhiteboardLinkInput = { remove?: boolean | null; targetId: string; targetKind: string; whiteboardId: string }

export type WhiteboardLinkOutput = { changed: boolean; linked: boolean; links: LinkView[]; whiteboardId: string }

export type WhiteboardUpdateInput = { deleted?: boolean | null; expectedVersion?: number | null; projectId?: string | null; restoreRevision?: number | null; title?: string | null; whiteboardId: string; workspaceWide?: boolean | null }

export type WhiteboardUpdateOutput = { edit?: EditOutcome | null; idempotentReplay?: boolean; whiteboard: WhiteboardLine }

export type WhiteboardUpdateElementsInput = { confirmDelete?: boolean | null; expectedSceneSeq?: number | null; operations: Operation[]; whiteboardId: string }

export type WhiteboardUpdateElementsOutput = { changed: string[]; results: OperationResult[]; savedRevision?: number | null; sceneSeq: number }

export type WorkspaceDeleteInput = { confirm: string }

export type WorkspaceDeleteOutput = { deleteAfter?: string | null; lifecycle: Lifecycle; ownedAccounts: number }

export type WorkspaceDeleteCancelInput = Record<string, never>

export type WorkspaceDeleteCancelOutput = { deleteAfter?: string | null; lifecycle: Lifecycle; ownedAccounts: number }

export type WorkspaceSearchInput = { conversationId?: string | null; from?: string | null; kinds?: string[] | null; limit?: number | null; projectId?: string | null; query: string; since?: string | null; until?: string | null }

export type WorkspaceSearchOutput = { hint: string; results: SearchResult[] }

export type WorkspaceShapeSetInput = { planHash?: string | null; shape: string }

export type WorkspaceShapeSetOutput = { installed: Installed; name: string; shape: string }

export type Operations = {
  agent_create: { input: AgentCreateInput; output: AgentCreateOutput }
  agent_managers_set: { input: AgentManagersSetInput; output: AgentManagersSetOutput }
  agent_template_list: { input: AgentTemplateListInput; output: AgentTemplateListOutput }
  agent_usage_list: { input: AgentUsageListInput; output: AgentUsageListOutput }
  briefing_get: { input: BriefingGetInput; output: BriefingGetOutput }
  bug_report_link_create: { input: BugReportLinkCreateInput; output: BugReportLinkCreateOutput }
  bug_report_link_list: { input: BugReportLinkListInput; output: BugReportLinkListOutput }
  bug_report_link_update: { input: BugReportLinkUpdateInput; output: BugReportLinkUpdateOutput }
  channel_create: { input: ChannelCreateInput; output: ChannelCreateOutput }
  conversation_continue: { input: ConversationContinueInput; output: ConversationContinueOutput }
  conversation_mark_read: { input: ConversationMarkReadInput; output: ConversationMarkReadOutput }
  conversation_members_update: { input: ConversationMembersUpdateInput; output: ConversationMembersUpdateOutput }
  conversation_update: { input: ConversationUpdateInput; output: ConversationUpdateOutput }
  decision_list: { input: DecisionListInput; output: DecisionListOutput }
  decision_record: { input: DecisionRecordInput; output: DecisionRecordOutput }
  dependency_graph_get: { input: DependencyGraphGetInput; output: DependencyGraphGetOutput }
  directory_list: { input: DirectoryListInput; output: DirectoryListOutput }
  directory_source_create: { input: DirectorySourceCreateInput; output: DirectorySourceCreateOutput }
  directory_source_remove: { input: DirectorySourceRemoveInput; output: DirectorySourceRemoveOutput }
  field_create: { input: FieldCreateInput; output: FieldCreateOutput }
  field_list: { input: FieldListInput; output: FieldListOutput }
  field_update: { input: FieldUpdateInput; output: FieldUpdateOutput }
  history_list: { input: HistoryListInput; output: HistoryListOutput }
  history_restore: { input: HistoryRestoreInput; output: HistoryRestoreOutput }
  history_show: { input: HistoryShowInput; output: HistoryShowOutput }
  host_agent_bind: { input: HostAgentBindInput; output: HostAgentBindOutput }
  host_agent_unbind: { input: HostAgentUnbindInput; output: HostAgentUnbindOutput }
  host_enrolment_create: { input: HostEnrolmentCreateInput; output: HostEnrolmentCreateOutput }
  host_list: { input: HostListInput; output: HostListOutput }
  host_unlink: { input: HostUnlinkInput; output: HostUnlinkOutput }
  invite_link_create: { input: InviteLinkCreateInput; output: InviteLinkCreateOutput }
  invite_link_list: { input: InviteLinkListInput; output: InviteLinkListOutput }
  invite_link_revoke: { input: InviteLinkRevokeInput; output: InviteLinkRevokeOutput }
  label_create: { input: LabelCreateInput; output: LabelCreateOutput }
  label_list: { input: LabelListInput; output: LabelListOutput }
  label_update: { input: LabelUpdateInput; output: LabelUpdateOutput }
  message_delete: { input: MessageDeleteInput; output: MessageDeleteOutput }
  message_forward: { input: MessageForwardInput; output: MessageForwardOutput }
  message_history: { input: MessageHistoryInput; output: MessageHistoryOutput }
  message_preview_hide: { input: MessagePreviewHideInput; output: MessagePreviewHideOutput }
  message_react: { input: MessageReactInput; output: MessageReactOutput }
  message_schedule: { input: MessageScheduleInput; output: MessageScheduleOutput }
  message_send: { input: MessageSendInput; output: MessageSendOutput }
  message_update: { input: MessageUpdateInput; output: MessageUpdateOutput }
  milestone_create: { input: MilestoneCreateInput; output: MilestoneCreateOutput }
  milestone_list: { input: MilestoneListInput; output: MilestoneListOutput }
  milestone_update: { input: MilestoneUpdateInput; output: MilestoneUpdateOutput }
  module_archive: { input: ModuleArchiveInput; output: ModuleArchiveOutput }
  module_create: { input: ModuleCreateInput; output: ModuleCreateOutput }
  module_open: { input: ModuleOpenInput; output: ModuleOpenOutput }
  module_part_link: { input: ModulePartLinkInput; output: ModulePartLinkOutput }
  module_part_unlink: { input: ModulePartUnlinkInput; output: ModulePartUnlinkOutput }
  module_restore: { input: ModuleRestoreInput; output: ModuleRestoreOutput }
  module_set: { input: ModuleSetInput; output: ModuleSetOutput }
  module_update: { input: ModuleUpdateInput; output: ModuleUpdateOutput }
  my_work_get: { input: MyWorkGetInput; output: MyWorkGetOutput }
  package_export: { input: PackageExportInput; output: PackageExportOutput }
  package_install: { input: PackageInstallInput; output: PackageInstallOutput }
  package_list: { input: PackageListInput; output: PackageListOutput }
  package_preview: { input: PackagePreviewInput; output: PackagePreviewOutput }
  package_remove: { input: PackageRemoveInput; output: PackageRemoveOutput }
  person_create: { input: PersonCreateInput; output: PersonCreateOutput }
  pin_set: { input: PinSetInput; output: PinSetOutput }
  plan_import: { input: PlanImportInput; output: PlanImportOutput }
  project_get: { input: ProjectGetInput; output: ProjectGetOutput }
  project_list: { input: ProjectListInput; output: ProjectListOutput }
  recurring_create: { input: RecurringCreateInput; output: RecurringCreateOutput }
  recurring_update: { input: RecurringUpdateInput; output: RecurringUpdateOutput }
  request_cancel: { input: RequestCancelInput; output: RequestCancelOutput }
  request_create: { input: RequestCreateInput; output: RequestCreateOutput }
  request_get: { input: RequestGetInput; output: RequestGetOutput }
  request_list: { input: RequestListInput; output: RequestListOutput }
  request_respond: { input: RequestRespondInput; output: RequestRespondOutput }
  run_activity_list: { input: RunActivityListInput; output: RunActivityListOutput }
  run_activity_report: { input: RunActivityReportInput; output: RunActivityReportOutput }
  run_cancel: { input: RunCancelInput; output: RunCancelOutput }
  run_create: { input: RunCreateInput; output: RunCreateOutput }
  run_detail_get: { input: RunDetailGetInput; output: RunDetailGetOutput }
  run_mark_not_started: { input: RunMarkNotStartedInput; output: RunMarkNotStartedOutput }
  run_result_report: { input: RunResultReportInput; output: RunResultReportOutput }
  run_update: { input: RunUpdateInput; output: RunUpdateOutput }
  run_usage_report: { input: RunUsageReportInput; output: RunUsageReportOutput }
  scheduled_message_update: { input: ScheduledMessageUpdateInput; output: ScheduledMessageUpdateOutput }
  task_archive: { input: TaskArchiveInput; output: TaskArchiveOutput }
  task_claim: { input: TaskClaimInput; output: TaskClaimOutput }
  task_comment_create: { input: TaskCommentCreateInput; output: TaskCommentCreateOutput }
  task_comment_delete: { input: TaskCommentDeleteInput; output: TaskCommentDeleteOutput }
  task_comment_update: { input: TaskCommentUpdateInput; output: TaskCommentUpdateOutput }
  task_create: { input: TaskCreateInput; output: TaskCreateOutput }
  task_create_many: { input: TaskCreateManyInput; output: TaskCreateManyOutput }
  task_criteria_update: { input: TaskCriteriaUpdateInput; output: TaskCriteriaUpdateOutput }
  task_get: { input: TaskGetInput; output: TaskGetOutput }
  task_import: { input: TaskImportInput; output: TaskImportOutput }
  task_link: { input: TaskLinkInput; output: TaskLinkOutput }
  task_link_many: { input: TaskLinkManyInput; output: TaskLinkManyOutput }
  task_list: { input: TaskListInput; output: TaskListOutput }
  task_move: { input: TaskMoveInput; output: TaskMoveOutput }
  task_ready_list: { input: TaskReadyListInput; output: TaskReadyListOutput }
  task_release: { input: TaskReleaseInput; output: TaskReleaseOutput }
  task_reorder: { input: TaskReorderInput; output: TaskReorderOutput }
  task_restore: { input: TaskRestoreInput; output: TaskRestoreOutput }
  task_update: { input: TaskUpdateInput; output: TaskUpdateOutput }
  task_update_many: { input: TaskUpdateManyInput; output: TaskUpdateManyOutput }
  task_watch_set: { input: TaskWatchSetInput; output: TaskWatchSetOutput }
  template_create: { input: TemplateCreateInput; output: TemplateCreateOutput }
  template_list: { input: TemplateListInput; output: TemplateListOutput }
  template_update: { input: TemplateUpdateInput; output: TemplateUpdateOutput }
  thread_start: { input: ThreadStartInput; output: ThreadStartOutput }
  thread_update: { input: ThreadUpdateInput; output: ThreadUpdateOutput }
  view_create: { input: ViewCreateInput; output: ViewCreateOutput }
  view_list: { input: ViewListInput; output: ViewListOutput }
  view_update: { input: ViewUpdateInput; output: ViewUpdateOutput }
  whiteboard_create: { input: WhiteboardCreateInput; output: WhiteboardCreateOutput }
  whiteboard_duplicate: { input: WhiteboardDuplicateInput; output: WhiteboardDuplicateOutput }
  whiteboard_link: { input: WhiteboardLinkInput; output: WhiteboardLinkOutput }
  whiteboard_update: { input: WhiteboardUpdateInput; output: WhiteboardUpdateOutput }
  whiteboard_update_elements: { input: WhiteboardUpdateElementsInput; output: WhiteboardUpdateElementsOutput }
  workspace_delete: { input: WorkspaceDeleteInput; output: WorkspaceDeleteOutput }
  workspace_delete_cancel: { input: WorkspaceDeleteCancelInput; output: WorkspaceDeleteCancelOutput }
  workspace_search: { input: WorkspaceSearchInput; output: WorkspaceSearchOutput }
  workspace_shape_set: { input: WorkspaceShapeSetInput; output: WorkspaceShapeSetOutput }
}

export type OpName = keyof Operations
