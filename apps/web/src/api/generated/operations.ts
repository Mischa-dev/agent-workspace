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

export type AccountLine = { accountId: string; bankedResets?: number | null; connected: boolean; harness: string; name: string; readyTasks: number; windows: WindowLine[] }

export type Action = ({ agentProfileId?: string | null; environmentId?: string | null; kind: "launch"; prompt?: string | null }) | { field: string; kind: "set_field"; value: string } | { kind: "notify"; message: string; recipients: Recipients } | { conversationId: string; kind: "post_message"; text: string } | { input: unknown; kind: "run_operation"; operation: string } | ({ accountId: string; conversationId?: string | null; kind: "ask_agent"; prompt: string })

export type ActivityEntry = { actor?: string | null; actorId?: string | null; at: string; changed: string[]; position: number; requestedBy?: string | null; requestedById?: string | null; summary: string; type: string; version?: number | null; via?: string | null }

export type ActivityLine = { kind: string; occurredAt: string; parentRef?: string | null; payload: unknown; seq: number; stepRef?: string | null; summary: string }

export type AgentSettingsView = { accountId: string; ambient: boolean; displayName: string; handle: string; replyCooldownSeconds: number }

export type AgentTemplateView = { definition: unknown; id: string; installId?: string | null; key: string; name: string; purpose: string; spaceId?: string | null }

export type AgentUsageLine = { accountId: string; cachedInputTokens: number; confidence: string; cost?: number | null; currency?: string | null; handle?: string | null; inputTokens: number; name?: string | null; outputTokens: number; runs: number }

export type Aggregate = ("sum" | "average" | "min" | "max") | "count" | "count_values" | "count_unique" | "earliest" | "latest" | "percent_checked" | "percent_empty" | "list"

export type AnswerEntry = { columns: string[]; durationMs?: number | null; error?: string | null; parameters: unknown; ranAt?: string | null; reviewed: boolean; rowCount: number; rows: unknown[]; running: boolean; truncated: boolean }

export type ArchiveParts = "archive" | "keep"

export type AssigneeProjection = { actorId: string; displayName: string; handle: string }

export type Audience = "public" | "internal"

export type AudienceInput = { accounts?: string[]; roles?: string[] }

export type AvatarLine = { accountId: string; url: string }

export type BatchTaskLine = { environment: string; key: string; priority: string; project: string; projectId: string; reasons: string[]; taskId: string; title: string; unblocks: number }

export type BlockLine = { hash: string; id: string; level?: number | null; markdown?: string | null; position: number; preview?: string | null; type: string }

export type BlockedAccount = { accountId: string; blockedAt: string; name?: string | null }

export type BlockedProjection = { blockedAt: string; blockedByActorId: string; reason: string }

export type BrowsedChannel = { autoJoin: boolean; id: string; joined: boolean; memberCount: number; name: string; private: boolean; topic?: string | null }

export type BudgetMeter = { id: string; name: string; unit: string }

export type BudgetPeriod = "minute" | "hour" | "day" | "month"

export type BudgetScope = "workspace" | "account" | "space" | "conversation"

export type BudgetView = { eachAccount: boolean; hardLimit?: number | null; id?: string | null; isDefault: boolean; meter: string; meterName: string; period: string; scope: string; scopeId?: string | null; setBy?: string | null; softLimit?: number | null; unit: string; used: number; version?: number | null }

export type BuiltModuleSummary = { description: string; icon?: string | null; id: string; key: string; name: string; on: boolean; parts: number; spaceId?: string | null }

export type ButtonInput = { label?: string | null; steps: ButtonStepInput[] }

export type ButtonStepInput = { arguments?: Record<string, unknown>; operation: string }

export type ButtonStepView = { arguments: Record<string, unknown>; operation: string; title: string }

export type ButtonView = { label: string; steps: ButtonStepView[] }

export type CandidateField = { id: string; name: string; options?: string[]; takesValue: boolean }

export type CarriedValue = { field: string; followsMove?: boolean; targetField: string; taskId: string; taskKey: string; value: unknown }

export type Category = ("all" | "mentions" | "tasks" | "runs" | "pull_requests") | "threads" | "usage"

export type ChangeSetView = { actor?: string | null; actorId: string; createdAt: string; id: string; proposalId?: string | null; thingId: string; thingKind: string; touched: unknown; undoable: boolean; undoes?: string | null; undoneBy?: string | null; updatedAt: string; versionFrom: number; versionTo: number }

export type ChangedTask = { finished?: boolean | null; id: string; key: string; status?: string | null; statusGroup?: string | null; title?: string | null }

export type ChartStyle = ("bar" | "line" | "donut") | "number"

export type ChartWrite = { field?: string | null; measure?: MeasureOp | null; style?: ChartStyle | null }

export type ChipTone = "muted" | "agent" | "warning" | "success" | "danger"

export type Choice = "keep_mine" | "take_theirs"

export type ClaimRun = { runId: string; state: string }

export type ClaimSession = { externalRef: string; harness: string; state: string; url?: string | null }

export type ClaimValue = { claimedAt: string; claimedBy: string; expiresAt: string; handoff?: HandoffNote | null; note?: string | null; taskId: string }

export type ColumnWrite = { field: string; hidden?: boolean; width?: number | null }

export type CommentItem = { at: string; author: string; files?: FileSummary[]; id: string; replyToCommentId?: string | null; text: string; via?: string | null }

export type CommentLine = { at: string; author?: string | null; authorKind: string; deleted?: boolean; edited?: boolean; id: string; text: string }

export type CommentThreadView = { anchor?: unknown; createdAt: string; id: string; messageCount: number; name: string; outcome?: string | null; status: string; unread: boolean }

export type Conditions = { filters?: Record<string, string>; viewId?: string | null }

export type ConnectedModuleView = { accountId: string; accountName: string; approvedAt: string; approvedBy: string; baseUrl: string; description: string; digest: string; events: string[]; fileCheck?: Record<string, unknown> | null; frameOrigins: string[]; installSentAt?: string | null; key: string; kinds: string[]; manifestUrl: string; moduleId: string; name: string; operations: OperationSummary[]; panels: PanelSummary[]; pending?: ManifestSummary | null; publisher?: string | null; revokedAt?: string | null; scopes: ScopeView[]; state: string; version: string }

export type ConnectionEntry = { address: string; id: string; kind: string; name: string; queries: number; updatedAt: string; version: number }

export type ConnectionView = { accountId: string; consecutiveFailures: number; disabledReason?: string | null; forCredential: boolean; homeWorkspaceId: string; id: string; isDefault: boolean; lastDeliveredAt?: string | null; lastError?: string | null; lastSeenAt?: string | null; lastTest?: LastTest | null; madeThroughApp: boolean; name: string; reachesAll: boolean; state: string; subscriptions: Subscription[]; version: number; webhookFormat: string; webhookUrl?: string | null }

export type Content = { body?: Record<string, unknown> | null; key: string; kind: ContentKind; path?: string | null }

export type ContentKind = "switches" | "layout" | "role" | "space" | "channels" | "module" | "collection" | "form" | "automation" | "page" | "skill" | "agent" | "theme"

export type ContextMessage = { author: string; authorId: string; authorKind: string; createdAt: string; edited?: boolean; messageId: string; removed?: boolean; text: string; trust: string }

export type ConversationFile = { contentType: string; createdAt: string; height?: number | null; id: string; messageId: string; name: string; size: number; width?: number | null }

export type ConversationMember = { displayName: string; handle: string; id: string; kind: string; manager: boolean }

export type ConversationSummary = { id: string; kind: string; lastMessage?: LastMessage | null; members?: string | null; muted?: boolean; name: string; outcome?: string | null; parentId?: string | null; parentName?: string | null; projectId: string; projectName: string; status?: string | null; unreadCount: number; url: string }

export type ConversationTab = { id: string; kind: string; position: number; title: string }

export type ConversationTask = { how: string; id: string; key: string; statusCategory: string; title: string }

export type CreatableKind = "project" | "team"

export type CreatedTask = { assignee?: string | null; criteriaCount: number; id?: string | null; index: number; key: string; labels?: string[]; parentKey?: string | null; ref?: string | null; status: string; title: string; type: string; url?: string | null; version: number }

export type CredentialReach = "space" | "account"

export type CredentialView = { accountId: string; clientName: string; createdAt: string; expiresAt: string; id: string; lastUsedAt?: string | null; reach: string; replacedBy?: string | null; scopes: string[]; spaceId: string; status: string; tokenPrefix: string }

export type CriterionOp = "add" | "edit" | "check" | "uncheck" | "remove" | "move"

export type CriterionOperationArgument = { completed?: boolean | null; criterion?: string | null; op: CriterionOp; position?: number | null; text?: string | null }

export type CriterionOperationOutcome = { changed: boolean; criterionId: string; op: CriterionOp }

export type CriterionValue = { completed: boolean; completedAt?: string | null; completedBy?: string | null; id: string; position: number; text: string }

export type CustomEmojiView = { createdAt: string; createdByAccountId: string; id: string; imageUrl: string; name: string; retired: boolean }

export type Cycle = { completed: number; completedAt?: string | null; endsOn: string; id: string; name: string; number: number; scope: number; snapshots: Snapshot[]; started: number; startsOn: string; state: string; title: string; version: number }

export type DashboardEntry = { description?: string; id: string; name: string; position: number; shared: boolean; tiles: TileEntry[]; updatedAt: string; version: number }

export type DateBucket = "day" | "week" | "month"

export type DecisionArg = "accept" | "reject" | "withdraw"

export type DecisionLine = { decidedAt: string; decidedBy: string; id: string; title: string }

export type DecisionView = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

export type DeliveryView = { attempts: number; createdAt: string; deliveredAt?: string | null; elapsedMs?: number | null; eventId: string; eventPosition: number; eventType: string; lastError?: string | null; lastStatus?: number | null; nextAttemptAt?: string | null; reason: string; state: string }

export type Density = "detailed" | "compact"

export type DependencyChange = { changed: number; key: string; ready?: boolean | null; version: number }

export type DependencyLinks = { blockedBy?: string[] | null; blocks?: string[] | null; taskId: string }

export type DependencyValue = { blockedBy: TaskRef[]; blocks: TaskRef[]; changed?: number; ready?: boolean | null; task: ChangedTask; version?: number }

export type Direction = "asc" | "desc"

export type DmRequestView = { direction: string; state: string }

export type DraftView = { conversationId: string; conversationKind: string; conversationName?: string | null; draft: unknown; previous?: unknown; spaceId: string; updatedAt: string }

export type EditOutcome = { changed: string[]; proposalId?: string | null; results: OperationResult[]; savedRevision?: number | null; sceneSeq: number }

export type EmbedDecl = { description: string; id: string; label: string; piece: Piece; thing: string }

export type Entry = { addressed: boolean; family: string; kind: string; summary: string }

export type EntryView = { description: string; entry: string; error?: string | null; id: string; kind: string; name: string; parts: number; shape: boolean; source: string; sourceId?: string | null; target?: string | null; version: string }

export type EnvironmentValue = { branch?: string | null; harness: string; harnessName: string; id: string; isDefault: boolean; lastCheckMessage?: string | null; lastCheckOk?: boolean | null; name: string; providerEnvironmentId?: string | null; repositoryId?: string | null; runsOn?: string | null; setupNotes: string; version: number }

export type FailedLine = { project: string; reason: string; tasks: number }

export type FieldDecl = { id: string; label: string; markdown?: boolean; options?: string[]; tones?: Record<string, ChipTone>; type: FieldType }

export type FieldFilterArgument = { field: string; op: FieldOp; values?: string[] }

export type FieldOp = "is" | "isNot" | "empty" | "notEmpty" | "contains" | "gt" | "gte" | "lt" | "lte" | "within" | "categoryIs" | "categoryIsNot"

export type FieldType = ("text" | "number" | "date" | "single_choice" | "multiple_choice" | "person" | "url" | "checkbox" | "link") | "long_text" | "status" | "people" | "labels" | "email" | "phone" | "relation" | "rollup" | "file" | "created_time" | "edited_time" | "created_by" | "edited_by" | "auto_id" | "formula" | "button"

export type FieldView = { archived: boolean; button?: ButtonView | null; collection?: string | null; description?: string; formula?: FormulaView | null; id: string; inverse?: string | null; name: string; options?: OptionView[]; position: number; rollup?: RollupView | null; rules?: RulesInput | null; target?: LinkTarget | null; type: FieldType; version: number }

export type FieldWrite = { archived?: boolean | null; button?: ButtonInput | null; description?: string | null; field?: string | null; formula?: string | null; name?: string | null; options?: OptionInput[] | null; pairName?: string | null; position?: number | null; rollup?: RollupInput | null; rules?: RulesInput | null; target?: string | null; twoWay?: boolean | null; type?: string | null }

export type FigureGroup = { color?: string | null; count: number; key: string; name: string; value?: number | null }

export type Figures = { collectionName: string; columns?: string[]; error?: string | null; groups: FigureGroup[]; measure?: string | null; ranAt?: string | null; records: string[]; rows?: unknown[]; running?: boolean; style?: string | null; total: number; truncated?: boolean; value?: number | null; viewName: string; viewType: string }

export type FileItem = { kind: FileKind; mime?: string | null; modifiedAt: string; name: string; ref: string; size?: number | null; webUrl?: string | null }

export type FileKind = "file" | "folder"

export type FileSummary = { commentId?: string | null; contentType: string; fileId: string; height?: number | null; idempotentReplay?: boolean; name: string; size: number; width?: number | null }

export type FilterOp = ("equals" | "notEquals" | "contains" | "notContains" | "greaterThan" | "lessThan" | "atLeast" | "atMost" | "empty" | "notEmpty") | "oneOf"

export type FilterWrite = { field: string; op: FieldOp; values?: string[] }

export type FollowKind = "space" | "task" | "page" | "sheet" | "record" | "whiteboard" | "conversation"

export type FollowLevel = "changes" | "everything"

export type FollowView = { id: string; kind: FollowKind; level: FollowLevel; spaceId?: string | null }

export type FollowedThreadView = { id: string; lastActivityAt: string; messageCount: number; name: string; originMessageId?: string | null; outcome?: string | null; parentId: string; parentKind: string; parentName?: string | null; spaceId: string; spaceName: string; status: string; unread: boolean }

export type FormConfig = { dailyLimit?: number | null; fields?: FormField[]; intro?: string; sections?: FormSection[]; sets?: FormSet[]; thanks?: string }

export type FormField = { field: string; required?: boolean; showIf?: ShowIf | null }

export type FormFieldWrite = { field: string; required?: boolean }

export type FormSection = { before: string; description?: string; title: string }

export type FormSet = { field: string; value: unknown }

export type FormSummary = { archived?: boolean; audience: Audience; collectionId: string; collectionName: string; id: string; linkId?: string | null; name: string; questions: number; version: number }

export type FormWrite = { dailyLimit?: number | null; fields?: FormFieldWrite[]; intro?: string | null; thanks?: string | null }

export type FormulaView = { result: ResultType; source: string }

export type GroupView = { description: string; handle: string; id: string; memberCount: number; memberIds?: string[] | null; name: string }

export type GuardAction = "deny" | "review"

export type GuardView = { accounts: string[]; action: string; enabled: boolean; id: string; name: string; operations: string[]; reviewers: string[]; spaceId?: string | null; targetKinds: string[]; terms: string[]; version: number }

export type GuidePage = { body: string; pageId: string; stale: boolean; title: string; truncated: boolean; updatedAt: string; version: number }

export type HandLinkKind = "about" | "relates_to" | "made_from"

export type HandoffNote = { at: string; by: string; note: string; via?: string | null }

export type HistoryCaps = { diff: boolean; list: boolean; restore: boolean; showVersion: boolean }

export type HistoryEntry = { acceptedBy?: string | null; actor?: string | null; actorId?: string | null; at: string; changed: string[]; summary: string; version: number; via?: string | null }

export type HookDeliveryView = { deliveryId: string; outcome: string; reason?: string | null; receivedAt: string }

export type HookView = { createdAt: string; id: string; lastDeliveryAt?: string | null; name: string; recent: HookDeliveryView[]; spaceId: string; url: string }

export type HostAgentView = { accountId: string; actions: unknown[]; backends: unknown[]; hostAgentRef: string; name?: string | null; status: string }

export type HostOwnerChoice = "person" | "workspace"

export type HostView = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type InboxFilter = { category?: Category; spaces?: string[]; view?: View }

export type InboxItem = { conversationId: string; conversationKind: string; conversationName: string; createdAt: string; followsRequestId?: string | null; generation: number; leaseExpiresAt?: string | null; mentioned: boolean; messageAuthor: string; messageExcerpt: string; messageId: string; otherMembers: string; projectId: string; replyConversationId: string; requestId: string; required: boolean; status: string; threadId?: string | null }

export type InboxViewItem = { density: Density; filter: InboxFilter; id: string; name: string; position: number }

export type Initiative = { archived: boolean; completed: number; fields: InitiativeField[]; id: string; milestones: InitiativeMilestone[]; number: number; scope: number; spaces: InitiativeSpace[]; title: string; version: number }

export type InitiativeField = { name: string; type: string; value: unknown }

export type InitiativeMilestone = { archived: boolean; completed: number; id: string; name: string; scope: number; spaceId: string; spaceKey: string; spaceName: string; targetDate?: string | null }

export type InitiativeSpace = { canUpdate?: boolean | null; health?: string | null; healthLabel?: string | null; latestUpdate?: LatestUpdate | null; lead?: Person | null; spaceId: string; spaceKey: string; spaceName: string; targetDate?: string | null; version: number }

export type InlineAutomation = { collection?: string | null; rule: unknown }

export type InlineCollection = { description?: string | null; fields?: FieldWrite[]; name: string; titleName?: string | null; views?: ViewWrite[] }

export type InlineForm = { collection: string; fields?: InlineFormField[]; intro?: string; name: string; thanks?: string }

export type InlineFormField = { field: string; required?: boolean }

export type InstallTarget = { newSpace?: NewSpace | null; spaceId?: string | null }

export type InstallView = { id: string; installedAt: string; name: string; objects: number; packageId: string; spaceId?: string | null; state: string; summary: string[]; version: string }

export type Installed = { changes: string[]; installId: string; objects: InstalledObject[]; packageId: string; skipped: string[]; spaceId?: string | null; version: string }

export type InstalledObject = { action: string; id: string; key: string; kind: string }

export type InvitationLine = { acceptedAt?: string | null; createdAt: string; expiresAt: string; id: string; issuedBy: string; note?: string | null; role: string; roleId?: string | null; status: string; tokenPrefix: string; version: number }

export type InviteLinkView = { active: boolean; createdAt: string; createdBy: string; expiresAt: string; id: string; maxUses?: number | null; revokedAt?: string | null; roleId: string; roleName: string; spaceId?: string | null; uses: number }

export type IssueReason = "no_matching_field" | "option_missing" | "person_not_in_target" | "link_outside_move" | "page_link"

export type JoinableAccount = { accountId: string; displayName: string; handle: string; handleTaken: boolean; kind: string }

export type LabelLine = { archived: boolean; color: string; description?: string; id: string; name: string; taskCount: number; version: number }

export type LastMessage = { at: string; author: string; removed?: boolean; text: string }

export type LastTest = { at: string; error?: string | null; ok: boolean; status?: number | null }

export type LaterLine = { done: boolean; id: string; kind: string; messageId?: string | null; note?: string; preview?: string | null; project: string; remindAt?: string | null; task?: string | null; url: string }

export type LatestUpdate = { author?: Person | null; channelId: string; health?: string | null; name: string; postedAt: string; threadId: string }

export type LaunchValue = { createdAt: string; environment?: string | null; harness?: string | null; id: string; requestedBy: string; runId?: string | null; runState?: string | null; startAt?: string | null; startWhen: string; state: string; statusReason?: string | null; task?: string | null; taskTitle?: string | null; threadUrl?: string | null }

export type LaunchedLine = { environment: string; project: string; tasks: string[] }

export type Layer = { scope: Scope; updatedAt: string; updatedByActorId?: string | null; value: unknown; version: number }

export type LayoutLine = { fromWorkspace: boolean; preset: string; tone: string; words?: WordLine[] }

export type LayoutPresetLine = { base: string; description: string; show: Record<string, boolean>; version: number; words: Record<string, PresetWord> }

export type LayoutRuleArgument = { person?: string | null; preset: string; role?: string | null }

export type Lifecycle = "active" | "importing" | "suspended" | "deleting"

export type LinkArgument = { targetId: string; targetKind: string }

export type LinkTarget = ("task" | "page") | "record"

export type LinkValue = { changed: boolean; key: string; kind: string; linked: boolean; notes?: string[]; role?: string | null; targetId: string; taskId: string; taskUrl: string; version: number }

export type LinkView = { key?: string | null; projectId: string; projectName: string; targetId: string; targetKind: string; title: string; url: string }

export type LinkedItem = { id: string; kind: string; status?: string | null; text: string; title: string }

export type LinkedRepositoryProjection = { repository: RepositoryProjection; role: string }

export type LinkedTask = { id: string; key: string; status: string; title: string }

export type LinkedThread = { id: string; name: string; url: string }

export type ManifestSummary = { accountName: string; description: string; digest: string; events: string[]; frameOrigins: string[]; key: string; kinds: string[]; name: string; operations: OperationSummary[]; panels: PanelSummary[]; publisher?: string | null; scopes: ScopeView[]; version: string }

export type MeasureOp = "count" | "sum" | "average" | "min" | "max"

export type MemberRef = { handle: string; id: string; kind: string; name: string }

export type MemoryPageView = { body: string; pageId: string; spaceId: string; title: string; updatedAt: string; version: number }

export type MemoryPointer = { head: string; pageId: string; title: string; truncated: boolean; updatedAt: string; version: number }

export type Mention = { author: string; conversation: string; conversationId: string; messageId: string; project: string; sentAt: string; sequence: number; text: string }

export type MessageContent = { nodes: MessageNode[]; version: number }

export type MessageNode = { text: string; type: "text" } | { actorId: string; type: "mention" } | { type: "work_item_ref"; workItemId: string }

export type MeterKind = "gauge" | "counter" | "each"

export type MeterUsage = { kind: MeterKind; limit?: number | null; meter: string; name: string; resetsAt?: string | null; unit: string; used: number }

export type MilestonePlace = { archived: boolean; id: string; name: string; position: number; version: number }

export type MilestoneRef = { id: string; name: string; targetDate?: string | null }

export type MilestoneValue = { archived: boolean; canceled?: number; description: string; estimateDone?: number; estimateTotal?: number; finished: number; id: string; idempotentReplay?: boolean; initiative?: string | null; initiativeId?: string | null; latestDueDate?: string | null; name: string; open?: number; overdue: boolean; position?: number; progress?: number; targetDate?: string | null; total: number; url: string; version: number }

export type ModelUsageLine = { cachedInputTokens: number; confidence: string; cost?: number | null; currency?: string | null; inputTokens: number; model: string; outputTokens: number }

export type ModuleDetail = { embeds: EmbedDecl[]; listens: string[]; permissions: string[]; things: ThingDecl[] }

export type ModuleEntry = { key: string; name: string }

export type ModuleKind = "core" | "built" | "connected"

export type ModulePanelRef = { moduleId: string; panel: string }

export type ModuleView = { description: string; embeds: string[]; events: string[]; id: string; kind: ModuleKind; name: string; needs: string[]; offBecause?: string | null; on: boolean; switchable: boolean; switchedOn: boolean; things: string[]; tools: Record<string, string[]>; version: number; worksWith: string[] }

export type MoveIssue = { candidates?: CandidateField[]; field: string; fieldId: string; fieldType: FieldType; id: string; linkTarget?: LinkTarget | null; message: string; reason: IssueReason; taskId: string; taskKey: string; value: unknown }

export type MovePlan = { archivedFieldValues?: number; carried?: CarriedValue[]; issues?: MoveIssue[]; notes?: string[]; snapshot: string; targetProjectId: string; targetProjectKey: string; tasks: PlannedTask[] }

export type MovedTask = { from: string; id: string; title: string; to: string; url: string }

export type NamedProjection = { id: string; name: string }

export type NamedRange = { name: string; range: string; tab: string; tabId: string }

export type NavEntry = { icon?: string | null; label: string; target: unknown }

export type NavView = { icon?: string | null; id: string; label: string; target: unknown }

export type NewSpace = { key: string; name: string }

export type NewTask = { acceptanceCriteria?: string[]; assignee?: string | null; blockedByRefs?: string[] | null; description?: string | null; dueDate?: string | null; estimate?: number | null; fields?: Record<string, unknown> | null; labels?: string[]; milestone?: string | null; parentId?: string | null; parentRef?: string | null; priority?: string | null; ref?: string | null; startDate?: string | null; status?: string | null; title: string; type?: string | null }

export type NotificationLine = { at: string; count: number; id: string; kind: string; preview?: string | null; project: string; read: boolean; snoozedUntil?: string | null; subject: string; subjectId: string; subjectKind: string; title: string; untilActivity?: boolean; url: string }

export type Offer = { hostId: string; hostName: string; offer: Record<string, unknown> }

export type Operation = { arrowheads?: string | null; axis?: string | null; backgroundColor?: string | null; dataBase64?: string | null; direction?: string | null; dx?: number | null; dy?: number | null; edge?: string | null; fileId?: string | null; fillStyle?: string | null; fontSize?: number | null; frameId?: string | null; from?: string | null; gap?: number | null; height?: number | null; id?: string | null; ids?: string[] | null; link?: string | null; locked?: boolean | null; mimeType?: string | null; name?: string | null; op: string; opacity?: number | null; points?: number[][] | null; position?: string | null; roughness?: number | null; rounded?: boolean | null; strokeColor?: string | null; strokeStyle?: string | null; strokeWidth?: number | null; text?: string | null; textAlign?: string | null; to?: string | null; type?: string | null; width?: number | null; x?: number | null; y?: number | null }

export type OperationResult = { ids: string[]; op: string }

export type OperationSummary = { name: string; permission?: string | null; verb: string }

export type OptionInput = { category?: string | null; color?: string | null; id?: string | null; name: string }

export type OptionView = { archived?: boolean; category?: string | null; color: string; id: string; name: string }

export type OutlineLine = { hash: string; id: string; level: number; position: number; sectionEnd: number; text: string }

export type OutsideRef = { at: string; externalId: string; hostId?: string | null; issuer: string; lineage: unknown[] }

export type OwnerView = { id: string; kind: string; name: string }

export type Package = { contents: Content[]; description?: string; digest?: string | null; format: string; id: string; name: string; needsAdmin?: string[]; publisher?: Publisher | null; requires?: Requires; target: PackageTarget; version: string }

export type PackageTarget = "space" | "workspace"

export type PageEditInput = { blockId?: string | null; hash?: string | null; markdown?: string | null; op: string; position?: number | null }

export type PageLine = { id: string; stale: boolean; title: string; updatedAt: string }

export type PageLineValue = { excerpt: string; id: string; isGuide: boolean; isTemplate?: boolean; skillDescription?: string | null; stale: boolean; title: string; updatedAt: string; updatedBy?: string | null; version: number }

export type PageTemplateView = { builtIn: boolean; excerpt: string; id: string; projectName?: string | null; title: string }

export type PanelActionInput = { id: string; values?: Record<string, unknown> | null }

export type PanelAnswer = { kind: "panel"; panel: Record<string, unknown> } | { errors: Record<string, unknown>; kind: "errors" } | ({ kind: "done"; message?: string | null; result?: Record<string, unknown> | null }) | { kind: "close" } | { address: string; height: number; kind: "frame"; launchId: string; origin: string; title: string }

export type PanelEntry = { moduleId: string; moduleKey: string; panel: string; part: string; slot: string; title: string }

export type PanelSummary = { id: string; slot: string; title: string }

export type PanelThingInput = { id: string; kind: string }

export type Parameter = { default?: unknown; name: string; type: ParameterKind }

export type ParameterKind = ("text" | "number" | "boolean") | "date"

export type PartRef = { id: string; kind: string }

export type PartView = { id: string; kind: string; name?: string | null }

export type PermissionEntry = { builtIn: string[]; key: string; label: string; level?: string | null; meaning: string; module: string; required: boolean; scope: string; thingKind?: string | null }

export type Person = { id: string; name: string }

export type PersonFound = { handle: string; id: string; kind: string; name: string; projects: string[] }

export type Piece = { actions?: string[]; fields?: string[]; groupBy?: string | null; thing: string; type: PieceType }

export type PieceType = "list" | "table" | "board" | "detail" | "fields" | "form" | "card" | "actions"

export type PinItem = { id: string; kind: string; text: string; title: string }

export type PlaceHit = { detail?: string | null; id: string; kind: string; path?: string | null; title: string }

export type PlanItem = { action: string; existingId?: string | null; key: string; kind: string; name: string; needsAdmin: boolean; reason?: string | null }

export type PlannedTask = { id: string; key: string; parentId?: string | null; title: string }

export type PresetWord = { many: string; one: string }

export type PressedStep = { operation: string; value: unknown }

export type PreviewRecord = { number: number; title: string }

export type Problem = { message: string; path: string }

export type ProjectHeader = { id: string; key: string; name: string; role: string; url: string }

export type ProjectLine = { archived?: boolean; id: string; key: string; messagesSpace: boolean; name: string; openTasks: number; role: string; unreadConversations: number; url: string; version: number }

export type ProjectTemplateLine = { archived: boolean; canEdit: boolean; channels: string[]; createdBy?: string | null; description: string; id: string; labels: number; name: string; settings: number; sourceProject?: string | null; statuses: string[]; taskTemplates: number; types: string[]; version: number }

export type ProposalView = { appliedVersion?: number | null; author?: string | null; authorId: string; baseVersion: number; change: unknown; createdAt: string; decidedAt?: string | null; decidedBy?: string | null; decisionNote?: string | null; id: string; note?: string | null; operation: string; preview: unknown; spaceId: string; status: string; thingId: string; thingKind: string; thingTitle?: string | null }

export type PublicField = { description?: string; id: string; name: string; options?: PublicOption[]; required: boolean; section?: PublicSection | null; showIf?: ShowIf | null; type: string }

export type PublicOption = { color: string; id: string; name: string }

export type PublicSection = { description?: string; title: string }

export type Publisher = { name: string; url?: string | null }

export type PullRequest = { baseRef: string; checksState: string; draft: boolean; headRef: string; id: string; mergedAt?: string | null; number: number; repository: string; repositoryId: string; reviewState: string; spaceId: string; state: string; title: string; updatedAt: string; url: string }

export type PullRequestLine = { checksState: string; draft: boolean; failingChecks?: string[]; id: string; lastSyncedAt?: string | null; mergeabilityState: string; needsYou?: string[]; number: number; pendingChecks?: number; project: string; projectId: string; projectKey: string; repository: string; repositoryId: string; requestedReviewers?: string[]; reviewState: string; runId?: string | null; state: string; syncState: string; tasks?: LinkedTask[]; thread?: LinkedThread | null; title: string; updatedAt: string; url: string; version: number }

export type PullRequestProjection = { baseRef: string; canonicalUrl: string; checksState: string; draft: boolean; headRef: string; headSha: string; id: string; mergeabilityState: string; number: number; projectId: string; repositoryId: string; reviewState: string; state: string; suggested?: boolean; syncState: string; title: string; url: string; version: number }

export type QueryEntry = { answer?: AnswerEntry | null; canRun: boolean; connection?: string | null; description?: string; id: string; name: string; parameters: Parameter[]; review: ReviewState; reviewedAt?: string | null; reviewedBy?: string | null; rowCap?: number | null; shared: boolean; statement?: string | null; updatedAt: string; version: number }

export type RaisedRequestOutput = { activityId: string; clientRequestId?: string | null; requestId: string }

export type ReactionSummary = { byYou: boolean; count: number; emoji: string; name: string; people: string[] }

export type RecipeView = { id: string; title: string }

export type Recipients = "owner" | "assignee" | "project_admins"

export type RecurringView = { collectionId: string; enabled: boolean; every: number; frequency: string; id: string; lastRecordId?: string | null; lastRunAt?: string | null; monthDay?: number | null; nextRunAt?: string | null; ownerId: string; ownerName?: string | null; pausedReason?: string | null; startsOn: string; templateId: string; templateName: string; timeOfDay: string; timezone: string; version: number; weekdays?: number[] }

export type Refreshed = "unchanged" | "applied" | "pending"

export type RelationRef = { kind: string; task: TaskRef }

export type RelationsValue = { changed: boolean; relations: RelationRef[]; task: ChangedTask; version: number }

export type RemoveParts = "archive" | "keep"

export type ReorderedTask = { boardPosition: number; id: string; key: string; version: number }

export type RepositoryItem = { activeRuns: number; archived?: boolean; cloneUrl: string; defaultBranch: string; enabled: boolean; github: string; id: string; name: string; notReadyBecause?: string | null; openPullRequests: number; preparationFailure?: string | null; preparationState: string; preparedAt?: string | null; project: string; projectId: string; projectKey: string; readyForRuns: boolean; settingsUrl: string; trusted: boolean; version: number }

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

export type Resolved = { default: unknown; fallbackValue?: unknown; key: string; layers: Layer[]; overrides?: Source | null; source: Source; value: unknown }

export type ResultType = "number" | "text" | "boolean" | "date" | "list"

export type ReviewState = "draft" | "reviewed"

export type RevisionItem = { redacted?: boolean; revision: number; text: string; truncated?: boolean; via?: string | null; writtenAt: string; writtenBy: string }

export type RevisionLine = { at: string; author?: string | null; note?: string | null; version: number; via?: string | null }

export type RoleCapabilities = { assign: boolean; edit: boolean; editPermissions: boolean; invite: boolean; remove: boolean }

export type RoleDefaults = { channels?: string[]; homeHidden?: string[]; layoutPreset?: string | null; pinnedViews?: string[] | null }

export type RoleView = { actWithinRequester: boolean; agentLimit?: string[] | null; baseRole: string; builtIn: boolean; capabilities: RoleCapabilities; defaults: RoleDefaults; description: string; id: string; isDefault: boolean; key: string; modulesOff: string[]; name: string; permissions: string[]; projectHolders: number; updatedAt: string; version: number; workspaceHolders: number }

export type RollupInput = { aggregate: string; field?: string | null; relation: string }

export type RollupView = { aggregate: Aggregate; relation: string; result: ResultType; target?: string | null }

export type Rule = { action: RuleAction; enabled: boolean; filter: unknown; id: string; name: string; position: number; version: number }

export type RuleAction = { kind: RuleActionKind; status?: string | null; to?: string | null }

export type RuleActionKind = "accept" | "assign" | "keep"

export type RulesInput = { buildersOnly?: boolean; default?: unknown; max?: number | null; min?: number | null; pattern?: string | null; required?: boolean; unique?: boolean }

export type RunActivityInput = { at?: string | null; id: string; kind: string; parentRef?: string | null; payload?: unknown; stepRef?: string | null; summary?: string | null }

export type RunCauseInput = { conversationId?: string | null; kind: string; messageId?: string | null; ref?: string | null; taskId?: string | null }

export type RunPullRequestLine = { checks?: string | null; number: number; review?: string | null; state: string; title: string; url: string }

export type RunRecord = { activityCount: number; cancelRequestedAt?: string | null; clientRunId?: string | null; externalUrl?: string | null; finishedAt?: string | null; liveness: string; reportIntervalSeconds: number; runId: string; state: string; statusLine?: string | null; title: string; version: number }

export type RunResultInput = { branch?: string | null; id?: string | null; kind: string; number?: number | null; thingKind?: string | null; title?: string | null; url?: string | null }

export type RunValue = { approvalsWaiting: number; branch?: string | null; createdAt: string; failure?: string | null; id: string; liveness: string; next: string; pollAfterSeconds?: number | null; projectId?: string | null; prompt: string; pullRequest?: RunPullRequestLine | null; queuePosition?: number | null; stage: string; startedBy: string; state: string; statusLine?: string | null; threadId?: string | null; updatedAt: string; url: string }

export type SavedSearch = { id: string; name: string; position: number; query: string }

export type ScheduleInput = { every?: number | null; frequency: string; monthDay?: number | null; startsOn?: string | null; timeOfDay?: string | null; timezone?: string | null; weekdays?: number[] }

export type ScheduleView = { createdAt: string; everyMinutes?: number | null; id: string; lastDueAt?: string | null; nextDueAt?: string | null; note: string; state: string; thingId?: string | null; thingKind?: string | null; timesDue: number; version: number }

export type ScheduledLine = { conversationId: string; failure?: string | null; id: string; place: string; sendAt: string; status: string; text: string; url: string }

export type Scope = "workspace" | "project" | "user"

export type ScopeAccount = { accountId: string; displayName?: string | null }

export type ScopeConversation = { conversationId: string; heard: boolean; kind: string; members: ScopeMember[]; name?: string | null; workspaceId: string }

export type ScopeMember = { accountId: string; displayName?: string | null; you: boolean }

export type ScopePermissions = { forApps: string[]; permissions: string[]; role: string; roleId?: string | null; roleName: string }

export type ScopeSpace = { kind: string; name: string; role: string; spaceId: string }

export type ScopeView = { approved: boolean; known: boolean; label: string; required: boolean; scope: string; why: string }

export type SearchResult = { at: string; author?: string | null; conversation?: string | null; conversationId?: string | null; id: string; kind: string; project: string; projectId?: string | null; snippet: string; title: string; url: string }

export type SectionShow = "all" | "unread"

export type SectionSort = "manual" | "alpha" | "recent"

export type SentMessage = { conversationId: string; idempotentReplay: boolean; messageId: string; sentAt: string; sequence: number; text: string }

export type SettingCapabilities = { manageOwn: boolean; manageProject: boolean; manageWorkspace: boolean }

export type Settings = { enabled: boolean; lengthDays: number; rollOver: boolean; startsOnWeekday: number; upcoming: number; version: number }

export type ShapeChannel = { name: string; private?: boolean }

export type ShapePage = { guide?: boolean; markdown?: string; title: string }

export type SharedAccount = { account: string; expiresAt?: string | null }

export type SharedLink = { expiresAt?: string | null; id: string }

export type SharedSpace = { id: string; name: string }

export type SheetMatch = { cell: string; input: string; shown: string; tab: string }

export type SheetViewOut = { filters: ViewFilter[]; hiddenColumns: string[]; id: string; name: string; shared: boolean; sheetId: string; sort: ViewSort[]; tab: string; tabId: string; updatedAt: string; version: number }

export type ShowIf = { field: string; isOneOf?: string[] }

export type SidebarItem = { conversationId: string; hidden: boolean; kind: string; lastMessageAt?: string | null; name: string; position: number; private: boolean; sectionId: string; spaceId: string; unread: boolean }

export type SidebarSection = { collapsed: boolean; id: string; items: SidebarItem[]; kind: string; name: string; position: number; show: string; sort: string; version: number }

export type SkillLine = { description: string; name: string; pageId: string; stale: boolean; title: string; updatedAt: string; version: number }

export type SkippedLine = { project: string; reason: string }

export type SkippedProject = { projectId: string; reason: string }

export type Snapshot = { completed: number; completedPoints?: number | null; onDate: string; scope: number; scopePoints?: number | null; started: number }

export type SortWrite = { direction?: Direction; field: string }

export type Source = "launch" | "project" | "user" | "workspace" | "default"

export type SourceView = { error?: string | null; fetchedAt?: string | null; id: string; name: string; url: string }

export type SpaceKind = "workspace" | "project" | "team" | "personal"

export type SpaceShape = { channels?: ShapeChannel[]; kind?: string | null; pages?: ShapePage[] }

export type SpaceStatus = { canUpdate?: boolean | null; health?: string | null; healthLabel?: string | null; latestUpdate?: LatestUpdate | null; lead?: Person | null; spaceId: string; targetDate?: string | null; version: number }

export type SpaceView = { archived: boolean; description: string; icon?: string | null; id: string; key: string; kind: StoredKind; name: string; role?: string | null; url: string; version: number; visibility: SpaceVisibility; workspaceId: string }

export type SpaceVisibility = "members" | "workspace"

export type StartingPoint = { agentLimit?: string[] | null; baseRole: string; defaults: RoleDefaults; description: string; key: string; name: string; permissions: string[] }

export type StatusCount = { category: string; count: number; status: string }

export type StatusProjection = { id: string; name: string; semanticCategory: string }

export type StatusView = { accountId: string; awayUntil?: string | null; clearAt?: string | null; emoji?: string | null; state?: string | null; text?: string | null; updatedAt: string; workspaceOnly: boolean }

export type StoredKind = SpaceKind

export type SubjectKind = "page" | "sheet" | "whiteboard" | "record" | "task"

export type Subscription = { conversationIds?: string[] | null; echoes: boolean; id: string; includesDms: boolean; kinds: string[]; minReason: string; spaceIds?: string[] | null; workspaceIds?: string[] | null }

export type SubscriptionInput = { conversationIds?: string[] | null; echoes?: boolean | null; includesDms?: boolean | null; kinds?: string[] | null; minReason?: string | null; spaceIds?: string[] | null; workspaceIds?: string[] | null }

export type SuggestedEdit = { before?: string | null; blockId?: string | null; markdown?: string | null; op: string }

export type SuggestionLine = { appliedVersion?: number | null; at: string; author?: string | null; authorId: string; baseVersion: number; decidedBy?: string | null; edits: SuggestedEdit[]; id: string; note?: string | null; status: string }

export type SuggestionMade = { message: string; status: string; suggestionId: string }

export type TaskFieldValue = { name: string; type: FieldType; value: unknown }

export type TaskHistory = { entries: TaskHistoryEntry[]; olderCursor?: string | null; total: number }

export type TaskHistoryEntry = { at: string; by: string; byHandle: string; byKind: string; changedFields?: string[]; criterionId?: string | null; fromStatus?: string | null; fromTaskId?: string | null; kind: string; sequence: number; text: string; textTruncated?: boolean; toStatus?: string | null; via?: string | null }

export type TaskLine = { claimedBy?: string | null; id: string; key: string; priority: string; project: string; status: string; title: string; updatedAt: string }

export type TaskOverview = { blocked?: TaskLine[]; byStatus: StatusCount[]; claimed?: TaskLine[]; next: TaskLine[] }

export type TaskRef = { finished: boolean; id: string; key: string; status: string; statusGroup?: string; title: string }

export type TaskSummary = { archived?: boolean; assignee?: string | null; blocked: boolean; boardPosition: number; claimedBy?: string | null; criteriaDone?: number | null; criteriaTotal?: number | null; dueDate?: string | null; fields?: Record<string, unknown> | null; id: string; key: string; labels?: string[]; milestone?: string | null; openBlockerCount?: number; overdue?: boolean; priority: string; project: string; ready?: boolean; status: string; statusGroup: string; title: string; type: string; updatedAt: string; version: number }

export type TemplateSubtask = { description?: string | null; title: string }

export type TemplateUse = { projectId?: string | null; summary: string[]; template: string; templateName: string; usedAt: string; usedBy: string }

export type TemplateView = { archived?: boolean; collectionId: string; fields: Record<string, unknown>; id: string; name: string; title: string; version: number }

export type ThemeDocument = { css?: string; dark: ThemeMode; format: number; light: ThemeMode }

export type ThemeMode = { base: string; tokens?: Record<string, string> }

export type ThemeSummary = { darkBase: string; id: string; lightBase: string; mine: boolean; name: string; offered: boolean; ownerAccountId: string; updatedAt: string; version: number }

export type ThingDecl = { fields?: FieldDecl[]; href: string; kind: string; many: string; one: string }

export type ThingInput = { id: string; kind: SubjectKind }

export type ThingKind = "page"

export type ThreadLine = { anchorFound: boolean; blockId?: string | null; comments: CommentLine[]; id: string; quote?: string | null; resolved: boolean; resolvedBy?: string | null }

export type ThreadRef = { id: string; name: string; status: string }

export type ThreadUpdate = { changedAt: string; changedBy?: string | null; name: string; outcome?: string | null; status: string; threadId: string; under: string }

export type TileEntry = { collection?: string | null; figures?: Figures | null; height: number; id: string; kind: TileKind; query?: string | null; range?: string; sheet?: string | null; style?: ChartStyle | null; text?: string; title?: string; unseen?: string | null; view?: string | null; width: number }

export type TileInput = { height?: number | null; id?: string | null; kind?: TileKind | null; query?: string | null; range?: string | null; sheet?: string | null; style?: ChartStyle | null; text?: string | null; title?: string | null; view?: string | null; width?: number | null }

export type TileKind = "view" | "text" | "sheet" | "query"

export type TransferOutput = { byteSize?: number | null; counts: unknown; createdAt: string; direction: string; downloadUrl?: string | null; error?: string | null; expiresAt?: string | null; finishedAt?: string | null; id: string; requestedByActorId: string; state: string }

export type TriageAction = "accept" | "decline" | "duplicate" | "snooze"

export type TriagedTask = { snoozedUntil?: string | null; status: string; taskId: string; version: number }

export type Trigger = { kind: "task_created" } | { kind: "task_matches" } | { kind: "run_finished"; outcomes?: string[] } | { kind: "pull_request_checks_failed" } | ({ contains?: string | null; conversationId?: string | null; kind: "message_posted" }) | ({ every?: number; frequency: string; kind: "schedule"; monthDay?: number | null; time: string; timezone: string; weekdays?: number[] }) | ({ collectionId?: string | null; kind: "record_created" }) | ({ collectionId?: string | null; kind: "record_changed" }) | ({ collectionId: string; filters?: FilterWrite[]; kind: "record_matches"; viewId?: string | null }) | ({ collectionId: string; field: string; filters?: FilterWrite[]; kind: "field_changed"; to?: string[]; viewId?: string | null }) | ({ kind: "page_changed"; pageId?: string | null }) | ({ kind: "sheet_changed"; sheetId?: string | null }) | ({ kind: "whiteboard_changed"; whiteboardId?: string | null }) | ({ eventType?: string | null; kind: "module_event" }) | { hookId: string; kind: "incoming_hook" } | ({ kind: "capacity_window"; maxTasks?: number | null; providerAccountId?: string | null })

export type UnattributedUsageLine = { accountId: string; cost?: number | null; currency?: string | null; inputTokens: number; name?: string | null; outputTokens: number; reports: number }

export type UpdatedTask = { archived: boolean; changedFields?: string[]; id: string; index: number; key: string; status: string; url: string; version: number }

export type View = ("snoozed" | "archived") | "inbox" | "unread"

export type ViewEntry = { builtin?: string | null; collectionId: string; config: unknown; id: string; listParams?: Record<string, string> | null; name: string; pinPosition?: number | null; pinned: boolean; position: number; shared: boolean; type: string; version: number }

export type ViewFilter = { column: string; op: FilterOp; value?: unknown; values?: unknown[] }

export type ViewSort = { column: string; descending?: boolean }

export type ViewWrite = { chart?: ChartWrite | null; columns?: ColumnWrite[] | null; dateField?: string | null; endField?: string | null; filters?: FilterWrite[] | null; groupBucket?: DateBucket | null; groupBy?: string | null; name?: string | null; position?: number | null; remove?: boolean | null; sort?: SortWrite[] | null; titleWidth?: number | null; type?: string | null; view?: string | null }

export type WatcherRef = { handle: string; kind: string; name: string }

export type WhiteboardLine = { createdAt: string; createdBy?: string | null; deleted: boolean; elementCount: number; id: string; isTemplate: boolean; linkCount: number; organizationId: string; organizationName: string; projectId?: string | null; projectName?: string | null; sceneSeq: number; sceneUpdatedAt: string; title: string; updatedAt: string; updatedBy?: string | null; updatedByKind?: string | null; url: string; version: number }

export type WhiteboardTemplateView = { builtIn: boolean; id: string; projectName?: string | null; title: string }

export type WindowLine = { label: string; limitReached: boolean; resetsAt?: string | null; source: string; unusedPercent?: number | null; usedPercent?: number | null }

export type WordLine = { many: string; noun: string; one: string }

export type WorkItemProjection = { acceptanceCriteria: AcceptanceCriterionProjection[]; archived: boolean; assignee?: AssigneeProjection | null; blocked?: BlockedProjection | null; canonicalUrl: string; delegate?: AssigneeProjection | null; description: string; descriptionTruncated: boolean; id: string; itemType: NamedProjection; key: string; labels: string[]; linkedPullRequests: PullRequestProjection[]; linkedRepositories: LinkedRepositoryProjection[]; parentWorkItemId?: string | null; priority: string; projectId: string; status: StatusProjection; title: string; version: number }

export type WorkflowStatus = { category: string; id: string; isDefault: boolean; name: string }

export type WorkflowStatusDraft = { category: string; id?: string | null; isDefault?: boolean | null; key?: string | null; name: string }

export type WorkflowTransition = { from: string; to: string }

export type WorkflowTransitionDraft = { from: string; to: string }

export type WorkflowType = { id: string; isDefault: boolean; name: string }

export type WorkflowTypeDraft = { id?: string | null; isDefault?: boolean | null; key?: string | null; name: string }

export type WorkflowValue = { estimateUnit?: string; statuses: WorkflowStatus[]; transitions: WorkflowTransition[]; types: WorkflowType[]; version?: number }

export type WorkspaceStoreView = { bucket: string; endpoint: string; pathStyle: boolean; prefix: string; region: string; storeId: string; verifiedAt: string }

export type WorkspaceTemplateView = { builtIn: boolean; definition: unknown; description: string; id: string; name: string; savedAt?: string | null; savedBy?: string | null; whatItDoes: string[] }

export type You = { access: string; actorId: string; connectedThrough?: string | null; handle: string; kind: string; name: string }

export type AccountBlockListInput = Record<string, never>

export type AccountBlockListOutput = { blocked: BlockedAccount[] }

export type AccountBlockSetInput = { account: string; blocked: boolean }

export type AccountBlockSetOutput = { blocked: BlockedAccount[] }

export type AccountJoinableListInput = Record<string, never>

export type AccountJoinableListOutput = { accounts: JoinableAccount[] }

export type AccountOwnerTakeInput = { account: string }

export type AccountOwnerTakeOutput = { account: string; ownerAccount: string }

export type AccountWorkspaceAddInput = { account: string; handle?: string | null; roleId?: string | null }

export type AccountWorkspaceAddOutput = { accountId: string; requestId?: string | null; spaceId?: string | null; status: string }

export type ActivityListInput = { before?: number | null; limit?: number | null; thing: string }

export type ActivityListOutput = { entries: ActivityEntry[]; kind: string }

export type AgentActionGetInput = { commandId: string }

export type AgentActionGetOutput = { commandId: string; panel?: ModulePanelRef | null; reply?: Record<string, unknown> | null; state: string }

export type AgentActionRunInput = { account?: string | null; actionId: string; host?: string | null; input?: Record<string, unknown> | null }

export type AgentActionRunOutput = { commandId: string; panel?: ModulePanelRef | null; reply?: Record<string, unknown> | null; state: string }

export type AgentCreateInput = { connectionName?: string | null; displayName: string; handle: string; ownerAccountId?: string | null; roleId?: string | null; scopes?: string[] | null }

export type AgentCreateOutput = { accountId: string; connectionId: string; credential?: string | null; credentialExpiresAt?: string | null; spaceId: string }

export type AgentManagersSetInput = { add?: string[]; agent: string; remove?: string[] }

export type AgentManagersSetOutput = { agent: string; managers: string[] }

export type AgentOfferListInput = Record<string, never>

export type AgentOfferListOutput = { offers: Offer[] }

export type AgentOfferStartInput = { host: string; name?: string | null; offerId: string; roleId?: string | null }

export type AgentOfferStartOutput = { accountId?: string | null; commandId?: string | null; page?: string | null; panel?: ModulePanelRef | null }

export type AgentPauseInput = { account: string; everywhere?: boolean; reason?: string | null }

export type AgentPauseOutput = { account: string; changed: boolean; status: string }

export type AgentProfileListInput = { projectId?: string | null }

export type AgentProfileListOutput = { agents: Array<Record<string, unknown>>; harnesses: Array<Record<string, unknown>>; profiles: Array<Record<string, unknown>>; templates: Array<Record<string, unknown>> }

export type AgentProfileSetInput = { allowedTools?: string[] | null; avatar?: string | null; color?: string | null; description?: string | null; enabled?: boolean | null; environmentId?: string | null; expectedVersion?: number | null; handle?: string | null; harness?: string | null; instructions?: string | null; name?: string | null; profile?: string | null; projectId: string; remove?: boolean | null; runDefaults?: Record<string, unknown> | null; template?: string | null }

export type AgentProfileSetOutput = { done: string; profile?: Record<string, unknown> | null }

export type AgentRecipeListInput = Record<string, never>

export type AgentRecipeListOutput = { recipes: RecipeView[] }

export type AgentRefDecideInput = { accept: boolean; agent: string }

export type AgentRefDecideOutput = { active?: OutsideRef | null; agent: string; pending?: OutsideRef | null }

export type AgentRefRemoveInput = { agent: string }

export type AgentRefRemoveOutput = { active?: OutsideRef | null; agent: string; pending?: OutsideRef | null }

export type AgentRefSetInput = { agent: string; externalId: string; issuer: string }

export type AgentRefSetOutput = { active?: OutsideRef | null; agent: string; pending?: OutsideRef | null }

export type AgentRefUpdateInput = { agent: string; externalId: string; issuer: string }

export type AgentRefUpdateOutput = { active?: OutsideRef | null; agent: string; pending?: OutsideRef | null }

export type AgentRegisterDecideInput = { decision: string; proposal: string; roleId?: string | null }

export type AgentRegisterDecideOutput = { accountId?: string | null; proposalId: string; status: string }

export type AgentResumeInput = { account: string; everywhere?: boolean; reason?: string | null }

export type AgentResumeOutput = { account: string; changed: boolean; status: string }

export type AgentScopeGetInput = { account: string }

export type AgentScopeGetOutput = { accountId: string; alsoInOtherWorkspaces: boolean; connections: number; displayName?: string | null; follows: number; handle?: string | null; managers: ScopeAccount[]; privateConversations: ScopeConversation[]; readsWithConsent: boolean; spaces: ScopeSpace[]; workspaceRole: string }

export type AgentTemplateListInput = { spaceId?: string | null }

export type AgentTemplateListOutput = { templates: AgentTemplateView[] }

export type AgentUsageListInput = { days?: number | null }

export type AgentUsageListOutput = { accounts: AgentUsageLine[]; days: number; notTiedToARun: UnattributedUsageLine[] }

export type AppConsentSetInput = { app: string; conversation: string; granted?: boolean | null }

export type AppConsentSetOutput = { appAccountId: string; changed: boolean; conversationId: string; granted: boolean }

export type AppLimitSetInput = { account?: string | null; lift?: boolean; permissions?: string[] | null }

export type AppLimitSetOutput = { accountId: string; permissions?: string[] | null }

export type AutomationCreateInput = { actions: Action[]; conditions?: Conditions | null; enabled?: boolean | null; name: string; projectId: string; trigger: Trigger }

export type AutomationCreateOutput = unknown

export type AutomationDeleteInput = { projectId: string; ruleId: string }

export type AutomationDeleteOutput = unknown

export type AutomationHistoryInput = { projectId: string; ruleId: string }

export type AutomationHistoryOutput = unknown

export type AutomationListInput = { projectId: string }

export type AutomationListOutput = unknown

export type AutomationRunStartInput = { projectId: string; ruleId: string; taskId?: string | null }

export type AutomationRunStartOutput = unknown

export type AutomationUpdateInput = { actions?: Action[] | null; conditions?: Conditions | null; enabled?: boolean | null; expectedVersion: number; name?: string | null; projectId: string; ruleId: string; trigger?: Trigger | null }

export type AutomationUpdateOutput = unknown

export type BriefingGetInput = { projectId?: string | null }

export type BriefingGetOutput = { claims?: TaskLine[]; mentions: Mention[]; nextSteps: string; notifications: NotificationLine[]; replyRequests?: InboxItem[]; tasks: TaskLine[]; threadUpdates: ThreadUpdate[]; unread: ConversationSummary[]; you: You }

export type BudgetListInput = { account?: string | null }

export type BudgetListOutput = { budgets: BudgetView[]; meters: BudgetMeter[] }

export type BudgetSetInput = { eachAccount?: boolean; expectedVersion?: number | null; hardLimit?: number | null; meter: string; period: BudgetPeriod; scope: BudgetScope; scopeId?: string | null; softLimit?: number | null }

export type BudgetSetOutput = { budget?: BudgetView | null }

export type BugReportLinkCreateInput = { collection?: string | null; delete?: boolean | null; form?: FormWrite | null; linkId?: string | null; name?: string | null; projectId: string; replace?: boolean | null; state?: string | null }

export type BugReportLinkCreateOutput = unknown

export type BugReportLinkListInput = { projectId: string }

export type BugReportLinkListOutput = unknown

export type BugReportLinkUpdateInput = { collection?: string | null; delete?: boolean | null; form?: FormWrite | null; linkId?: string | null; name?: string | null; projectId: string; replace?: boolean | null; state?: string | null }

export type BugReportLinkUpdateOutput = unknown

export type ButtonGetInput = { collection: string; field: string }

export type ButtonGetOutput = { button: ButtonView; canPress: boolean; reason?: string | null }

export type ButtonPressInput = { collection: string; field: string; record: string }

export type ButtonPressOutput = { record: PreviewRecord; steps: PressedStep[] }

export type ChangeListInput = { limit?: number | null; sinceHours?: number | null; thingId: string; thingKind: string }

export type ChangeListOutput = { changes: ChangeSetView[] }

export type ChangeUndoInput = { changeSetId: string }

export type ChangeUndoOutput = { change: ChangeSetView; skipped: string[]; version: number }

export type ChannelCreateInput = { autoJoin?: boolean | null; name: string; private?: boolean | null; projectId: string }

export type ChannelCreateOutput = { archived: boolean; created?: boolean | null; id: string; kind: string; name: string; outcome?: string | null; private?: boolean; status?: string | null; url: string; version: number }

export type ChannelGetInput = { channelId: string }

export type ChannelGetOutput = { archived: boolean; autoJoin: boolean; canEdit: boolean; canManage: boolean; description?: string | null; id: string; joined: boolean; memberCount: number; name: string; private: boolean; spaceId: string; topic?: string | null; version: number }

export type ChannelJoinInput = { channelId: string }

export type ChannelJoinOutput = { archived: boolean; autoJoin: boolean; canEdit: boolean; canManage: boolean; description?: string | null; id: string; joined: boolean; memberCount: number; name: string; private: boolean; spaceId: string; topic?: string | null; version: number }

export type ChannelLeaveInput = { channelId: string }

export type ChannelLeaveOutput = { channelId: string; stillVisible: boolean }

export type ChannelListInput = { query?: string | null; spaceId: string }

export type ChannelListOutput = { channels: BrowsedChannel[] }

export type ChannelUpdateInput = { autoJoin?: boolean | null; channelId: string; description?: string | null; expectedVersion?: number | null; topic?: string | null }

export type ChannelUpdateOutput = { archived: boolean; autoJoin: boolean; canEdit: boolean; canManage: boolean; description?: string | null; id: string; joined: boolean; memberCount: number; name: string; private: boolean; spaceId: string; topic?: string | null; version: number }

export type CommentCreateInput = { anchor?: unknown; attachmentIds?: string[]; content?: unknown; newThread?: boolean; replyToMessageId?: string | null; text?: string | null; thing: ThingInput; threadId?: string | null }

export type CommentCreateOutput = { conversationId: string; href?: string | null; messageId: string; threadId?: string | null }

export type CommentListInput = { cursor?: string | null; limit?: number | null; status?: string | null; thing: ThingInput }

export type CommentListOutput = { acceptsComments: boolean; conversationId?: string | null; messageCount: number; nextCursor?: string | null; spaceId?: string | null; threads: CommentThreadView[]; unread: boolean }

export type CommentThreadUpdateInput = { outcome?: string | null; status: string; threadId: string }

export type CommentThreadUpdateOutput = { outcome?: string | null; status: string; threadId: string }

export type ConnectedModuleApproveInput = { digest?: string | null; moduleId: string; scopes?: string[] | null }

export type ConnectedModuleApproveOutput = { accountId: string; accountName: string; approvedAt: string; approvedBy: string; baseUrl: string; description: string; digest: string; events: string[]; fileCheck?: Record<string, unknown> | null; frameOrigins: string[]; installSentAt?: string | null; key: string; kinds: string[]; manifestUrl: string; moduleId: string; name: string; operations: OperationSummary[]; panels: PanelSummary[]; pending?: ManifestSummary | null; publisher?: string | null; revokedAt?: string | null; scopes: ScopeView[]; state: string; version: string }

export type ConnectedModuleInstallInput = { digest?: string | null; scopes?: string[] | null; url: string }

export type ConnectedModuleInstallOutput = { accountId: string; accountName: string; approvedAt: string; approvedBy: string; baseUrl: string; description: string; digest: string; events: string[]; fileCheck?: Record<string, unknown> | null; frameOrigins: string[]; installSentAt?: string | null; key: string; kinds: string[]; manifestUrl: string; moduleId: string; name: string; operations: OperationSummary[]; panels: PanelSummary[]; pending?: ManifestSummary | null; publisher?: string | null; revokedAt?: string | null; scopes: ScopeView[]; state: string; version: string }

export type ConnectedModuleListInput = { includeRevoked?: boolean; moduleId?: string | null }

export type ConnectedModuleListOutput = { modules: ConnectedModuleView[] }

export type ConnectedModuleRefreshInput = { moduleId: string }

export type ConnectedModuleRefreshOutput = { module: ConnectedModuleView; refreshed: Refreshed }

export type ConnectedModuleRevokeInput = { moduleId: string }

export type ConnectedModuleRevokeOutput = { accountId: string; accountName: string; approvedAt: string; approvedBy: string; baseUrl: string; description: string; digest: string; events: string[]; fileCheck?: Record<string, unknown> | null; frameOrigins: string[]; installSentAt?: string | null; key: string; kinds: string[]; manifestUrl: string; moduleId: string; name: string; operations: OperationSummary[]; panels: PanelSummary[]; pending?: ManifestSummary | null; publisher?: string | null; revokedAt?: string | null; scopes: ScopeView[]; state: string; version: string }

export type ConnectedModuleRunInput = { input?: Record<string, unknown>; operation: string }

export type ConnectedModuleRunOutput = { moduleId: string; operation: string; result: unknown }

export type ConnectedModuleTestInput = { moduleId: string }

export type ConnectedModuleTestOutput = { moduleId: string; problems: Problem[] }

export type ConnectionCreateInput = { account?: string | null; name?: string | null; reachesAll?: boolean | null; subscriptions?: SubscriptionInput[] | null; webhookFormat?: string | null; webhookUrl?: string | null }

export type ConnectionCreateOutput = { connection: ConnectionView; webhookSecret?: string | null }

export type ConnectionDeliveryListInput = { before?: number | null; connectionId: string; limit?: number | null }

export type ConnectionDeliveryListOutput = { deliveries: DeliveryView[]; nextBefore?: number | null }

export type ConnectionListInput = { account?: string | null }

export type ConnectionListOutput = { connections: ConnectionView[] }

export type ConnectionRedeliverInput = { connectionId: string; eventPosition: number }

export type ConnectionRedeliverOutput = { attempts: number; createdAt: string; deliveredAt?: string | null; elapsedMs?: number | null; eventId: string; eventPosition: number; eventType: string; lastError?: string | null; lastStatus?: number | null; nextAttemptAt?: string | null; reason: string; state: string }

export type ConnectionRemoveInput = { connectionId: string }

export type ConnectionRemoveOutput = { accountId: string; consecutiveFailures: number; disabledReason?: string | null; forCredential: boolean; homeWorkspaceId: string; id: string; isDefault: boolean; lastDeliveredAt?: string | null; lastError?: string | null; lastSeenAt?: string | null; lastTest?: LastTest | null; madeThroughApp: boolean; name: string; reachesAll: boolean; state: string; subscriptions: Subscription[]; version: number; webhookFormat: string; webhookUrl?: string | null }

export type ConnectionSecretRotateInput = { connectionId: string }

export type ConnectionSecretRotateOutput = { connection: ConnectionView; webhookSecret?: string | null }

export type ConnectionTestInput = { connectionId: string }

export type ConnectionTestOutput = { queued: boolean }

export type ConnectionUpdateInput = { connectionId: string; expectedVersion?: number | null; name?: string | null; reachesAll?: boolean | null; subscriptions?: SubscriptionInput[] | null; webhookFormat?: string | null; webhookUrl?: string | null }

export type ConnectionUpdateOutput = { connection: ConnectionView; webhookSecret?: string | null }

export type ConversationAgentSettingsListInput = { conversation: string }

export type ConversationAgentSettingsListOutput = { accounts: AgentSettingsView[] }

export type ConversationAgentSettingsSetInput = { account: string; ambient?: boolean | null; conversation: string; replyCooldownSeconds?: number | null }

export type ConversationAgentSettingsSetOutput = { accountId: string; ambient: boolean; displayName: string; handle: string; replyCooldownSeconds: number }

export type ConversationContinueInput = { conversationId: string }

export type ConversationContinueOutput = { agentTurnsRemaining: number; asked?: MemberRef[]; conversationId: string; idempotentReplay?: boolean; latestMessageId?: string | null }

export type ConversationFileListInput = { conversationId: string }

export type ConversationFileListOutput = { files: ConversationFile[] }

export type ConversationMarkReadInput = { conversationId: string; fromMessageId?: string | null; unread?: boolean | null; upToSequence?: number | null }

export type ConversationMarkReadOutput = { conversationId: string; lastReadSequence: number; unreadCount: number }

export type ConversationMemberListInput = { conversationId: string }

export type ConversationMemberListOutput = { canManage: boolean; conversationId: string; members: ConversationMember[] }

export type ConversationMembersUpdateInput = { add?: string[] | null; conversationId: string; expectedVersion?: number | null; remove?: string[] | null }

export type ConversationMembersUpdateOutput = { added?: MemberRef[]; conversationId: string; idempotentReplay?: boolean; left?: boolean; members: MemberRef[]; name: string; removed?: MemberRef[]; url: string; version: number }

export type ConversationTabListInput = { conversationId: string }

export type ConversationTabListOutput = { tabs: ConversationTab[] }

export type ConversationTabSetInput = { conversationId: string; page: string; position?: number | null; remove?: boolean }

export type ConversationTabSetOutput = { tabs: ConversationTab[] }

export type ConversationTaskListInput = { conversationId: string }

export type ConversationTaskListOutput = { tasks: ConversationTask[] }

export type ConversationUpdateInput = { archived?: boolean | null; clearOutcome?: boolean | null; conversationId: string; expectedVersion?: number | null; name?: string | null; outcome?: string | null; status?: string | null }

export type ConversationUpdateOutput = { archived: boolean; created?: boolean | null; id: string; kind: string; name: string; outcome?: string | null; private?: boolean; status?: string | null; url: string; version: number }

export type ConversationsMarkReadInput = { spaceId?: string | null }

export type ConversationsMarkReadOutput = { marked: number; more: boolean }

export type CredentialCreateInput = { account?: string | null; days?: number | null; name: string; reach?: CredentialReach | null; space?: string | null; write?: boolean | null }

export type CredentialCreateOutput = { credential: CredentialView; oldSecretValidUntil?: string | null; secret?: string | null }

export type CredentialListInput = { account?: string | null }

export type CredentialListOutput = { credentials: CredentialView[] }

export type CredentialRevokeInput = { credential: string }

export type CredentialRevokeOutput = { accountId: string; clientName: string; createdAt: string; expiresAt: string; id: string; lastUsedAt?: string | null; reach: string; replacedBy?: string | null; scopes: string[]; spaceId: string; status: string; tokenPrefix: string }

export type CredentialRotateInput = { credential: string; days?: number | null; graceMinutes?: number | null }

export type CredentialRotateOutput = { credential: CredentialView; oldSecretValidUntil?: string | null; secret?: string | null }

export type CustomEmojiCreateInput = { imageBase64: string; name: string }

export type CustomEmojiCreateOutput = { createdAt: string; createdByAccountId: string; id: string; imageUrl: string; name: string; retired: boolean }

export type CustomEmojiListInput = { includeRetired?: boolean }

export type CustomEmojiListOutput = { emoji: CustomEmojiView[] }

export type CustomEmojiUpdateInput = { id: string; retired: boolean }

export type CustomEmojiUpdateOutput = { createdAt: string; createdByAccountId: string; id: string; imageUrl: string; name: string; retired: boolean }

export type CycleListInput = { spaceId: string }

export type CycleListOutput = { cycles: Cycle[]; settings: Settings }

export type CycleSetInput = { cycle?: string | null; enabled?: boolean | null; expectedVersion?: number | null; lengthDays?: number | null; name?: string | null; rollOver?: boolean | null; spaceId: string; startsOnWeekday?: number | null; upcoming?: number | null }

export type CycleSetOutput = { cycles: Cycle[]; settings: Settings }

export type DashboardCreateInput = { description?: string | null; name: string; shared?: boolean | null; spaceId: string; tiles?: TileInput[] }

export type DashboardCreateOutput = { description?: string; id: string; name: string; position: number; shared: boolean; tiles: TileEntry[]; updatedAt: string; version: number }

export type DashboardDeleteInput = { dashboard: string }

export type DashboardDeleteOutput = { id: string; removed: boolean }

export type DashboardGetInput = { dashboard: string; figures?: boolean | null }

export type DashboardGetOutput = { description?: string; id: string; name: string; position: number; shared: boolean; tiles: TileEntry[]; updatedAt: string; version: number }

export type DashboardListInput = { spaceId: string }

export type DashboardListOutput = { canShare: boolean; dashboards: DashboardEntry[] }

export type DashboardUpdateInput = { dashboard: string; description?: string | null; expectedVersion?: number | null; name?: string | null; share?: boolean | null; tiles?: TileInput[] | null }

export type DashboardUpdateOutput = { description?: string; id: string; name: string; position: number; shared: boolean; tiles: TileEntry[]; updatedAt: string; version: number }

export type DataConnectionCreateInput = { address: string; name: string; spaceId: string }

export type DataConnectionCreateOutput = { address: string; id: string; kind: string; name: string; queries: number; updatedAt: string; version: number }

export type DataConnectionDeleteInput = { connection: string }

export type DataConnectionDeleteOutput = { id: string; removed: boolean }

export type DataConnectionListInput = { spaceId: string }

export type DataConnectionListOutput = { connections: ConnectionEntry[] }

export type DataConnectionUpdateInput = { address?: string | null; connection: string; expectedVersion?: number | null; name?: string | null }

export type DataConnectionUpdateOutput = { address: string; id: string; kind: string; name: string; queries: number; updatedAt: string; version: number }

export type DataQueryCreateInput = { connection: string; description?: string | null; name: string; parameters?: Parameter[]; rowCap?: number | null; spaceId: string; statement: string }

export type DataQueryCreateOutput = { answer?: AnswerEntry | null; canRun: boolean; connection?: string | null; description?: string; id: string; name: string; parameters: Parameter[]; review: ReviewState; reviewedAt?: string | null; reviewedBy?: string | null; rowCap?: number | null; shared: boolean; statement?: string | null; updatedAt: string; version: number }

export type DataQueryDeleteInput = { query: string }

export type DataQueryDeleteOutput = { id: string; removed: boolean }

export type DataQueryGetInput = { query: string }

export type DataQueryGetOutput = { answer?: AnswerEntry | null; canRun: boolean; connection?: string | null; description?: string; id: string; name: string; parameters: Parameter[]; review: ReviewState; reviewedAt?: string | null; reviewedBy?: string | null; rowCap?: number | null; shared: boolean; statement?: string | null; updatedAt: string; version: number }

export type DataQueryListInput = { spaceId: string }

export type DataQueryListOutput = { canConnect: boolean; canShare: boolean; queries: QueryEntry[] }

export type DataQueryReviewInput = { expectedVersion?: number | null; query: string; state: ReviewState }

export type DataQueryReviewOutput = { answer?: AnswerEntry | null; canRun: boolean; connection?: string | null; description?: string; id: string; name: string; parameters: Parameter[]; review: ReviewState; reviewedAt?: string | null; reviewedBy?: string | null; rowCap?: number | null; shared: boolean; statement?: string | null; updatedAt: string; version: number }

export type DataQueryRunInput = { parameters?: Record<string, unknown> | null; query: string }

export type DataQueryRunOutput = { answer?: AnswerEntry | null; canRun: boolean; connection?: string | null; description?: string; id: string; name: string; parameters: Parameter[]; review: ReviewState; reviewedAt?: string | null; reviewedBy?: string | null; rowCap?: number | null; shared: boolean; statement?: string | null; updatedAt: string; version: number }

export type DataQueryShareInput = { query: string; shared: boolean }

export type DataQueryShareOutput = { answer?: AnswerEntry | null; canRun: boolean; connection?: string | null; description?: string; id: string; name: string; parameters: Parameter[]; review: ReviewState; reviewedAt?: string | null; reviewedBy?: string | null; rowCap?: number | null; shared: boolean; statement?: string | null; updatedAt: string; version: number }

export type DataQueryUpdateInput = { connection?: string | null; description?: string | null; expectedVersion?: number | null; name?: string | null; parameters?: Parameter[] | null; query: string; rowCap?: number | null; statement?: string | null }

export type DataQueryUpdateOutput = { answer?: AnswerEntry | null; canRun: boolean; connection?: string | null; description?: string; id: string; name: string; parameters: Parameter[]; review: ReviewState; reviewedAt?: string | null; reviewedBy?: string | null; rowCap?: number | null; shared: boolean; statement?: string | null; updatedAt: string; version: number }

export type DecisionListInput = { includeSuperseded?: boolean; limit?: number | null; spaceId: string }

export type DecisionListOutput = { decisions: DecisionView[] }

export type DecisionRecordInput = { detail?: string | null; pin?: boolean; saySo?: boolean; sourceConversationId?: string | null; sourceMessageId?: string | null; spaceId?: string | null; supersedes?: string | null; title: string }

export type DecisionRecordOutput = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

export type DependencyGraphGetInput = { includeFinished?: boolean | null; label?: string | null; milestoneId?: string | null; parentId?: string | null; projectId: string }

export type DependencyGraphGetOutput = unknown

export type DirectoryListInput = { query?: string | null; shapes?: boolean | null }

export type DirectoryListOutput = { entries: EntryView[]; ownSourcesAllowed: boolean; shape?: string | null; sources: SourceView[] }

export type DirectorySourceCreateInput = { name?: string | null; url: string }

export type DirectorySourceCreateOutput = { error?: string | null; fetchedAt?: string | null; id: string; name: string; url: string }

export type DirectorySourceRemoveInput = { sourceId: string }

export type DirectorySourceRemoveOutput = { id: string }

export type DmRequestRespondInput = { accept: boolean; conversationId: string }

export type DmRequestRespondOutput = { canSend: boolean; otherAccountId?: string | null; reason?: string | null; request?: DmRequestView | null; youBlocked: boolean }

export type DmStandingGetInput = { conversationId: string }

export type DmStandingGetOutput = { canSend: boolean; otherAccountId?: string | null; reason?: string | null; request?: DmRequestView | null; youBlocked: boolean }

export type EnvironmentArchiveInput = { environment: string; projectId: string }

export type EnvironmentArchiveOutput = { checkMessage?: string | null; checkOk?: boolean | null; done: string; environment?: EnvironmentValue | null }

export type EnvironmentCheckInput = { environment: string; projectId: string }

export type EnvironmentCheckOutput = { checkMessage?: string | null; checkOk?: boolean | null; done: string; environment?: EnvironmentValue | null }

export type EnvironmentCreateInput = { branch?: string | null; harness: string; makeDefault?: boolean | null; name: string; projectId: string; providerEnvironmentId?: string | null; repositoryId?: string | null; setupNotes?: string | null }

export type EnvironmentCreateOutput = { checkMessage?: string | null; checkOk?: boolean | null; done: string; environment?: EnvironmentValue | null }

export type EnvironmentListInput = { projectId: string }

export type EnvironmentListOutput = { defaultEnvironmentId?: string | null; environments: EnvironmentValue[] }

export type EnvironmentUpdateInput = { branch?: string | null; environment: string; expectedVersion?: number | null; harness?: string | null; makeDefault?: boolean | null; name?: string | null; projectId: string; providerEnvironmentId?: string | null; repositoryId?: string | null; setupNotes?: string | null }

export type EnvironmentUpdateOutput = { checkMessage?: string | null; checkOk?: boolean | null; done: string; environment?: EnvironmentValue | null }

export type EventKindListInput = Record<string, never>

export type EventKindListOutput = { events: Entry[] }

export type EventListInput = { connectionId?: string | null; cursor?: string | null; limit?: number | null; waitSeconds?: number | null }

export type EventListOutput = { connectionId: string; events: Array<Record<string, unknown>>; hasMore: boolean; nextCursor: string }

export type FieldCreateInput = { archived?: boolean | null; description?: string | null; expectedVersion?: number | null; field?: string | null; name?: string | null; options?: OptionInput[] | null; position?: number | null; projectId: string; target?: string | null; type?: string | null }

export type FieldCreateOutput = { changedFields: string[]; created: boolean; field: FieldView; note?: string | null }

export type FieldListInput = { includeArchived?: boolean | null; projectId: string }

export type FieldListOutput = { canManage: boolean; fields: FieldView[] }

export type FieldUpdateInput = { archived?: boolean | null; description?: string | null; expectedVersion?: number | null; field?: string | null; name?: string | null; options?: OptionInput[] | null; position?: number | null; projectId: string; target?: string | null; type?: string | null }

export type FieldUpdateOutput = { changedFields: string[]; created: boolean; field: FieldView; note?: string | null }

export type FileLinkGetInput = { fileId: string }

export type FileLinkGetOutput = { contentType: string; expiresAt: string; name: string; size: number; url: string }

export type FileSourceGetInput = { moduleId: string; ref: string }

export type FileSourceGetOutput = { kind: FileKind; mime?: string | null; modifiedAt: string; name: string; ref: string; size?: number | null; webUrl?: string | null }

export type FileSourceListInput = { cursor?: string | null; folder?: string | null; moduleId: string }

export type FileSourceListOutput = { items: FileItem[]; nextCursor?: string | null }

export type FileUploadFinishInput = { uploadId: string }

export type FileUploadFinishOutput = { commentId?: string | null; contentType: string; createdAt: string; height?: number | null; id: string; name: string; preview?: string | null; size: number; uploadedBy: string; width?: number | null }

export type FileUploadStartInput = { contentType?: string | null; conversationId?: string | null; forComment?: boolean; name: string; sha256: string; size: number; taskId?: string | null }

export type FileUploadStartOutput = { expiresAt: string; headers: Record<string, string>; method: string; uploadId: string; url: string }

export type FollowListInput = { space?: string | null }

export type FollowListOutput = { follows: FollowView[] }

export type FollowSetInput = { id: string; kind: FollowKind; level?: FollowLevel | null; on?: boolean | null }

export type FollowSetOutput = { changed: boolean; id: string; kind: FollowKind; level?: FollowLevel | null }

export type FormAnswerSendInput = { answers: Record<string, unknown>; form: string }

export type FormAnswerSendOutput = { collectionId: string; number: number; recordId: string; thanks?: string; title: string }

export type FormCreateInput = { collection: string; definition?: unknown; name: string }

export type FormCreateOutput = { archived?: boolean; audience: Audience; canAnswer: boolean; canBuild: boolean; collectionId: string; collectionName: string; definition: FormConfig; id: string; linkId?: string | null; name: string; questions: PublicField[]; titleName: string; version: number }

export type FormGetInput = { form: string }

export type FormGetOutput = { archived?: boolean; audience: Audience; canAnswer: boolean; canBuild: boolean; collectionId: string; collectionName: string; definition: FormConfig; id: string; linkId?: string | null; name: string; questions: PublicField[]; titleName: string; version: number }

export type FormListInput = { collection?: string | null; includeArchived?: boolean; spaceId: string }

export type FormListOutput = { forms: FormSummary[] }

export type FormUpdateInput = { archived?: boolean | null; definition?: unknown; expectedVersion?: number | null; form: string; name?: string | null }

export type FormUpdateOutput = { archived?: boolean; audience: Audience; canAnswer: boolean; canBuild: boolean; collectionId: string; collectionName: string; definition: FormConfig; id: string; linkId?: string | null; name: string; questions: PublicField[]; titleName: string; version: number }

export type FormulaPreviewInput = { collection: string; field?: string | null; formula: string; record?: number | null }

export type FormulaPreviewOutput = { problem?: string | null; record?: PreviewRecord | null; result?: ResultType | null; value?: unknown }

export type GithubAutomationUpdateInput = { expectedVersion?: number | null; projectId: string; pullRequestMerge?: boolean | null; pullRequestOpen?: boolean | null }

export type GithubAutomationUpdateOutput = { automation?: Record<string, unknown> | null; done: string; installation?: Record<string, unknown> | null }

export type GithubInstallationLinkInput = { installationId: number; projectId: string; repositories: string[] }

export type GithubInstallationLinkOutput = { automation?: Record<string, unknown> | null; done: string; installation?: Record<string, unknown> | null }

export type GroupArchiveInput = { group: string }

export type GroupArchiveOutput = { archived: boolean; description: string; handle: string; id: string; memberIds: string[]; name: string; version: number }

export type GroupCreateInput = { description?: string | null; handle: string; members?: string[]; name: string }

export type GroupCreateOutput = { archived: boolean; description: string; handle: string; id: string; memberIds: string[]; name: string; version: number }

export type GroupListInput = { members?: boolean; prefix?: string | null }

export type GroupListOutput = { groups: GroupView[] }

export type GroupMembersSetInput = { add?: string[]; group: string; remove?: string[] }

export type GroupMembersSetOutput = { archived: boolean; description: string; handle: string; id: string; memberIds: string[]; name: string; version: number }

export type GroupRestoreInput = { group: string }

export type GroupRestoreOutput = { archived: boolean; description: string; handle: string; id: string; memberIds: string[]; name: string; version: number }

export type GroupUpdateInput = { description?: string | null; expectedVersion?: number | null; group: string; handle?: string | null; name?: string | null }

export type GroupUpdateOutput = { archived: boolean; description: string; handle: string; id: string; memberIds: string[]; name: string; version: number }

export type GuardListInput = Record<string, never>

export type GuardListOutput = { guards: GuardView[] }

export type GuardSetInput = { accounts?: string[] | null; action?: GuardAction | null; enabled?: boolean | null; expectedVersion?: number | null; guardId?: string | null; name?: string | null; operations?: string[] | null; remove?: boolean; reviewers?: string[] | null; space?: string | null; targetKinds?: string[] | null; terms?: string[] | null }

export type GuardSetOutput = { guard?: GuardView | null }

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

export type HostLinkInput = { accountId?: string | null; cardUrl: string; owner: HostOwnerChoice; token?: string | null }

export type HostLinkOutput = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type HostListInput = { mineOnly?: boolean }

export type HostListOutput = { hosts: HostView[] }

export type HostUnlinkInput = { hostId: string }

export type HostUnlinkOutput = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type InboxViewListInput = Record<string, never>

export type InboxViewListOutput = { views: InboxViewItem[] }

export type InboxViewSetInput = { delete?: boolean | null; density?: Density | null; filter?: InboxFilter | null; inboxViewId?: string | null; name?: string | null; position?: number | null }

export type InboxViewSetOutput = { views: InboxViewItem[] }

export type IncomingHookCreateInput = { name: string; spaceId: string }

export type IncomingHookCreateOutput = { hook: HookView; secret?: string | null }

export type IncomingHookDeleteInput = { hookId: string; spaceId: string }

export type IncomingHookDeleteOutput = { deleted: boolean }

export type IncomingHookListInput = { spaceId: string }

export type IncomingHookListOutput = { hooks: HookView[] }

export type InitiativeCreateInput = { fields?: Record<string, unknown>; title: string }

export type InitiativeCreateOutput = { collectionId: string; id: string; number: number; title: string }

export type InitiativeListInput = { includeArchived?: boolean }

export type InitiativeListOutput = { collectionId?: string | null; initiatives: Initiative[]; spaceId?: string | null }

export type InvitationCreateInput = { expectedVersion?: number | null; expiresInHours?: number | null; invitation?: string | null; note?: string | null; projectId: string; replace?: boolean | null; revoke?: boolean | null; role?: string | null; wholeWorkspace?: boolean | null }

export type InvitationCreateOutput = { done: string; invitation: InvitationLine; joinUrl?: string | null }

export type InvitationListInput = { projectId: string; wholeWorkspace?: boolean | null }

export type InvitationListOutput = { invitations: InvitationLine[] }

export type InvitationUpdateInput = { expectedVersion?: number | null; expiresInHours?: number | null; invitation?: string | null; note?: string | null; projectId: string; replace?: boolean | null; revoke?: boolean | null; role?: string | null; wholeWorkspace?: boolean | null }

export type InvitationUpdateOutput = { done: string; invitation: InvitationLine; joinUrl?: string | null }

export type InviteLinkCreateInput = { expiresInDays?: number | null; maxUses?: number | null; roleId?: string | null; space?: string | null }

export type InviteLinkCreateOutput = { link: InviteLinkView; token?: string | null; url?: string | null }

export type InviteLinkListInput = { includeInactive?: boolean }

export type InviteLinkListOutput = { links: InviteLinkView[] }

export type InviteLinkRevokeInput = { link: string }

export type InviteLinkRevokeOutput = { active: boolean; createdAt: string; createdBy: string; expiresAt: string; id: string; maxUses?: number | null; revokedAt?: string | null; roleId: string; roleName: string; spaceId?: string | null; uses: number }

export type JobGetInput = { jobId: string }

export type JobGetOutput = { createdAt: string; jobId: string; op: string; pollIntervalMs?: number | null; result?: unknown; status: string; updatedAt: string }

export type LabelCreateInput = { archived?: boolean | null; color?: string | null; description?: string | null; expectedVersion?: number | null; label?: string | null; name?: string | null; projectId: string }

export type LabelCreateOutput = { changedFields: string[]; created: boolean; label: LabelLine; note?: string | null }

export type LabelListInput = { includeArchived?: boolean | null; projectId: string }

export type LabelListOutput = { labels: LabelLine[] }

export type LabelUpdateInput = { archived?: boolean | null; color?: string | null; description?: string | null; expectedVersion?: number | null; label?: string | null; name?: string | null; projectId: string }

export type LabelUpdateOutput = { changedFields: string[]; created: boolean; label: LabelLine; note?: string | null }

export type LaterListInput = { view?: string | null }

export type LaterListOutput = { items?: LaterLine[]; openCount: number; scheduled?: ScheduledLine[] }

export type LaterSaveInput = { at?: string | null; messageId?: string | null; task?: string | null; text?: string | null }

export type LaterSaveOutput = { item: LaterLine }

export type LaterUpdateInput = { at?: string | null; done?: boolean | null; id: string; remove?: boolean | null; text?: string | null }

export type LaterUpdateOutput = { item?: LaterLine | null; removed: boolean }

export type LaunchCancelInput = { launchId: string }

export type LaunchCancelOutput = { createdAt: string; environment?: string | null; harness?: string | null; id: string; requestedBy: string; runId?: string | null; runState?: string | null; startAt?: string | null; startWhen: string; state: string; statusReason?: string | null; task?: string | null; taskTitle?: string | null; threadUrl?: string | null }

export type LaunchListInput = { projectId?: string | null }

export type LaunchListOutput = { queued: LaunchValue[]; recent: LaunchValue[] }

export type LayoutPresetCreateInput = { base?: string | null; description?: string | null; expectedVersion?: number | null; name?: string | null; preset?: string | null; projectId: string; remove?: boolean | null; show?: Record<string, boolean> | null; words?: Record<string, PresetWord> | null }

export type LayoutPresetCreateOutput = { done: string; id: string; name: string; preset?: LayoutPresetLine | null }

export type LayoutPresetUpdateInput = { base?: string | null; description?: string | null; expectedVersion?: number | null; name?: string | null; preset?: string | null; projectId: string; remove?: boolean | null; show?: Record<string, boolean> | null; words?: Record<string, PresetWord> | null }

export type LayoutPresetUpdateOutput = { done: string; id: string; name: string; preset?: LayoutPresetLine | null }

export type LayoutUpdateInput = { expectedVersion?: number | null; preset?: string | null; projectId: string; rules?: LayoutRuleArgument[] | null; wholeWorkspace?: boolean | null }

export type LayoutUpdateOutput = { layout?: LayoutLine | null; projectId: string; rules?: number | null; version: number }

export type MemoryPageOpenInput = { conversation?: string | null; space?: string | null }

export type MemoryPageOpenOutput = { page?: MemoryPageView | null }

export type MemoryPageSetInput = { body: string; conversation?: string | null; expectedVersion?: number | null; note?: string | null; space?: string | null }

export type MemoryPageSetOutput = { body: string; pageId: string; spaceId: string; title: string; updatedAt: string; version: number }

export type MessageDeleteInput = { messageId: string }

export type MessageDeleteOutput = { alreadyRemoved: boolean; cancelledReplyRequests: number; conversationId: string; messageId: string; note: string; removedAt: string; removedBy: string; threads?: ThreadRef[] }

export type MessageDraftListInput = { conversationId?: string | null }

export type MessageDraftListOutput = { drafts: DraftView[] }

export type MessageDraftSetInput = { conversationId: string; draft?: unknown; savedAt?: string | null }

export type MessageDraftSetOutput = { draft?: DraftView | null; keptNewer: boolean }

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

export type MilestoneCreateInput = { addTasks?: string[] | null; archived?: boolean | null; clientRequestId?: string | null; description?: string | null; expectedVersion?: number | null; initiative?: string | null; milestoneId?: string | null; name?: string | null; projectId: string; removeTasks?: string[] | null; targetDate?: string | null }

export type MilestoneCreateOutput = { archived: boolean; canceled?: number; description: string; estimateDone?: number; estimateTotal?: number; finished: number; id: string; idempotentReplay?: boolean; initiative?: string | null; initiativeId?: string | null; latestDueDate?: string | null; name: string; open?: number; overdue: boolean; position?: number; progress?: number; targetDate?: string | null; total: number; url: string; version: number }

export type MilestoneListInput = { includeArchived?: boolean | null; projectId: string }

export type MilestoneListOutput = { milestones: MilestoneValue[] }

export type MilestoneReorderInput = { expectedVersion?: number | null; milestoneId: string; position: number }

export type MilestoneReorderOutput = { milestones: MilestonePlace[] }

export type MilestoneUpdateInput = { addTasks?: string[] | null; archived?: boolean | null; clientRequestId?: string | null; description?: string | null; expectedVersion?: number | null; initiative?: string | null; milestoneId?: string | null; name?: string | null; projectId: string; removeTasks?: string[] | null; targetDate?: string | null }

export type MilestoneUpdateOutput = { archived: boolean; canceled?: number; description: string; estimateDone?: number; estimateTotal?: number; finished: number; id: string; idempotentReplay?: boolean; initiative?: string | null; initiativeId?: string | null; latestDueDate?: string | null; name: string; open?: number; overdue: boolean; position?: number; progress?: number; targetDate?: string | null; total: number; url: string; version: number }

export type ModelCatalogSetInput = { harness: string; model?: string | null; overrides?: unknown; projectId: string; refresh?: boolean | null; remove?: boolean | null }

export type ModelCatalogSetOutput = { done: string; harness: unknown }

export type ModuleArchiveInput = { moduleId: string; parts: ArchiveParts }

export type ModuleArchiveOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModuleCreateInput = { automations?: InlineAutomation[]; collections?: InlineCollection[]; description?: string | null; forms?: InlineForm[]; icon?: string | null; key: string; name: string; nav?: NavEntry[]; parts?: PartRef[]; spaceId?: string | null }

export type ModuleCreateOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModuleFrameCheckInput = { launchId: string; ref: string }

export type ModuleFrameCheckOutput = { answer: PanelAnswer; moduleId: string; panel: string }

export type ModuleListInput = { module?: string | null; projectId?: string | null }

export type ModuleListOutput = { built: BuiltModuleSummary[]; detail?: ModuleDetail | null; modules: ModuleView[]; preset?: string | null; workspaceId: string }

export type ModuleOpenInput = { moduleId: string }

export type ModuleOpenOutput = { archived: boolean; archivedParts?: string | null; description: string; icon?: string | null; id: string; key: string; name: string; nav: NavView[]; on: boolean; parts: PartView[]; revision: number; spaceId?: string | null }

export type ModulePanelListInput = { slot?: string | null; spaceId?: string | null }

export type ModulePanelListOutput = { panels: PanelEntry[] }

export type ModulePanelOpenInput = { moduleId: string; panel: string; spaceId?: string | null; state?: string | null; thing?: PanelThingInput | null }

export type ModulePanelOpenOutput = { answer: PanelAnswer; moduleId: string; panel: string }

export type ModulePanelPressInput = { action: PanelActionInput; moduleId: string; panel: string; spaceId?: string | null; state?: string | null; thing?: PanelThingInput | null }

export type ModulePanelPressOutput = { answer: PanelAnswer; moduleId: string; panel: string }

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

export type MyWorkGetOutput = { assigned: unknown; claimed: unknown; review: unknown; updates: unknown }

export type NotificationKeywordListInput = Record<string, never>

export type NotificationKeywordListOutput = { keywords: string[]; limit: number }

export type NotificationKeywordSetInput = { keywords: string[] }

export type NotificationKeywordSetOutput = { keywords: string[]; limit: number }

export type NotificationListInput = { category?: string | null; cursor?: string | null; limit?: number | null; projectId?: string | null; view?: string | null }

export type NotificationListOutput = { nextCursor?: string | null; notifications: NotificationLine[]; unreadCount: number }

export type NotificationUpdateInput = { action: string; all?: boolean | null; ids?: string[] | null; projectId?: string | null; snoozeUntil?: string | null }

export type NotificationUpdateOutput = { unreadCount: number; updated: string[] }

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

export type PackageUpgradeInput = { choices?: Record<string, Choice>; entry?: string | null; fileId?: string | null; installId: string; package?: Record<string, unknown> | null; planHash: string }

export type PackageUpgradeOutput = { added: string[]; changes: string[]; detached: string[]; installId: string; kept: string[]; skipped: string[]; updated: string[]; upgradedFrom: string; version: string }

export type PackageUpgradePreviewInput = { entry?: string | null; fileId?: string | null; installId: string; package?: Record<string, unknown> | null }

export type PackageUpgradePreviewOutput = { fromVersion: string; items: PlanItem[]; leaves: string[]; major: boolean; planHash: string; roles: unknown; toVersion: string; turnsOff: string[]; turnsOn: string[] }

export type PageBlocksGetInput = { from?: string | null; includeMarkdown?: boolean | null; pageId: string; section?: string | null; to?: string | null; version?: number | null }

export type PageBlocksGetOutput = { blocks: BlockLine[]; markdown?: string | null; outline: OutlineLine[]; pageId: string; projectId: string; suggestion?: SuggestionMade | null; title: string; truncated?: boolean; url: string; version: number }

export type PageCommentCreateInput = { blockId?: string | null; pageId: string; projectId?: string | null; quote?: string | null; replyTo?: string | null; text: string }

export type PageCommentCreateOutput = { commentId: string; threadId: string; url: string }

export type PageCommentListInput = { includeResolved?: boolean | null; pageId: string }

export type PageCommentListOutput = { pageId: string; threads: ThreadLine[]; url: string }

export type PageCommentUpdateInput = { commentId: string; delete?: boolean | null; pageId: string; resolved?: boolean | null; text?: string | null }

export type PageCommentUpdateOutput = { commentId: string; threadId: string; url: string }

export type PageCreateInput = { archived?: boolean | null; body?: string | null; clientRequestId?: string | null; expectedVersion?: number | null; fromTemplateId?: string | null; isGuide?: boolean | null; isTemplate?: boolean | null; note?: string | null; pageId?: string | null; pinToConversationId?: string | null; projectId?: string | null; restoreVersion?: number | null; skillDescription?: string | null; threadId?: string | null; title?: string | null }

export type PageCreateOutput = { archived: boolean; body: string; history: RevisionLine[]; id: string; idempotentReplay?: boolean; isGuide: boolean; isTemplate?: boolean; openComments?: number; pendingSuggestions?: number; projectId: string; skillDescription?: string | null; stale: boolean; suggestion?: SuggestionMade | null; title: string; truncated?: boolean; updatedAt: string; url: string; version: number }

export type PageGetInput = { pageId: string; version?: number | null }

export type PageGetOutput = { archived: boolean; body: string; history: RevisionLine[]; id: string; idempotentReplay?: boolean; isGuide: boolean; isTemplate?: boolean; openComments?: number; pendingSuggestions?: number; projectId: string; skillDescription?: string | null; stale: boolean; suggestion?: SuggestionMade | null; title: string; truncated?: boolean; updatedAt: string; url: string; version: number }

export type PageListInput = { includeArchived?: boolean | null; projectId: string }

export type PageListOutput = { pages: PageLineValue[] }

export type PageMakeInput = { blocks?: string[]; messages?: string[]; move?: boolean; pageId?: string | null; spaceId?: string | null; threadId?: string | null; title?: string | null }

export type PageMakeOutput = { id: string; sourceVersion?: number | null; spaceId: string; title: string; url: string }

export type PageSuggestionDecideInput = { decision: string; pageId: string; suggestionId: string }

export type PageSuggestionDecideOutput = { status: string; suggestionId: string; url: string; version: number }

export type PageSuggestionListInput = { includeDecided?: boolean | null; pageId: string }

export type PageSuggestionListOutput = { pageId: string; suggestions: SuggestionLine[]; url: string }

export type PageTemplateListInput = { spaceId: string }

export type PageTemplateListOutput = { templates: PageTemplateView[] }

export type PageUpdateInput = { archived?: boolean | null; body?: string | null; clientRequestId?: string | null; expectedVersion?: number | null; fromTemplateId?: string | null; isGuide?: boolean | null; isTemplate?: boolean | null; note?: string | null; pageId?: string | null; pinToConversationId?: string | null; projectId?: string | null; restoreVersion?: number | null; skillDescription?: string | null; threadId?: string | null; title?: string | null }

export type PageUpdateOutput = { archived: boolean; body: string; history: RevisionLine[]; id: string; idempotentReplay?: boolean; isGuide: boolean; isTemplate?: boolean; openComments?: number; pendingSuggestions?: number; projectId: string; skillDescription?: string | null; stale: boolean; suggestion?: SuggestionMade | null; title: string; truncated?: boolean; updatedAt: string; url: string; version: number }

export type PageUpdateBlocksInput = { edits: PageEditInput[]; expectedVersion?: number | null; note?: string | null; pageId: string; suggest?: boolean | null; threadId?: string | null }

export type PageUpdateBlocksOutput = { blocks: BlockLine[]; markdown?: string | null; outline: OutlineLine[]; pageId: string; projectId: string; suggestion?: SuggestionMade | null; title: string; truncated?: boolean; url: string; version: number }

export type PeopleSearchInput = { limit?: number | null; projectId?: string | null; query?: string | null }

export type PeopleSearchOutput = { people: PersonFound[] }

export type PermissionsGetInput = { account?: string | null; spaceId?: string | null }

export type PermissionsGetOutput = { actorId: string; personalAgentLimit?: string[] | null; project?: ScopePermissions | null; workspace: ScopePermissions }

export type PersonCreateInput = { displayName: string; handle: string; roleId?: string | null }

export type PersonCreateOutput = { accountId: string; claimExpiresAt: string; claimToken?: string | null }

export type PinSetInput = { conversationId: string; decisionId?: string | null; messageId?: string | null; pageId?: string | null; pinned?: boolean | null }

export type PinSetOutput = { conversationId: string; id: string; kind: string; pinned: boolean; pins: PinItem[] }

export type PlaceSearchInput = { q: string }

export type PlaceSearchOutput = { hits: PlaceHit[] }

export type PlanImportInput = { commit?: boolean | null; expectedVersion?: number | null; importId?: string | null; pageId?: string | null; plan?: string | null; projectId: string; title?: string | null }

export type PlanImportOutput = unknown

export type ProfileAvatarListInput = Record<string, never>

export type ProfileAvatarListOutput = { avatars: AvatarLine[] }

export type ProfileGetInput = { account?: string | null }

export type ProfileGetOutput = { accountId: string; avatarUrl?: string | null; canEdit: boolean; dmPolicy?: string | null; handle: string; kind: string; name: string; pronouns?: string | null; sharedSpaces: SharedSpace[]; statuses: StatusView[]; timeZone?: string | null; title?: string | null }

export type ProfileUpdateInput = { account?: string | null; avatarBase64?: string | null; dmPolicy?: string | null; pronouns?: string | null; removeAvatar?: boolean; timeZone?: string | null; title?: string | null }

export type ProfileUpdateOutput = { accountId: string; avatarUrl?: string | null; canEdit: boolean; dmPolicy?: string | null; handle: string; kind: string; name: string; pronouns?: string | null; sharedSpaces: SharedSpace[]; statuses: StatusView[]; timeZone?: string | null; title?: string | null }

export type ProjectGetInput = { projectId: string }

export type ProjectGetOutput = { channels: ConversationSummary[]; decisions: DecisionLine[]; guide?: GuidePage | null; labels?: string[]; layout?: LayoutLine | null; memory?: MemoryPointer | null; modulesOff?: string[]; openThreads: ConversationSummary[]; pages: PageLine[]; project: ProjectHeader; repositories: RepositoryLine[]; skills?: SkillLine[]; taskCollections?: string[]; taskFields?: string[]; tasks: TaskOverview }

export type ProjectListInput = { includeArchived?: boolean | null }

export type ProjectListOutput = { projects: ProjectLine[] }

export type ProjectStatusGetInput = { spaceId: string }

export type ProjectStatusGetOutput = { canUpdate?: boolean | null; health?: string | null; healthLabel?: string | null; latestUpdate?: LatestUpdate | null; lead?: Person | null; spaceId: string; targetDate?: string | null; version: number }

export type ProjectStatusSetInput = { expectedVersion?: number | null; health?: string | null; lead?: string | null; spaceId: string; targetDate?: string | null }

export type ProjectStatusSetOutput = { canUpdate?: boolean | null; health?: string | null; healthLabel?: string | null; latestUpdate?: LatestUpdate | null; lead?: Person | null; spaceId: string; targetDate?: string | null; version: number }

export type ProjectTemplateCreateInput = { archived?: boolean | null; description?: string | null; expectedVersion?: number | null; name?: string | null; projectId: string; template?: string | null }

export type ProjectTemplateCreateOutput = { done: string; template: ProjectTemplateLine }

export type ProjectTemplateListInput = { includeArchived?: boolean | null; projectId: string }

export type ProjectTemplateListOutput = { templates: ProjectTemplateLine[] }

export type ProjectTemplateUpdateInput = { archived?: boolean | null; description?: string | null; expectedVersion?: number | null; name?: string | null; projectId: string; template?: string | null }

export type ProjectTemplateUpdateOutput = { done: string; template: ProjectTemplateLine }

export type ProjectUpdateCreateInput = { channelId?: string | null; health: string; spaceId: string; text: string; title?: string | null }

export type ProjectUpdateCreateOutput = { channelId: string; messageId: string; status: SpaceStatus; threadId: string }

export type ProposalDecideInput = { decision: DecisionArg; parts?: number[] | null; proposalId: string }

export type ProposalDecideOutput = { appliedVersion?: number | null; author?: string | null; authorId: string; baseVersion: number; change: unknown; createdAt: string; decidedAt?: string | null; decidedBy?: string | null; decisionNote?: string | null; id: string; note?: string | null; operation: string; preview: unknown; spaceId: string; status: string; thingId: string; thingKind: string; thingTitle?: string | null }

export type ProposalListInput = { includeDecided?: boolean; limit?: number | null; thingId?: string | null; thingKind?: string | null }

export type ProposalListOutput = { proposals: ProposalView[] }

export type PullRequestCreateInput = { body?: string | null; runId: string; taskIds?: string[] | null; title: string }

export type PullRequestCreateOutput = { job: Record<string, unknown>; queued: boolean }

export type PullRequestGetInput = { identifier: string; projectId?: string | null }

export type PullRequestGetOutput = { baseRef: string; canonicalUrl: string; checksState: string; draft: boolean; headRef: string; headSha: string; id: string; mergeabilityState: string; number: number; projectId: string; repositoryId: string; reviewState: string; state: string; suggested?: boolean; syncState: string; title: string; url: string; version: number }

export type PullRequestListInput = { checks?: string | null; cursor?: string | null; limit?: number | null; needsMe?: boolean | null; projectId?: string | null; repositoryId?: string | null; review?: string | null; state?: string | null; taskId?: string | null }

export type PullRequestListOutput = { nextCursor?: string | null; note?: string | null; pullRequests: PullRequestLine[]; total: number }

export type PullRequestMergeInput = { expectedHeadSha?: string | null; method?: string | null; pullRequestId: string }

export type PullRequestMergeOutput = { alreadyMerged: boolean; mergeCommitSha: string; method: string; number: number; pullRequestId: string }

export type PullRequestRefreshInput = { pullRequestId: string }

export type PullRequestRefreshOutput = { job: Record<string, unknown>; queued: boolean }

export type RecordCreateInput = { collection: string; fields?: Record<string, unknown>; fromMessageId?: string | null; saySo?: boolean; template?: string | null; title?: string | null }

export type RecordCreateOutput = { collectionId: string; id: string; number: number; title: string }

export type RecordTemplateCreateInput = { collection: string; fields?: Record<string, unknown>; name: string; title: string }

export type RecordTemplateCreateOutput = { archived?: boolean; collectionId: string; fields: Record<string, unknown>; id: string; name: string; title: string; version: number }

export type RecordTemplateListInput = { collection: string; includeArchived?: boolean }

export type RecordTemplateListOutput = { templates: TemplateView[] }

export type RecordTemplateUpdateInput = { archived?: boolean | null; expectedVersion?: number | null; fields?: Record<string, unknown> | null; name?: string | null; template: string; title?: string | null }

export type RecordTemplateUpdateOutput = { archived?: boolean; collectionId: string; fields: Record<string, unknown>; id: string; name: string; title: string; version: number }

export type RecordUpdateInput = { archived?: boolean | null; expectedVersion?: number | null; fields?: Record<string, unknown>; note?: string | null; record: string; title?: string | null }

export type RecordUpdateOutput = { changed: string[]; collectionId: string; id: string; number: number; proposalId?: string | null; proposed?: unknown[]; title: string; version: number }

export type RecurringCreateInput = { assignee?: string | null; clearDueInDays?: boolean | null; delete?: boolean | null; dueInDays?: number | null; enabled?: boolean | null; every?: number | null; expectedVersion?: number | null; frequency?: string | null; monthDay?: number | null; projectId: string; recurring?: string | null; startsOn?: string | null; template?: string | null; timeOfDay?: string | null; timezone?: string | null; weekdays?: number[] | null }

export type RecurringCreateOutput = { created: boolean; deleted: boolean; recurring?: unknown }

export type RecurringRecordCreateInput = { every?: number | null; frequency: string; monthDay?: number | null; startsOn?: string | null; template: string; timeOfDay?: string | null; timezone?: string | null; weekdays?: number[] }

export type RecurringRecordCreateOutput = { collectionId: string; enabled: boolean; every: number; frequency: string; id: string; lastRecordId?: string | null; lastRunAt?: string | null; monthDay?: number | null; nextRunAt?: string | null; ownerId: string; ownerName?: string | null; pausedReason?: string | null; startsOn: string; templateId: string; templateName: string; timeOfDay: string; timezone: string; version: number; weekdays?: number[] }

export type RecurringRecordDeleteInput = { recurring: string }

export type RecurringRecordDeleteOutput = { id: string }

export type RecurringRecordListInput = { collection: string; includeArchived?: boolean }

export type RecurringRecordListOutput = { recurring: RecurringView[] }

export type RecurringRecordUpdateInput = { enabled?: boolean | null; expectedVersion?: number | null; recurring: string; schedule?: ScheduleInput | null }

export type RecurringRecordUpdateOutput = { collectionId: string; enabled: boolean; every: number; frequency: string; id: string; lastRecordId?: string | null; lastRunAt?: string | null; monthDay?: number | null; nextRunAt?: string | null; ownerId: string; ownerName?: string | null; pausedReason?: string | null; startsOn: string; templateId: string; templateName: string; timeOfDay: string; timezone: string; version: number; weekdays?: number[] }

export type RecurringUpdateInput = { assignee?: string | null; clearDueInDays?: boolean | null; delete?: boolean | null; dueInDays?: number | null; enabled?: boolean | null; every?: number | null; expectedVersion?: number | null; frequency?: string | null; monthDay?: number | null; projectId: string; recurring?: string | null; startsOn?: string | null; template?: string | null; timeOfDay?: string | null; timezone?: string | null; weekdays?: number[] | null }

export type RecurringUpdateOutput = { created: boolean; deleted: boolean; recurring?: unknown }

export type ReplyClaimInput = { requestId: string }

export type ReplyClaimOutput = { instructions: string; memory?: MemoryPointer | null; recentMessages: ContextMessage[]; request: InboxItem; run?: ClaimRun | null; session?: ClaimSession | null }

export type ReplyPassInput = { generation?: number | null; reason?: string | null; requestId: string }

export type ReplyPassOutput = { messageId?: string | null; requestId: string; status: string }

export type ReplyProgressReportInput = { generation: number; requestId: string; status: string }

export type ReplyProgressReportOutput = { leaseExpiresAt: string; requestId: string; status: string }

export type ReplyReleaseInput = { generation?: number | null; reason: string; requestId: string }

export type ReplyReleaseOutput = { messageId?: string | null; requestId: string; status: string }

export type ReplyRetryInput = { requestId: string }

export type ReplyRetryOutput = { requestId: string; status: string }

export type ReplySendInput = { attachmentIds?: string[] | null; final?: boolean | null; generation: number; requestId: string; text: string }

export type ReplySendOutput = { messageId?: string | null; requestId: string; status: string }

export type ReplyStopInput = { requestId: string }

export type ReplyStopOutput = { requestId: string; status: string; stoppedRequestIds: string[] }

export type RepositoryArchiveInput = { expectedVersion?: number | null; projectId: string; repository: string }

export type RepositoryArchiveOutput = { done: string; repository: Record<string, unknown> }

export type RepositoryCreateInput = { defaultBranch?: string | null; githubRepositoryId?: number | null; installationId?: number | null; name?: string | null; owner?: string | null; projectId: string; trustAcknowledged: boolean }

export type RepositoryCreateOutput = { done: string; repository: Record<string, unknown> }

export type RepositoryGetInput = { projectId?: string | null; repositoryId: string }

export type RepositoryGetOutput = { archived: boolean; canonicalUrl: string; cloneUrl: string; defaultBranch: string; enabled: boolean; id: string; name: string; owner: string; preparationState: string; projectId: string; trusted: boolean; version: number }

export type RepositoryListInput = { includeArchived?: boolean | null; projectId?: string | null }

export type RepositoryListOutput = { repositories: RepositoryItem[] }

export type RepositoryPrepareInput = { expectedVersion?: number | null; projectId: string; repository: string }

export type RepositoryPrepareOutput = { done: string; repository: Record<string, unknown> }

export type RequestCancelInput = { requestId: string }

export type RequestCancelOutput = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type RequestCreateInput = { allowText?: boolean | null; audience: AudienceInput; conversationId?: string | null; detail?: string | null; expiresInSeconds?: number | null; kind?: string | null; onExpiry?: string | null; options?: RequestOptionInput[]; sensitive?: boolean | null; spaceId?: string | null; taskId?: string | null; title: string }

export type RequestCreateOutput = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type RequestGetInput = { requestId: string }

export type RequestGetOutput = { mayAnswer: boolean; request: RequestView }

export type RequestListInput = { limit?: number | null; role?: string | null; spaceId?: string | null; status?: string | null }

export type RequestListOutput = { requests: RequestView[] }

export type RequestRespondInput = { optionId?: string | null; requestId: string; text?: string | null }

export type RequestRespondOutput = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type RoleAssignInput = { account: string; applyToProjects?: boolean; expectedVersion?: number | null; role: string }

export type RoleAssignOutput = { actorId: string; projectsChanged: number; projectsSkipped: number; role: string; roleId?: string | null; roleName: string; skippedProjects: SkippedProject[] }

export type RoleCreateInput = { actWithinRequester?: boolean; agentLimit?: string[] | null; baseRole: string; defaults?: RoleDefaults; description?: string; isDefault?: boolean; modulesOff?: string[]; name: string; permissions: string[] }

export type RoleCreateOutput = { actWithinRequester: boolean; agentLimit?: string[] | null; baseRole: string; builtIn: boolean; capabilities: RoleCapabilities; defaults: RoleDefaults; description: string; id: string; isDefault: boolean; key: string; modulesOff: string[]; name: string; permissions: string[]; projectHolders: number; updatedAt: string; version: number; workspaceHolders: number }

export type RoleDeleteInput = { role: string }

export type RoleDeleteOutput = { roleId: string }

export type RoleGetInput = { role: string }

export type RoleGetOutput = { actWithinRequester: boolean; agentLimit?: string[] | null; baseRole: string; builtIn: boolean; capabilities: RoleCapabilities; defaults: RoleDefaults; description: string; id: string; isDefault: boolean; key: string; modulesOff: string[]; name: string; permissions: string[]; projectHolders: number; updatedAt: string; version: number; workspaceHolders: number }

export type RoleListInput = Record<string, never>

export type RoleListOutput = { canManage: boolean; canManageAdminRoles: boolean; modules: ModuleEntry[]; permissions: PermissionEntry[]; roles: RoleView[]; startingPoints: StartingPoint[]; yourPermissions: string[] }

export type RoleUpdateInput = { actWithinRequester?: boolean | null; agentLimit?: string[] | null; clearAgentLimit?: boolean; defaults?: RoleDefaults | null; description?: string | null; expectedVersion: number; isDefault?: boolean | null; modulesOff?: string[] | null; name?: string | null; permissions?: string[] | null; role: string }

export type RoleUpdateOutput = { actWithinRequester: boolean; agentLimit?: string[] | null; baseRole: string; builtIn: boolean; capabilities: RoleCapabilities; defaults: RoleDefaults; description: string; id: string; isDefault: boolean; key: string; modulesOff: string[]; name: string; permissions: string[]; projectHolders: number; updatedAt: string; version: number; workspaceHolders: number }

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

export type RunFailureHideInput = { runId: string }

export type RunFailureHideOutput = { approvalsWaiting: number; branch?: string | null; createdAt: string; failure?: string | null; id: string; liveness: string; next: string; pollAfterSeconds?: number | null; projectId?: string | null; prompt: string; pullRequest?: RunPullRequestLine | null; queuePosition?: number | null; stage: string; startedBy: string; state: string; statusLine?: string | null; threadId?: string | null; updatedAt: string; url: string }

export type RunGetInput = { runId: string }

export type RunGetOutput = { approvalsWaiting: number; branch?: string | null; createdAt: string; failure?: string | null; id: string; liveness: string; next: string; pollAfterSeconds?: number | null; projectId?: string | null; prompt: string; pullRequest?: RunPullRequestLine | null; queuePosition?: number | null; stage: string; startedBy: string; state: string; statusLine?: string | null; threadId?: string | null; updatedAt: string; url: string }

export type RunListInput = { limit?: number | null; mine?: boolean | null; projectId?: string | null; state?: string | null }

export type RunListOutput = { runs: RunValue[] }

export type RunMarkNotStartedInput = { runId: string }

export type RunMarkNotStartedOutput = { idempotentReplay?: boolean; run: RunRecord }

export type RunPublishInput = { runId: string }

export type RunPublishOutput = { job: Record<string, unknown>; queued: boolean }

export type RunResultReportInput = { results: RunResultInput[]; runId: string }

export type RunResultReportOutput = { added: number; run: RunRecord }

export type RunRetryInput = { clientRequestId?: string | null; runId: string }

export type RunRetryOutput = { approvalsWaiting: number; branch?: string | null; createdAt: string; failure?: string | null; id: string; liveness: string; next: string; pollAfterSeconds?: number | null; projectId?: string | null; prompt: string; pullRequest?: RunPullRequestLine | null; queuePosition?: number | null; stage: string; startedBy: string; state: string; statusLine?: string | null; threadId?: string | null; updatedAt: string; url: string }

export type RunStartInput = { clientRequestId?: string | null; harness?: string | null; model?: string | null; prompt: string; reasoning?: string | null; repositoryId?: string | null; speed?: string | null; threadId: string }

export type RunStartOutput = { approvalsWaiting: number; branch?: string | null; createdAt: string; failure?: string | null; id: string; liveness: string; next: string; pollAfterSeconds?: number | null; projectId?: string | null; prompt: string; pullRequest?: RunPullRequestLine | null; queuePosition?: number | null; stage: string; startedBy: string; state: string; statusLine?: string | null; threadId?: string | null; updatedAt: string; url: string }

export type RunStepListInput = { includeDiff?: boolean; runId: string }

export type RunStepListOutput = { diff?: Record<string, unknown> | null; run: Record<string, unknown> }

export type RunStopInput = { clientRequestId?: string | null; runId: string }

export type RunStopOutput = { approvalsWaiting: number; branch?: string | null; createdAt: string; failure?: string | null; id: string; liveness: string; next: string; pollAfterSeconds?: number | null; projectId?: string | null; prompt: string; pullRequest?: RunPullRequestLine | null; queuePosition?: number | null; stage: string; startedBy: string; state: string; statusLine?: string | null; threadId?: string | null; updatedAt: string; url: string }

export type RunUpdateInput = { backend?: string | null; expectedVersion?: number | null; externalUrl?: string | null; failureReason?: string | null; outcomeSummary?: string | null; runId: string; state?: string | null; statusLine?: string | null; title?: string | null }

export type RunUpdateOutput = { idempotentReplay?: boolean; run: RunRecord }

export type RunUsageListInput = { days?: number | null; groupBy?: string | null; projectId?: string | null }

export type RunUsageListOutput = { usage: Record<string, unknown> }

export type RunUsageReportInput = { cachedInputTokens?: number | null; cost?: number | null; currency?: string | null; inputTokens?: number | null; model: string; outputTokens?: number | null; reportKey: string; runId?: string | null; spaceId?: string | null; windowEnd?: string | null; windowStart?: string | null }

export type RunUsageReportOutput = { duplicate: boolean; reportId: string; runId?: string | null }

export type SavedSearchListInput = Record<string, never>

export type SavedSearchListOutput = { searches: SavedSearch[] }

export type SavedSearchSetInput = { delete?: boolean | null; name?: string | null; position?: number | null; query?: string | null; savedSearchId?: string | null }

export type SavedSearchSetOutput = { searches: SavedSearch[] }

export type ScheduleCreateInput = { about?: string | null; at?: string | null; everyMinutes?: number | null; inMinutes?: number | null; note?: string | null }

export type ScheduleCreateOutput = { createdAt: string; everyMinutes?: number | null; id: string; lastDueAt?: string | null; nextDueAt?: string | null; note: string; state: string; thingId?: string | null; thingKind?: string | null; timesDue: number; version: number }

export type ScheduleListInput = { includeFinished?: boolean | null }

export type ScheduleListOutput = { schedules: ScheduleView[] }

export type ScheduleUpdateInput = { at?: string | null; cancel?: boolean | null; everyMinutes?: number | null; expectedVersion?: number | null; inMinutes?: number | null; note?: string | null; scheduleId: string }

export type ScheduleUpdateOutput = { createdAt: string; everyMinutes?: number | null; id: string; lastDueAt?: string | null; nextDueAt?: string | null; note: string; state: string; thingId?: string | null; thingKind?: string | null; timesDue: number; version: number }

export type ScheduledMessageUpdateInput = { cancel?: boolean | null; id: string; sendAt?: string | null; sendNow?: boolean | null; text?: string | null }

export type ScheduledMessageUpdateOutput = { canceled: boolean; scheduled?: ScheduledLine | null }

export type SettingListInput = { keys?: string[] | null; spaceId?: string | null }

export type SettingListOutput = { capabilities: SettingCapabilities; organizationId: string; projectId?: string | null; settings: Resolved[] }

export type SettingUpdateInput = { expectedVersion?: number | null; key: string; reset?: boolean; scope?: Scope | null; spaceId?: string | null; value?: unknown }

export type SettingUpdateOutput = { done: string; setting: Resolved; version?: number | null }

export type SheetRangeSetInput = { name: string; range?: string | null; remove?: boolean; sheetId: string }

export type SheetRangeSetOutput = { ranges: NamedRange[] }

export type SheetSearchInput = { inFormulas?: boolean; limit?: number | null; matchCase?: boolean; query: string; sheetId: string; tab?: string | null; wholeCell?: boolean }

export type SheetSearchOutput = { matches: SheetMatch[]; revision: number; truncated: boolean }

export type SheetViewCreateInput = { filters?: ViewFilter[]; hiddenColumns?: string[]; name: string; shared?: boolean; sheetId: string; sort?: ViewSort[]; tab?: string | null }

export type SheetViewCreateOutput = { filters: ViewFilter[]; hiddenColumns: string[]; id: string; name: string; shared: boolean; sheetId: string; sort: ViewSort[]; tab: string; tabId: string; updatedAt: string; version: number }

export type SheetViewDeleteInput = { viewId: string }

export type SheetViewDeleteOutput = { deleted: string }

export type SheetViewListInput = { sheetId: string; tab?: string | null }

export type SheetViewListOutput = { views: SheetViewOut[] }

export type SheetViewUpdateInput = { expectedVersion?: number | null; filters?: ViewFilter[] | null; hiddenColumns?: string[] | null; name?: string | null; sort?: ViewSort[] | null; viewId: string }

export type SheetViewUpdateOutput = { filters: ViewFilter[]; hiddenColumns: string[]; id: string; name: string; shared: boolean; sheetId: string; sort: ViewSort[]; tab: string; tabId: string; updatedAt: string; version: number }

export type SidebarItemMoveInput = { conversationId: string; position?: number | null; sectionId?: string | null; starred?: boolean | null }

export type SidebarItemMoveOutput = { sections: SidebarSection[]; workspaceSpaceId?: string | null }

export type SidebarListInput = Record<string, never>

export type SidebarListOutput = { sections: SidebarSection[]; workspaceSpaceId?: string | null }

export type SidebarSectionSetInput = { collapsed?: boolean | null; delete?: boolean | null; expectedVersion?: number | null; name?: string | null; position?: number | null; sectionId?: string | null; show?: SectionShow | null; sort?: SectionSort | null }

export type SidebarSectionSetOutput = { sections: SidebarSection[]; workspaceSpaceId?: string | null }

export type SkillGetInput = { name?: string | null; pageId?: string | null; projectId?: string | null }

export type SkillGetOutput = { body: string; description: string; name: string; pageId: string; projectId: string; skillMd: string; stale: boolean; truncated?: boolean; updatedAt: string; url: string; version: number }

export type SkillListInput = { projectId: string }

export type SkillListOutput = { skills: SkillLine[] }

export type SpaceArchiveInput = { expectedVersion?: number | null; spaceId: string }

export type SpaceArchiveOutput = { archived: boolean; description: string; icon?: string | null; id: string; key: string; kind: StoredKind; name: string; role?: string | null; url: string; version: number; visibility: SpaceVisibility; workspaceId: string }

export type SpaceCreateInput = { description?: string | null; icon?: string | null; key: string; kind?: CreatableKind | null; name: string; shape?: SpaceShape | null; templateId?: string | null; visibility?: SpaceVisibility | null }

export type SpaceCreateOutput = { archived: boolean; description: string; icon?: string | null; id: string; key: string; kind: StoredKind; name: string; role?: string | null; url: string; version: number; visibility: SpaceVisibility; workspaceId: string }

export type SpaceGroupAddInput = { group: string; role?: string | null; spaceId: string }

export type SpaceGroupAddOutput = { groupId: string; handle: string; role: string; roleId: string; seats: number; spaceId: string }

export type SpaceGroupRemoveInput = { group: string; spaceId: string }

export type SpaceGroupRemoveOutput = { groupId: string; removed: boolean; spaceId: string }

export type SpaceJoinInput = { spaceId: string }

export type SpaceJoinOutput = { accountId: string; active: boolean; expiresAt?: string | null; handle: string; name: string; role: string; roleId?: string | null; spaceId: string; version: number; viaGroup: boolean }

export type SpaceLeaveInput = { spaceId: string }

export type SpaceLeaveOutput = { accountId: string; active: boolean; expiresAt?: string | null; handle: string; name: string; role: string; roleId?: string | null; spaceId: string; version: number; viaGroup: boolean }

export type SpaceListInput = { includeArchived?: boolean; kinds?: SpaceKind[] | null }

export type SpaceListOutput = { spaces: SpaceView[] }

export type SpaceMemberAddInput = { account: string; expiresAt?: string | null; role?: string | null; spaceId: string }

export type SpaceMemberAddOutput = { accountId: string; active: boolean; expiresAt?: string | null; handle: string; name: string; role: string; roleId?: string | null; spaceId: string; version: number; viaGroup: boolean }

export type SpaceMemberRemoveInput = { account: string; expectedVersion?: number | null; spaceId: string }

export type SpaceMemberRemoveOutput = { accountId: string; active: boolean; expiresAt?: string | null; handle: string; name: string; role: string; roleId?: string | null; spaceId: string; version: number; viaGroup: boolean }

export type SpaceMemberUpdateInput = { account: string; expectedVersion?: number | null; expiresAt?: string | null; role: string; spaceId: string }

export type SpaceMemberUpdateOutput = { accountId: string; active: boolean; expiresAt?: string | null; handle: string; name: string; role: string; roleId?: string | null; spaceId: string; version: number; viaGroup: boolean }

export type SpaceOpenInput = { spaceId: string }

export type SpaceOpenOutput = { archived: boolean; description: string; icon?: string | null; id: string; key: string; kind: StoredKind; name: string; role?: string | null; url: string; version: number; visibility: SpaceVisibility; workspaceId: string }

export type SpacePersonalOpenInput = Record<string, never>

export type SpacePersonalOpenOutput = { archived: boolean; description: string; icon?: string | null; id: string; key: string; kind: StoredKind; name: string; role?: string | null; url: string; version: number; visibility: SpaceVisibility; workspaceId: string }

export type SpaceRestoreInput = { expectedVersion?: number | null; spaceId: string }

export type SpaceRestoreOutput = { archived: boolean; description: string; icon?: string | null; id: string; key: string; kind: StoredKind; name: string; role?: string | null; url: string; version: number; visibility: SpaceVisibility; workspaceId: string }

export type SpaceUpdateInput = { description?: string | null; expectedVersion?: number | null; icon?: string | null; name?: string | null; spaceId: string; visibility?: SpaceVisibility | null }

export type SpaceUpdateOutput = { archived: boolean; description: string; icon?: string | null; id: string; key: string; kind: StoredKind; name: string; role?: string | null; url: string; version: number; visibility: SpaceVisibility; workspaceId: string }

export type StatusDeleteInput = { account?: string | null; workspaceOnly?: boolean }

export type StatusDeleteOutput = { cleared: boolean }

export type StatusListInput = { accounts: string[] }

export type StatusListOutput = { statuses: StatusView[] }

export type StatusSetInput = { account?: string | null; awayUntil?: string | null; clearAt?: string | null; emoji?: string | null; state?: string | null; text?: string | null; workspaceOnly?: boolean }

export type StatusSetOutput = { accountId: string; awayUntil?: string | null; clearAt?: string | null; emoji?: string | null; state?: string | null; text?: string | null; updatedAt: string; workspaceOnly: boolean }

export type TaskArchiveInput = { expectedVersion: number; taskId: string }

export type TaskArchiveOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; proposalId?: string | null; proposed?: unknown[]; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskClaimInput = { minutes?: number | null; note?: string | null; taskId: string }

export type TaskClaimOutput = { claimedAt: string; claimedBy: string; expiresAt: string; handoff?: HandoffNote | null; note?: string | null; taskId: string }

export type TaskCommentCreateInput = { attachmentIds?: string[] | null; replyToCommentId?: string | null; taskId: string; text: string }

export type TaskCommentCreateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; proposalId?: string | null; proposed?: unknown[]; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskCommentDeleteInput = { commentId: string; taskId: string }

export type TaskCommentDeleteOutput = { alreadyDeleted: boolean; commentId: string; note: string; taskId: string }

export type TaskCommentUpdateInput = { commentId: string; expectedRevision?: number | null; taskId: string; text: string }

export type TaskCommentUpdateOutput = { changed: boolean; commentId: string; revision: number; taskId: string }

export type TaskCreateInput = { acceptanceCriteria?: string[] | null; afterTaskId?: string | null; assignee?: string | null; beforeTaskId?: string | null; collection?: string | null; description?: string | null; dueDate?: string | null; estimate?: number | null; fields?: Record<string, unknown> | null; fromMessageId?: string | null; labels?: string[] | null; milestone?: string | null; parentId?: string | null; position?: string | null; priority?: string | null; projectId?: string | null; saySo?: boolean | null; startDate?: string | null; status?: string | null; title: string; type?: string | null }

export type TaskCreateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; proposalId?: string | null; proposed?: unknown[]; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskCreateManyInput = { clientRequestId?: string | null; createMissingLabels?: boolean | null; preview?: boolean | null; projectId?: string | null; tasks: NewTask[] }

export type TaskCreateManyOutput = { idempotentReplay?: boolean; labelsCreated?: string[]; notes?: string[]; preview: boolean; projectKey: string; tasks: CreatedTask[] }

export type TaskCriteriaUpdateInput = { expectedVersion: number; operations: CriterionOperationArgument[]; taskId: string }

export type TaskCriteriaUpdateOutput = { completed: number; criteria: CriterionValue[]; idempotentReplay?: boolean; key: string; operations: CriterionOperationOutcome[]; taskId: string; total: number; url: string; version: number }

export type TaskGetInput = { historyBefore?: string | null; historyLimit?: number | null; include?: string[] | null; taskId: string }

export type TaskGetOutput = { blockedBy?: TaskRef[]; blockedReason?: string | null; blocks?: TaskRef[]; claimedBy?: ClaimValue | null; collection?: string | null; comments?: CommentItem[]; conversations?: LinkedItem[]; dueDate?: string | null; estimate?: number | null; estimateUnit?: string; fields?: TaskFieldValue[]; files?: FileSummary[]; guidePageId?: string | null; handoffs?: HandoffNote[]; history?: TaskHistory | null; links?: LinkedItem[]; milestone?: MilestoneRef | null; overdue?: boolean; parent?: TaskRef | null; ready?: boolean; relations?: RelationRef[]; startDate?: string | null; subtasks?: TaskSummary[]; task: WorkItemProjection; watchers?: WatcherRef[] }

export type TaskImportInput = { commit?: boolean | null; githubState?: string | null; headingsAsMilestones?: boolean | null; importId?: string | null; mapping?: Record<string, string> | null; projectId: string; repositoryId?: string | null; source?: string | null; text?: string | null }

export type TaskImportOutput = { import: unknown; nextStep: string }

export type TaskLaunchInput = { agentAccountId?: string | null; attempts?: number | null; clientRequestId?: string | null; environmentId?: string | null; harness?: string | null; model?: string | null; prompt?: string | null; reasoning?: string | null; speed?: string | null; startAt?: string | null; startWhen?: string | null; taskIds: string[] }

export type TaskLaunchOutput = { launches: LaunchValue[] }

export type TaskLinkInput = { blockedBy?: string[] | null; blocks?: string[] | null; close?: boolean | null; conversationId?: string | null; decisionId?: string | null; expectedVersion?: number | null; kind?: string | null; messageId?: string | null; otherTaskId?: string | null; pageId?: string | null; pullRequestId?: string | null; remove?: boolean | null; repositoryId?: string | null; role?: string | null; taskId: string; whiteboardId?: string | null }

export type TaskLinkOutput = DependencyValue | RelationsValue | LinkValue

export type TaskLinkManyInput = { links: DependencyLinks[]; projectId?: string | null; remove?: boolean | null }

export type TaskLinkManyOutput = { changed: number; tasks: DependencyChange[] }

export type TaskListInput = { anyLabels?: string[] | null; assignee?: string[] | null; blocked?: boolean | null; claimed?: string | null; collection?: string | null; createdBy?: string | null; cursor?: string | null; delegate?: string[] | null; dueAfter?: string | null; dueBefore?: string | null; fieldFilters?: FieldFilterArgument[] | null; includeArchived?: boolean | null; includeDone?: boolean | null; labels?: string[] | null; limit?: number | null; milestone?: string[] | null; notAssignee?: string[] | null; notLabels?: string[] | null; notMilestone?: string[] | null; notPriority?: string[] | null; notStatus?: string[] | null; notType?: string[] | null; overdue?: boolean | null; parentId?: string | null; priority?: string[] | null; projectId?: string | null; projectIds?: string[] | null; query?: string | null; ready?: boolean | null; sort?: string | null; status?: string[] | null; type?: string[] | null; updatedSince?: string | null; view?: string | null }

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

export type TaskRestoreOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; proposalId?: string | null; proposed?: unknown[]; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskUpdateInput = { addLabels?: string[] | null; afterTaskId?: string | null; archived?: boolean | null; assignee?: string | null; beforeTaskId?: string | null; blockedReason?: string | null; clearAssignee?: boolean | null; clearEstimate?: boolean | null; clearParent?: boolean | null; cycle?: string | null; delegate?: string | null; description?: string | null; dueDate?: string | null; estimate?: number | null; expectedVersion: number; fields?: Record<string, unknown> | null; milestone?: string | null; note?: string | null; parentId?: string | null; position?: string | null; priority?: string | null; readyForReview?: boolean | null; removeLabels?: string[] | null; startDate?: string | null; status?: string | null; taskId: string; title?: string | null; type?: string | null }

export type TaskUpdateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; proposalId?: string | null; proposed?: unknown[]; resultId: string; url?: string | null; version: number; workItemId: string }

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

export type ThemeCreateInput = { document: ThemeDocument; name: string }

export type ThemeCreateOutput = { darkBase: string; document: ThemeDocument; id: string; lightBase: string; mine: boolean; name: string; offered: boolean; ownerAccountId: string; updatedAt: string; version: number }

export type ThemeDeleteInput = { id: string }

export type ThemeDeleteOutput = { deleted: boolean; id: string }

export type ThemeGetInput = { id: string }

export type ThemeGetOutput = { darkBase: string; document: ThemeDocument; id: string; lightBase: string; mine: boolean; name: string; offered: boolean; ownerAccountId: string; updatedAt: string; version: number }

export type ThemeListInput = Record<string, never>

export type ThemeListOutput = { builtIn: string[]; canOffer: boolean; format: number; themes: ThemeSummary[] }

export type ThemeUpdateInput = { document?: ThemeDocument | null; expectedVersion: number; id: string; name?: string | null }

export type ThemeUpdateOutput = { darkBase: string; document: ThemeDocument; id: string; lightBase: string; mine: boolean; name: string; offered: boolean; ownerAccountId: string; updatedAt: string; version: number }

export type ThingLinkInput = { from: string; kind: HandLinkKind; to: string }

export type ThingLinkOutput = { changed: boolean }

export type ThingLinkCreateInput = { expiresInHours?: number | null; id: string; kind: ThingKind }

export type ThingLinkCreateOutput = { expiresAt: string; id: string; token?: string | null }

export type ThingShareAddInput = { account: string; expiresAt?: string | null; id: string; kind: ThingKind }

export type ThingShareAddOutput = { accounts: SharedAccount[]; id: string; kind: ThingKind; links: SharedLink[] }

export type ThingShareRemoveInput = { account?: string | null; id: string; kind: ThingKind; link?: string | null }

export type ThingShareRemoveOutput = { accounts: SharedAccount[]; id: string; kind: ThingKind; links: SharedLink[] }

export type ThingUnlinkInput = { from: string; kind: HandLinkKind; to: string }

export type ThingUnlinkOutput = { changed: boolean }

export type ThingUpsertInput = { archived?: boolean | null; fields?: Record<string, unknown> | null; id?: string | null; kind: string; spaceId?: string | null; status?: string | null; title: string; url?: string | null }

export type ThingUpsertOutput = { archived: boolean; existed: boolean; fields: Record<string, unknown>; id: string; kind: string; spaceId?: string | null; status?: string | null; title: string; url?: string | null; version: number }

export type ThreadFollowedListInput = { limit?: number | null; openOnly?: boolean }

export type ThreadFollowedListOutput = { threads: FollowedThreadView[] }

export type ThreadStartInput = { conversationId: string; fromMessageId?: string | null; name?: string | null; text?: string | null }

export type ThreadStartOutput = { created?: boolean | null; id: string; message?: SentMessage | null; name: string; outcome?: string | null; parentId: string; status: string; url: string }

export type ThreadUpdateInput = { clearOutcome?: boolean | null; name?: string | null; outcome?: string | null; status?: string | null; threadId: string }

export type ThreadUpdateOutput = { created?: boolean | null; id: string; message?: SentMessage | null; name: string; outcome?: string | null; parentId: string; status: string; url: string }

export type TriageDecideInput = { action: TriageAction; duplicateOf?: string | null; reason?: string | null; status?: string | null; taskIds: string[]; until?: string | null }

export type TriageDecideOutput = { tasks: TriagedTask[] }

export type TriageEnableInput = { spaceId: string }

export type TriageEnableOutput = { statusId: string }

export type TriageRuleListInput = { spaceId: string }

export type TriageRuleListOutput = { rules: Rule[]; triageStatusId?: string | null }

export type TriageRuleSetInput = { action?: RuleAction | null; enabled?: boolean | null; expectedVersion?: number | null; filter?: unknown; name?: string | null; position?: number | null; remove?: boolean; rule?: string | null; spaceId: string }

export type TriageRuleSetOutput = { removed: boolean; rule?: Rule | null }

export type UsageBatchLaunchInput = { accountId: string; clientRequestId?: string | null; taskIds?: string[] | null; windowKey?: string | null }

export type UsageBatchLaunchOutput = { failed: FailedLine[]; launched: number; projects: LaunchedLine[] }

export type UsageBatchPreviewInput = { accountId: string; windowKey?: string | null }

export type UsageBatchPreviewOutput = { account: string; harness: string; more: BatchTaskLine[]; size: number; sizing: string; skipped: SkippedLine[]; tasks: BatchTaskLine[] }

export type UsageWindowListInput = { accountId?: string | null }

export type UsageWindowListOutput = { accounts: AccountLine[] }

export type ViewCreateInput = { collection: string; config?: unknown; name: string; position?: number | null; shared?: boolean; type: string }

export type ViewCreateOutput = { builtin?: string | null; collectionId: string; config: unknown; id: string; listParams?: Record<string, string> | null; name: string; pinPosition?: number | null; pinned: boolean; position: number; shared: boolean; type: string; version: number }

export type ViewDefaultSetInput = { collection: string; role?: string | null; view?: string | null }

export type ViewDefaultSetOutput = { defaultViewId?: string | null }

export type ViewDeleteInput = { view: string }

export type ViewDeleteOutput = { removed: string }

export type ViewListInput = { collection: string }

export type ViewListOutput = { defaultViewId?: string | null; views: ViewEntry[] }

export type ViewPinInput = { pinned: boolean; position?: number | null; view: string }

export type ViewPinOutput = { builtin?: string | null; collectionId: string; config: unknown; id: string; listParams?: Record<string, string> | null; name: string; pinPosition?: number | null; pinned: boolean; position: number; shared: boolean; type: string; version: number }

export type ViewUpdateInput = { config?: unknown; expectedVersion?: number | null; name?: string | null; position?: number | null; share?: boolean; view: string }

export type ViewUpdateOutput = { builtin?: string | null; collectionId: string; config: unknown; id: string; listParams?: Record<string, string> | null; name: string; pinPosition?: number | null; pinned: boolean; position: number; shared: boolean; type: string; version: number }

export type WhiteboardCreateInput = { fromTemplateId?: string | null; linkTo?: LinkArgument | null; operations?: Operation[] | null; projectId?: string | null; title: string; workspaceId?: string | null }

export type WhiteboardCreateOutput = { edit?: EditOutcome | null; idempotentReplay?: boolean; whiteboard: WhiteboardLine }

export type WhiteboardDuplicateInput = { title?: string | null; whiteboardId: string }

export type WhiteboardDuplicateOutput = { edit?: EditOutcome | null; idempotentReplay?: boolean; whiteboard: WhiteboardLine }

export type WhiteboardLinkInput = { remove?: boolean | null; targetId: string; targetKind: string; whiteboardId: string }

export type WhiteboardLinkOutput = { changed: boolean; linked: boolean; links: LinkView[]; whiteboardId: string }

export type WhiteboardProposeInput = { elements: unknown[]; note?: string | null; whiteboardId: string }

export type WhiteboardProposeOutput = { elements: number; proposalId?: string | null }

export type WhiteboardTemplateListInput = { spaceId?: string | null }

export type WhiteboardTemplateListOutput = { templates: WhiteboardTemplateView[] }

export type WhiteboardUpdateInput = { deleted?: boolean | null; expectedVersion?: number | null; isTemplate?: boolean | null; projectId?: string | null; restoreRevision?: number | null; title?: string | null; whiteboardId: string; workspaceWide?: boolean | null }

export type WhiteboardUpdateOutput = { edit?: EditOutcome | null; idempotentReplay?: boolean; whiteboard: WhiteboardLine }

export type WhiteboardUpdateElementsInput = { confirmDelete?: boolean | null; expectedSceneSeq?: number | null; operations: Operation[]; whiteboardId: string }

export type WhiteboardUpdateElementsOutput = { changed: string[]; proposalId?: string | null; results: OperationResult[]; savedRevision?: number | null; sceneSeq: number }

export type WorkflowGetInput = { projectId: string }

export type WorkflowGetOutput = { estimateUnit?: string; statuses: WorkflowStatus[]; transitions: WorkflowTransition[]; types: WorkflowType[]; version?: number }

export type WorkflowUpdateInput = { allTransitions?: boolean | null; estimateUnit?: string | null; expectedVersion: number; projectId: string; replacements?: Record<string, string> | null; statuses: WorkflowStatusDraft[]; transitions?: WorkflowTransitionDraft[] | null; types: WorkflowTypeDraft[] }

export type WorkflowUpdateOutput = { statusesAdded: number; statusesRetired: number; tasksMoved: number; typesAdded: number; typesRetired: number; workflow: WorkflowValue }

export type WorkspaceDeleteInput = { confirm: string }

export type WorkspaceDeleteOutput = { deleteAfter?: string | null; lifecycle: Lifecycle; ownedAccounts: number }

export type WorkspaceDeleteCancelInput = Record<string, never>

export type WorkspaceDeleteCancelOutput = { deleteAfter?: string | null; lifecycle: Lifecycle; ownedAccounts: number }

export type WorkspaceExportStartInput = Record<string, never>

export type WorkspaceExportStartOutput = { byteSize?: number | null; counts: unknown; createdAt: string; direction: string; downloadUrl?: string | null; error?: string | null; expiresAt?: string | null; finishedAt?: string | null; id: string; requestedByActorId: string; state: string }

export type WorkspaceMemberUpdateInput = { account: string; expiresAt: string }

export type WorkspaceMemberUpdateOutput = { accountId: string; active: boolean; expiresAt?: string | null; version: number }

export type WorkspaceSearchInput = { conversationId?: string | null; from?: string | null; kinds?: string[] | null; limit?: number | null; projectId?: string | null; query: string; since?: string | null; until?: string | null }

export type WorkspaceSearchOutput = { hint: string; results: SearchResult[] }

export type WorkspaceShapeSetInput = { planHash?: string | null; shape: string }

export type WorkspaceShapeSetOutput = { installed: Installed; name: string; shape: string }

export type WorkspaceStoreGetInput = Record<string, never>

export type WorkspaceStoreGetOutput = { moving: boolean; store?: WorkspaceStoreView | null }

export type WorkspaceStoreRevokeInput = Record<string, never>

export type WorkspaceStoreRevokeOutput = { moving: boolean; store?: WorkspaceStoreView | null }

export type WorkspaceStoreSetInput = { accessKeyId: string; bucket: string; endpoint: string; moveExisting?: boolean; pathStyle?: boolean; prefix?: string; region: string; secretAccessKey: string }

export type WorkspaceStoreSetOutput = { moving: boolean; store?: WorkspaceStoreView | null }

export type WorkspaceTemplateApplyInput = { projectId: string; template: string }

export type WorkspaceTemplateApplyOutput = { changes: string[]; projectId?: string | null; template: string; templateName: string }

export type WorkspaceTemplateListInput = { projectId: string }

export type WorkspaceTemplateListOutput = { canUse: boolean; lastUsed?: TemplateUse | null; templates: WorkspaceTemplateView[] }

export type WorkspaceTemplateSaveInput = { description?: string | null; includeProject?: boolean | null; name?: string | null; projectId: string; remove?: string | null }

export type WorkspaceTemplateSaveOutput = { done: string; template?: WorkspaceTemplateView | null }

export type WorkspaceThemeSetInput = { offered: boolean; themeId: string }

export type WorkspaceThemeSetOutput = { darkBase: string; document: ThemeDocument; id: string; lightBase: string; mine: boolean; name: string; offered: boolean; ownerAccountId: string; updatedAt: string; version: number }

export type WorkspaceTransferGetInput = { transferId: string }

export type WorkspaceTransferGetOutput = { byteSize?: number | null; counts: unknown; createdAt: string; direction: string; downloadUrl?: string | null; error?: string | null; expiresAt?: string | null; finishedAt?: string | null; id: string; requestedByActorId: string; state: string }

export type WorkspaceTransferListInput = Record<string, never>

export type WorkspaceTransferListOutput = { transfers: TransferOutput[] }

export type WorkspaceUpdateInput = { expectedVersion?: number | null; inProjectOnly?: boolean; modules?: Record<string, boolean> | null; name?: string | null; preset?: string | null; projectId: string; slug?: string | null }

export type WorkspaceUpdateOutput = { modulesOff: string[]; name: string; renamed: boolean; slug: string; switched: Record<string, boolean>; version: number }

export type WorkspaceUsageGetInput = Record<string, never>

export type WorkspaceUsageGetOutput = { meters: MeterUsage[]; planId: string; planName: string; status: string }

export type Operations = {
  account_block_list: { input: AccountBlockListInput; output: AccountBlockListOutput }
  account_block_set: { input: AccountBlockSetInput; output: AccountBlockSetOutput }
  account_joinable_list: { input: AccountJoinableListInput; output: AccountJoinableListOutput }
  account_owner_take: { input: AccountOwnerTakeInput; output: AccountOwnerTakeOutput }
  account_workspace_add: { input: AccountWorkspaceAddInput; output: AccountWorkspaceAddOutput }
  activity_list: { input: ActivityListInput; output: ActivityListOutput }
  agent_action_get: { input: AgentActionGetInput; output: AgentActionGetOutput }
  agent_action_run: { input: AgentActionRunInput; output: AgentActionRunOutput }
  agent_create: { input: AgentCreateInput; output: AgentCreateOutput }
  agent_managers_set: { input: AgentManagersSetInput; output: AgentManagersSetOutput }
  agent_offer_list: { input: AgentOfferListInput; output: AgentOfferListOutput }
  agent_offer_start: { input: AgentOfferStartInput; output: AgentOfferStartOutput }
  agent_pause: { input: AgentPauseInput; output: AgentPauseOutput }
  agent_profile_list: { input: AgentProfileListInput; output: AgentProfileListOutput }
  agent_profile_set: { input: AgentProfileSetInput; output: AgentProfileSetOutput }
  agent_recipe_list: { input: AgentRecipeListInput; output: AgentRecipeListOutput }
  agent_ref_decide: { input: AgentRefDecideInput; output: AgentRefDecideOutput }
  agent_ref_remove: { input: AgentRefRemoveInput; output: AgentRefRemoveOutput }
  agent_ref_set: { input: AgentRefSetInput; output: AgentRefSetOutput }
  agent_ref_update: { input: AgentRefUpdateInput; output: AgentRefUpdateOutput }
  agent_register_decide: { input: AgentRegisterDecideInput; output: AgentRegisterDecideOutput }
  agent_resume: { input: AgentResumeInput; output: AgentResumeOutput }
  agent_scope_get: { input: AgentScopeGetInput; output: AgentScopeGetOutput }
  agent_template_list: { input: AgentTemplateListInput; output: AgentTemplateListOutput }
  agent_usage_list: { input: AgentUsageListInput; output: AgentUsageListOutput }
  app_consent_set: { input: AppConsentSetInput; output: AppConsentSetOutput }
  app_limit_set: { input: AppLimitSetInput; output: AppLimitSetOutput }
  automation_create: { input: AutomationCreateInput; output: AutomationCreateOutput }
  automation_delete: { input: AutomationDeleteInput; output: AutomationDeleteOutput }
  automation_history: { input: AutomationHistoryInput; output: AutomationHistoryOutput }
  automation_list: { input: AutomationListInput; output: AutomationListOutput }
  automation_run_start: { input: AutomationRunStartInput; output: AutomationRunStartOutput }
  automation_update: { input: AutomationUpdateInput; output: AutomationUpdateOutput }
  briefing_get: { input: BriefingGetInput; output: BriefingGetOutput }
  budget_list: { input: BudgetListInput; output: BudgetListOutput }
  budget_set: { input: BudgetSetInput; output: BudgetSetOutput }
  bug_report_link_create: { input: BugReportLinkCreateInput; output: BugReportLinkCreateOutput }
  bug_report_link_list: { input: BugReportLinkListInput; output: BugReportLinkListOutput }
  bug_report_link_update: { input: BugReportLinkUpdateInput; output: BugReportLinkUpdateOutput }
  button_get: { input: ButtonGetInput; output: ButtonGetOutput }
  button_press: { input: ButtonPressInput; output: ButtonPressOutput }
  change_list: { input: ChangeListInput; output: ChangeListOutput }
  change_undo: { input: ChangeUndoInput; output: ChangeUndoOutput }
  channel_create: { input: ChannelCreateInput; output: ChannelCreateOutput }
  channel_get: { input: ChannelGetInput; output: ChannelGetOutput }
  channel_join: { input: ChannelJoinInput; output: ChannelJoinOutput }
  channel_leave: { input: ChannelLeaveInput; output: ChannelLeaveOutput }
  channel_list: { input: ChannelListInput; output: ChannelListOutput }
  channel_update: { input: ChannelUpdateInput; output: ChannelUpdateOutput }
  comment_create: { input: CommentCreateInput; output: CommentCreateOutput }
  comment_list: { input: CommentListInput; output: CommentListOutput }
  comment_thread_update: { input: CommentThreadUpdateInput; output: CommentThreadUpdateOutput }
  connected_module_approve: { input: ConnectedModuleApproveInput; output: ConnectedModuleApproveOutput }
  connected_module_install: { input: ConnectedModuleInstallInput; output: ConnectedModuleInstallOutput }
  connected_module_list: { input: ConnectedModuleListInput; output: ConnectedModuleListOutput }
  connected_module_refresh: { input: ConnectedModuleRefreshInput; output: ConnectedModuleRefreshOutput }
  connected_module_revoke: { input: ConnectedModuleRevokeInput; output: ConnectedModuleRevokeOutput }
  connected_module_run: { input: ConnectedModuleRunInput; output: ConnectedModuleRunOutput }
  connected_module_test: { input: ConnectedModuleTestInput; output: ConnectedModuleTestOutput }
  connection_create: { input: ConnectionCreateInput; output: ConnectionCreateOutput }
  connection_delivery_list: { input: ConnectionDeliveryListInput; output: ConnectionDeliveryListOutput }
  connection_list: { input: ConnectionListInput; output: ConnectionListOutput }
  connection_redeliver: { input: ConnectionRedeliverInput; output: ConnectionRedeliverOutput }
  connection_remove: { input: ConnectionRemoveInput; output: ConnectionRemoveOutput }
  connection_secret_rotate: { input: ConnectionSecretRotateInput; output: ConnectionSecretRotateOutput }
  connection_test: { input: ConnectionTestInput; output: ConnectionTestOutput }
  connection_update: { input: ConnectionUpdateInput; output: ConnectionUpdateOutput }
  conversation_agent_settings_list: { input: ConversationAgentSettingsListInput; output: ConversationAgentSettingsListOutput }
  conversation_agent_settings_set: { input: ConversationAgentSettingsSetInput; output: ConversationAgentSettingsSetOutput }
  conversation_continue: { input: ConversationContinueInput; output: ConversationContinueOutput }
  conversation_file_list: { input: ConversationFileListInput; output: ConversationFileListOutput }
  conversation_mark_read: { input: ConversationMarkReadInput; output: ConversationMarkReadOutput }
  conversation_member_list: { input: ConversationMemberListInput; output: ConversationMemberListOutput }
  conversation_members_update: { input: ConversationMembersUpdateInput; output: ConversationMembersUpdateOutput }
  conversation_tab_list: { input: ConversationTabListInput; output: ConversationTabListOutput }
  conversation_tab_set: { input: ConversationTabSetInput; output: ConversationTabSetOutput }
  conversation_task_list: { input: ConversationTaskListInput; output: ConversationTaskListOutput }
  conversation_update: { input: ConversationUpdateInput; output: ConversationUpdateOutput }
  conversations_mark_read: { input: ConversationsMarkReadInput; output: ConversationsMarkReadOutput }
  credential_create: { input: CredentialCreateInput; output: CredentialCreateOutput }
  credential_list: { input: CredentialListInput; output: CredentialListOutput }
  credential_revoke: { input: CredentialRevokeInput; output: CredentialRevokeOutput }
  credential_rotate: { input: CredentialRotateInput; output: CredentialRotateOutput }
  custom_emoji_create: { input: CustomEmojiCreateInput; output: CustomEmojiCreateOutput }
  custom_emoji_list: { input: CustomEmojiListInput; output: CustomEmojiListOutput }
  custom_emoji_update: { input: CustomEmojiUpdateInput; output: CustomEmojiUpdateOutput }
  cycle_list: { input: CycleListInput; output: CycleListOutput }
  cycle_set: { input: CycleSetInput; output: CycleSetOutput }
  dashboard_create: { input: DashboardCreateInput; output: DashboardCreateOutput }
  dashboard_delete: { input: DashboardDeleteInput; output: DashboardDeleteOutput }
  dashboard_get: { input: DashboardGetInput; output: DashboardGetOutput }
  dashboard_list: { input: DashboardListInput; output: DashboardListOutput }
  dashboard_update: { input: DashboardUpdateInput; output: DashboardUpdateOutput }
  data_connection_create: { input: DataConnectionCreateInput; output: DataConnectionCreateOutput }
  data_connection_delete: { input: DataConnectionDeleteInput; output: DataConnectionDeleteOutput }
  data_connection_list: { input: DataConnectionListInput; output: DataConnectionListOutput }
  data_connection_update: { input: DataConnectionUpdateInput; output: DataConnectionUpdateOutput }
  data_query_create: { input: DataQueryCreateInput; output: DataQueryCreateOutput }
  data_query_delete: { input: DataQueryDeleteInput; output: DataQueryDeleteOutput }
  data_query_get: { input: DataQueryGetInput; output: DataQueryGetOutput }
  data_query_list: { input: DataQueryListInput; output: DataQueryListOutput }
  data_query_review: { input: DataQueryReviewInput; output: DataQueryReviewOutput }
  data_query_run: { input: DataQueryRunInput; output: DataQueryRunOutput }
  data_query_share: { input: DataQueryShareInput; output: DataQueryShareOutput }
  data_query_update: { input: DataQueryUpdateInput; output: DataQueryUpdateOutput }
  decision_list: { input: DecisionListInput; output: DecisionListOutput }
  decision_record: { input: DecisionRecordInput; output: DecisionRecordOutput }
  dependency_graph_get: { input: DependencyGraphGetInput; output: DependencyGraphGetOutput }
  directory_list: { input: DirectoryListInput; output: DirectoryListOutput }
  directory_source_create: { input: DirectorySourceCreateInput; output: DirectorySourceCreateOutput }
  directory_source_remove: { input: DirectorySourceRemoveInput; output: DirectorySourceRemoveOutput }
  dm_request_respond: { input: DmRequestRespondInput; output: DmRequestRespondOutput }
  dm_standing_get: { input: DmStandingGetInput; output: DmStandingGetOutput }
  environment_archive: { input: EnvironmentArchiveInput; output: EnvironmentArchiveOutput }
  environment_check: { input: EnvironmentCheckInput; output: EnvironmentCheckOutput }
  environment_create: { input: EnvironmentCreateInput; output: EnvironmentCreateOutput }
  environment_list: { input: EnvironmentListInput; output: EnvironmentListOutput }
  environment_update: { input: EnvironmentUpdateInput; output: EnvironmentUpdateOutput }
  event_kind_list: { input: EventKindListInput; output: EventKindListOutput }
  event_list: { input: EventListInput; output: EventListOutput }
  field_create: { input: FieldCreateInput; output: FieldCreateOutput }
  field_list: { input: FieldListInput; output: FieldListOutput }
  field_update: { input: FieldUpdateInput; output: FieldUpdateOutput }
  file_link_get: { input: FileLinkGetInput; output: FileLinkGetOutput }
  file_source_get: { input: FileSourceGetInput; output: FileSourceGetOutput }
  file_source_list: { input: FileSourceListInput; output: FileSourceListOutput }
  file_upload_finish: { input: FileUploadFinishInput; output: FileUploadFinishOutput }
  file_upload_start: { input: FileUploadStartInput; output: FileUploadStartOutput }
  follow_list: { input: FollowListInput; output: FollowListOutput }
  follow_set: { input: FollowSetInput; output: FollowSetOutput }
  form_answer_send: { input: FormAnswerSendInput; output: FormAnswerSendOutput }
  form_create: { input: FormCreateInput; output: FormCreateOutput }
  form_get: { input: FormGetInput; output: FormGetOutput }
  form_list: { input: FormListInput; output: FormListOutput }
  form_update: { input: FormUpdateInput; output: FormUpdateOutput }
  formula_preview: { input: FormulaPreviewInput; output: FormulaPreviewOutput }
  github_automation_update: { input: GithubAutomationUpdateInput; output: GithubAutomationUpdateOutput }
  github_installation_link: { input: GithubInstallationLinkInput; output: GithubInstallationLinkOutput }
  group_archive: { input: GroupArchiveInput; output: GroupArchiveOutput }
  group_create: { input: GroupCreateInput; output: GroupCreateOutput }
  group_list: { input: GroupListInput; output: GroupListOutput }
  group_members_set: { input: GroupMembersSetInput; output: GroupMembersSetOutput }
  group_restore: { input: GroupRestoreInput; output: GroupRestoreOutput }
  group_update: { input: GroupUpdateInput; output: GroupUpdateOutput }
  guard_list: { input: GuardListInput; output: GuardListOutput }
  guard_set: { input: GuardSetInput; output: GuardSetOutput }
  history_list: { input: HistoryListInput; output: HistoryListOutput }
  history_restore: { input: HistoryRestoreInput; output: HistoryRestoreOutput }
  history_show: { input: HistoryShowInput; output: HistoryShowOutput }
  host_agent_bind: { input: HostAgentBindInput; output: HostAgentBindOutput }
  host_agent_unbind: { input: HostAgentUnbindInput; output: HostAgentUnbindOutput }
  host_enrolment_create: { input: HostEnrolmentCreateInput; output: HostEnrolmentCreateOutput }
  host_link: { input: HostLinkInput; output: HostLinkOutput }
  host_list: { input: HostListInput; output: HostListOutput }
  host_unlink: { input: HostUnlinkInput; output: HostUnlinkOutput }
  inbox_view_list: { input: InboxViewListInput; output: InboxViewListOutput }
  inbox_view_set: { input: InboxViewSetInput; output: InboxViewSetOutput }
  incoming_hook_create: { input: IncomingHookCreateInput; output: IncomingHookCreateOutput }
  incoming_hook_delete: { input: IncomingHookDeleteInput; output: IncomingHookDeleteOutput }
  incoming_hook_list: { input: IncomingHookListInput; output: IncomingHookListOutput }
  initiative_create: { input: InitiativeCreateInput; output: InitiativeCreateOutput }
  initiative_list: { input: InitiativeListInput; output: InitiativeListOutput }
  invitation_create: { input: InvitationCreateInput; output: InvitationCreateOutput }
  invitation_list: { input: InvitationListInput; output: InvitationListOutput }
  invitation_update: { input: InvitationUpdateInput; output: InvitationUpdateOutput }
  invite_link_create: { input: InviteLinkCreateInput; output: InviteLinkCreateOutput }
  invite_link_list: { input: InviteLinkListInput; output: InviteLinkListOutput }
  invite_link_revoke: { input: InviteLinkRevokeInput; output: InviteLinkRevokeOutput }
  job_get: { input: JobGetInput; output: JobGetOutput }
  label_create: { input: LabelCreateInput; output: LabelCreateOutput }
  label_list: { input: LabelListInput; output: LabelListOutput }
  label_update: { input: LabelUpdateInput; output: LabelUpdateOutput }
  later_list: { input: LaterListInput; output: LaterListOutput }
  later_save: { input: LaterSaveInput; output: LaterSaveOutput }
  later_update: { input: LaterUpdateInput; output: LaterUpdateOutput }
  launch_cancel: { input: LaunchCancelInput; output: LaunchCancelOutput }
  launch_list: { input: LaunchListInput; output: LaunchListOutput }
  layout_preset_create: { input: LayoutPresetCreateInput; output: LayoutPresetCreateOutput }
  layout_preset_update: { input: LayoutPresetUpdateInput; output: LayoutPresetUpdateOutput }
  layout_update: { input: LayoutUpdateInput; output: LayoutUpdateOutput }
  memory_page_open: { input: MemoryPageOpenInput; output: MemoryPageOpenOutput }
  memory_page_set: { input: MemoryPageSetInput; output: MemoryPageSetOutput }
  message_delete: { input: MessageDeleteInput; output: MessageDeleteOutput }
  message_draft_list: { input: MessageDraftListInput; output: MessageDraftListOutput }
  message_draft_set: { input: MessageDraftSetInput; output: MessageDraftSetOutput }
  message_forward: { input: MessageForwardInput; output: MessageForwardOutput }
  message_history: { input: MessageHistoryInput; output: MessageHistoryOutput }
  message_preview_hide: { input: MessagePreviewHideInput; output: MessagePreviewHideOutput }
  message_react: { input: MessageReactInput; output: MessageReactOutput }
  message_schedule: { input: MessageScheduleInput; output: MessageScheduleOutput }
  message_send: { input: MessageSendInput; output: MessageSendOutput }
  message_update: { input: MessageUpdateInput; output: MessageUpdateOutput }
  milestone_create: { input: MilestoneCreateInput; output: MilestoneCreateOutput }
  milestone_list: { input: MilestoneListInput; output: MilestoneListOutput }
  milestone_reorder: { input: MilestoneReorderInput; output: MilestoneReorderOutput }
  milestone_update: { input: MilestoneUpdateInput; output: MilestoneUpdateOutput }
  model_catalog_set: { input: ModelCatalogSetInput; output: ModelCatalogSetOutput }
  module_archive: { input: ModuleArchiveInput; output: ModuleArchiveOutput }
  module_create: { input: ModuleCreateInput; output: ModuleCreateOutput }
  module_frame_check: { input: ModuleFrameCheckInput; output: ModuleFrameCheckOutput }
  module_list: { input: ModuleListInput; output: ModuleListOutput }
  module_open: { input: ModuleOpenInput; output: ModuleOpenOutput }
  module_panel_list: { input: ModulePanelListInput; output: ModulePanelListOutput }
  module_panel_open: { input: ModulePanelOpenInput; output: ModulePanelOpenOutput }
  module_panel_press: { input: ModulePanelPressInput; output: ModulePanelPressOutput }
  module_part_link: { input: ModulePartLinkInput; output: ModulePartLinkOutput }
  module_part_unlink: { input: ModulePartUnlinkInput; output: ModulePartUnlinkOutput }
  module_restore: { input: ModuleRestoreInput; output: ModuleRestoreOutput }
  module_set: { input: ModuleSetInput; output: ModuleSetOutput }
  module_update: { input: ModuleUpdateInput; output: ModuleUpdateOutput }
  my_work_get: { input: MyWorkGetInput; output: MyWorkGetOutput }
  notification_keyword_list: { input: NotificationKeywordListInput; output: NotificationKeywordListOutput }
  notification_keyword_set: { input: NotificationKeywordSetInput; output: NotificationKeywordSetOutput }
  notification_list: { input: NotificationListInput; output: NotificationListOutput }
  notification_update: { input: NotificationUpdateInput; output: NotificationUpdateOutput }
  package_export: { input: PackageExportInput; output: PackageExportOutput }
  package_install: { input: PackageInstallInput; output: PackageInstallOutput }
  package_list: { input: PackageListInput; output: PackageListOutput }
  package_preview: { input: PackagePreviewInput; output: PackagePreviewOutput }
  package_remove: { input: PackageRemoveInput; output: PackageRemoveOutput }
  package_upgrade: { input: PackageUpgradeInput; output: PackageUpgradeOutput }
  package_upgrade_preview: { input: PackageUpgradePreviewInput; output: PackageUpgradePreviewOutput }
  page_blocks_get: { input: PageBlocksGetInput; output: PageBlocksGetOutput }
  page_comment_create: { input: PageCommentCreateInput; output: PageCommentCreateOutput }
  page_comment_list: { input: PageCommentListInput; output: PageCommentListOutput }
  page_comment_update: { input: PageCommentUpdateInput; output: PageCommentUpdateOutput }
  page_create: { input: PageCreateInput; output: PageCreateOutput }
  page_get: { input: PageGetInput; output: PageGetOutput }
  page_list: { input: PageListInput; output: PageListOutput }
  page_make: { input: PageMakeInput; output: PageMakeOutput }
  page_suggestion_decide: { input: PageSuggestionDecideInput; output: PageSuggestionDecideOutput }
  page_suggestion_list: { input: PageSuggestionListInput; output: PageSuggestionListOutput }
  page_template_list: { input: PageTemplateListInput; output: PageTemplateListOutput }
  page_update: { input: PageUpdateInput; output: PageUpdateOutput }
  page_update_blocks: { input: PageUpdateBlocksInput; output: PageUpdateBlocksOutput }
  people_search: { input: PeopleSearchInput; output: PeopleSearchOutput }
  permissions_get: { input: PermissionsGetInput; output: PermissionsGetOutput }
  person_create: { input: PersonCreateInput; output: PersonCreateOutput }
  pin_set: { input: PinSetInput; output: PinSetOutput }
  place_search: { input: PlaceSearchInput; output: PlaceSearchOutput }
  plan_import: { input: PlanImportInput; output: PlanImportOutput }
  profile_avatar_list: { input: ProfileAvatarListInput; output: ProfileAvatarListOutput }
  profile_get: { input: ProfileGetInput; output: ProfileGetOutput }
  profile_update: { input: ProfileUpdateInput; output: ProfileUpdateOutput }
  project_get: { input: ProjectGetInput; output: ProjectGetOutput }
  project_list: { input: ProjectListInput; output: ProjectListOutput }
  project_status_get: { input: ProjectStatusGetInput; output: ProjectStatusGetOutput }
  project_status_set: { input: ProjectStatusSetInput; output: ProjectStatusSetOutput }
  project_template_create: { input: ProjectTemplateCreateInput; output: ProjectTemplateCreateOutput }
  project_template_list: { input: ProjectTemplateListInput; output: ProjectTemplateListOutput }
  project_template_update: { input: ProjectTemplateUpdateInput; output: ProjectTemplateUpdateOutput }
  project_update_create: { input: ProjectUpdateCreateInput; output: ProjectUpdateCreateOutput }
  proposal_decide: { input: ProposalDecideInput; output: ProposalDecideOutput }
  proposal_list: { input: ProposalListInput; output: ProposalListOutput }
  pull_request_create: { input: PullRequestCreateInput; output: PullRequestCreateOutput }
  pull_request_get: { input: PullRequestGetInput; output: PullRequestGetOutput }
  pull_request_list: { input: PullRequestListInput; output: PullRequestListOutput }
  pull_request_merge: { input: PullRequestMergeInput; output: PullRequestMergeOutput }
  pull_request_refresh: { input: PullRequestRefreshInput; output: PullRequestRefreshOutput }
  record_create: { input: RecordCreateInput; output: RecordCreateOutput }
  record_template_create: { input: RecordTemplateCreateInput; output: RecordTemplateCreateOutput }
  record_template_list: { input: RecordTemplateListInput; output: RecordTemplateListOutput }
  record_template_update: { input: RecordTemplateUpdateInput; output: RecordTemplateUpdateOutput }
  record_update: { input: RecordUpdateInput; output: RecordUpdateOutput }
  recurring_create: { input: RecurringCreateInput; output: RecurringCreateOutput }
  recurring_record_create: { input: RecurringRecordCreateInput; output: RecurringRecordCreateOutput }
  recurring_record_delete: { input: RecurringRecordDeleteInput; output: RecurringRecordDeleteOutput }
  recurring_record_list: { input: RecurringRecordListInput; output: RecurringRecordListOutput }
  recurring_record_update: { input: RecurringRecordUpdateInput; output: RecurringRecordUpdateOutput }
  recurring_update: { input: RecurringUpdateInput; output: RecurringUpdateOutput }
  reply_claim: { input: ReplyClaimInput; output: ReplyClaimOutput }
  reply_pass: { input: ReplyPassInput; output: ReplyPassOutput }
  reply_progress_report: { input: ReplyProgressReportInput; output: ReplyProgressReportOutput }
  reply_release: { input: ReplyReleaseInput; output: ReplyReleaseOutput }
  reply_retry: { input: ReplyRetryInput; output: ReplyRetryOutput }
  reply_send: { input: ReplySendInput; output: ReplySendOutput }
  reply_stop: { input: ReplyStopInput; output: ReplyStopOutput }
  repository_archive: { input: RepositoryArchiveInput; output: RepositoryArchiveOutput }
  repository_create: { input: RepositoryCreateInput; output: RepositoryCreateOutput }
  repository_get: { input: RepositoryGetInput; output: RepositoryGetOutput }
  repository_list: { input: RepositoryListInput; output: RepositoryListOutput }
  repository_prepare: { input: RepositoryPrepareInput; output: RepositoryPrepareOutput }
  request_cancel: { input: RequestCancelInput; output: RequestCancelOutput }
  request_create: { input: RequestCreateInput; output: RequestCreateOutput }
  request_get: { input: RequestGetInput; output: RequestGetOutput }
  request_list: { input: RequestListInput; output: RequestListOutput }
  request_respond: { input: RequestRespondInput; output: RequestRespondOutput }
  role_assign: { input: RoleAssignInput; output: RoleAssignOutput }
  role_create: { input: RoleCreateInput; output: RoleCreateOutput }
  role_delete: { input: RoleDeleteInput; output: RoleDeleteOutput }
  role_get: { input: RoleGetInput; output: RoleGetOutput }
  role_list: { input: RoleListInput; output: RoleListOutput }
  role_update: { input: RoleUpdateInput; output: RoleUpdateOutput }
  run_activity_list: { input: RunActivityListInput; output: RunActivityListOutput }
  run_activity_report: { input: RunActivityReportInput; output: RunActivityReportOutput }
  run_cancel: { input: RunCancelInput; output: RunCancelOutput }
  run_create: { input: RunCreateInput; output: RunCreateOutput }
  run_detail_get: { input: RunDetailGetInput; output: RunDetailGetOutput }
  run_failure_hide: { input: RunFailureHideInput; output: RunFailureHideOutput }
  run_get: { input: RunGetInput; output: RunGetOutput }
  run_list: { input: RunListInput; output: RunListOutput }
  run_mark_not_started: { input: RunMarkNotStartedInput; output: RunMarkNotStartedOutput }
  run_publish: { input: RunPublishInput; output: RunPublishOutput }
  run_result_report: { input: RunResultReportInput; output: RunResultReportOutput }
  run_retry: { input: RunRetryInput; output: RunRetryOutput }
  run_start: { input: RunStartInput; output: RunStartOutput }
  run_step_list: { input: RunStepListInput; output: RunStepListOutput }
  run_stop: { input: RunStopInput; output: RunStopOutput }
  run_update: { input: RunUpdateInput; output: RunUpdateOutput }
  run_usage_list: { input: RunUsageListInput; output: RunUsageListOutput }
  run_usage_report: { input: RunUsageReportInput; output: RunUsageReportOutput }
  saved_search_list: { input: SavedSearchListInput; output: SavedSearchListOutput }
  saved_search_set: { input: SavedSearchSetInput; output: SavedSearchSetOutput }
  schedule_create: { input: ScheduleCreateInput; output: ScheduleCreateOutput }
  schedule_list: { input: ScheduleListInput; output: ScheduleListOutput }
  schedule_update: { input: ScheduleUpdateInput; output: ScheduleUpdateOutput }
  scheduled_message_update: { input: ScheduledMessageUpdateInput; output: ScheduledMessageUpdateOutput }
  setting_list: { input: SettingListInput; output: SettingListOutput }
  setting_update: { input: SettingUpdateInput; output: SettingUpdateOutput }
  sheet_range_set: { input: SheetRangeSetInput; output: SheetRangeSetOutput }
  sheet_search: { input: SheetSearchInput; output: SheetSearchOutput }
  sheet_view_create: { input: SheetViewCreateInput; output: SheetViewCreateOutput }
  sheet_view_delete: { input: SheetViewDeleteInput; output: SheetViewDeleteOutput }
  sheet_view_list: { input: SheetViewListInput; output: SheetViewListOutput }
  sheet_view_update: { input: SheetViewUpdateInput; output: SheetViewUpdateOutput }
  sidebar_item_move: { input: SidebarItemMoveInput; output: SidebarItemMoveOutput }
  sidebar_list: { input: SidebarListInput; output: SidebarListOutput }
  sidebar_section_set: { input: SidebarSectionSetInput; output: SidebarSectionSetOutput }
  skill_get: { input: SkillGetInput; output: SkillGetOutput }
  skill_list: { input: SkillListInput; output: SkillListOutput }
  space_archive: { input: SpaceArchiveInput; output: SpaceArchiveOutput }
  space_create: { input: SpaceCreateInput; output: SpaceCreateOutput }
  space_group_add: { input: SpaceGroupAddInput; output: SpaceGroupAddOutput }
  space_group_remove: { input: SpaceGroupRemoveInput; output: SpaceGroupRemoveOutput }
  space_join: { input: SpaceJoinInput; output: SpaceJoinOutput }
  space_leave: { input: SpaceLeaveInput; output: SpaceLeaveOutput }
  space_list: { input: SpaceListInput; output: SpaceListOutput }
  space_member_add: { input: SpaceMemberAddInput; output: SpaceMemberAddOutput }
  space_member_remove: { input: SpaceMemberRemoveInput; output: SpaceMemberRemoveOutput }
  space_member_update: { input: SpaceMemberUpdateInput; output: SpaceMemberUpdateOutput }
  space_open: { input: SpaceOpenInput; output: SpaceOpenOutput }
  space_personal_open: { input: SpacePersonalOpenInput; output: SpacePersonalOpenOutput }
  space_restore: { input: SpaceRestoreInput; output: SpaceRestoreOutput }
  space_update: { input: SpaceUpdateInput; output: SpaceUpdateOutput }
  status_delete: { input: StatusDeleteInput; output: StatusDeleteOutput }
  status_list: { input: StatusListInput; output: StatusListOutput }
  status_set: { input: StatusSetInput; output: StatusSetOutput }
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
  task_launch: { input: TaskLaunchInput; output: TaskLaunchOutput }
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
  theme_create: { input: ThemeCreateInput; output: ThemeCreateOutput }
  theme_delete: { input: ThemeDeleteInput; output: ThemeDeleteOutput }
  theme_get: { input: ThemeGetInput; output: ThemeGetOutput }
  theme_list: { input: ThemeListInput; output: ThemeListOutput }
  theme_update: { input: ThemeUpdateInput; output: ThemeUpdateOutput }
  thing_link: { input: ThingLinkInput; output: ThingLinkOutput }
  thing_link_create: { input: ThingLinkCreateInput; output: ThingLinkCreateOutput }
  thing_share_add: { input: ThingShareAddInput; output: ThingShareAddOutput }
  thing_share_remove: { input: ThingShareRemoveInput; output: ThingShareRemoveOutput }
  thing_unlink: { input: ThingUnlinkInput; output: ThingUnlinkOutput }
  thing_upsert: { input: ThingUpsertInput; output: ThingUpsertOutput }
  thread_followed_list: { input: ThreadFollowedListInput; output: ThreadFollowedListOutput }
  thread_start: { input: ThreadStartInput; output: ThreadStartOutput }
  thread_update: { input: ThreadUpdateInput; output: ThreadUpdateOutput }
  triage_decide: { input: TriageDecideInput; output: TriageDecideOutput }
  triage_enable: { input: TriageEnableInput; output: TriageEnableOutput }
  triage_rule_list: { input: TriageRuleListInput; output: TriageRuleListOutput }
  triage_rule_set: { input: TriageRuleSetInput; output: TriageRuleSetOutput }
  usage_batch_launch: { input: UsageBatchLaunchInput; output: UsageBatchLaunchOutput }
  usage_batch_preview: { input: UsageBatchPreviewInput; output: UsageBatchPreviewOutput }
  usage_window_list: { input: UsageWindowListInput; output: UsageWindowListOutput }
  view_create: { input: ViewCreateInput; output: ViewCreateOutput }
  view_default_set: { input: ViewDefaultSetInput; output: ViewDefaultSetOutput }
  view_delete: { input: ViewDeleteInput; output: ViewDeleteOutput }
  view_list: { input: ViewListInput; output: ViewListOutput }
  view_pin: { input: ViewPinInput; output: ViewPinOutput }
  view_update: { input: ViewUpdateInput; output: ViewUpdateOutput }
  whiteboard_create: { input: WhiteboardCreateInput; output: WhiteboardCreateOutput }
  whiteboard_duplicate: { input: WhiteboardDuplicateInput; output: WhiteboardDuplicateOutput }
  whiteboard_link: { input: WhiteboardLinkInput; output: WhiteboardLinkOutput }
  whiteboard_propose: { input: WhiteboardProposeInput; output: WhiteboardProposeOutput }
  whiteboard_template_list: { input: WhiteboardTemplateListInput; output: WhiteboardTemplateListOutput }
  whiteboard_update: { input: WhiteboardUpdateInput; output: WhiteboardUpdateOutput }
  whiteboard_update_elements: { input: WhiteboardUpdateElementsInput; output: WhiteboardUpdateElementsOutput }
  workflow_get: { input: WorkflowGetInput; output: WorkflowGetOutput }
  workflow_update: { input: WorkflowUpdateInput; output: WorkflowUpdateOutput }
  workspace_delete: { input: WorkspaceDeleteInput; output: WorkspaceDeleteOutput }
  workspace_delete_cancel: { input: WorkspaceDeleteCancelInput; output: WorkspaceDeleteCancelOutput }
  workspace_export_start: { input: WorkspaceExportStartInput; output: WorkspaceExportStartOutput }
  workspace_member_update: { input: WorkspaceMemberUpdateInput; output: WorkspaceMemberUpdateOutput }
  workspace_search: { input: WorkspaceSearchInput; output: WorkspaceSearchOutput }
  workspace_shape_set: { input: WorkspaceShapeSetInput; output: WorkspaceShapeSetOutput }
  workspace_store_get: { input: WorkspaceStoreGetInput; output: WorkspaceStoreGetOutput }
  workspace_store_revoke: { input: WorkspaceStoreRevokeInput; output: WorkspaceStoreRevokeOutput }
  workspace_store_set: { input: WorkspaceStoreSetInput; output: WorkspaceStoreSetOutput }
  workspace_template_apply: { input: WorkspaceTemplateApplyInput; output: WorkspaceTemplateApplyOutput }
  workspace_template_list: { input: WorkspaceTemplateListInput; output: WorkspaceTemplateListOutput }
  workspace_template_save: { input: WorkspaceTemplateSaveInput; output: WorkspaceTemplateSaveOutput }
  workspace_theme_set: { input: WorkspaceThemeSetInput; output: WorkspaceThemeSetOutput }
  workspace_transfer_get: { input: WorkspaceTransferGetInput; output: WorkspaceTransferGetOutput }
  workspace_transfer_list: { input: WorkspaceTransferListInput; output: WorkspaceTransferListOutput }
  workspace_update: { input: WorkspaceUpdateInput; output: WorkspaceUpdateOutput }
  workspace_usage_get: { input: WorkspaceUsageGetInput; output: WorkspaceUsageGetOutput }
}

export type OpName = keyof Operations
