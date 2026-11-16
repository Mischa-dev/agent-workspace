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

export type AgentTemplateView = { definition: unknown; id: string; installId?: string | null; key: string; name: string; purpose: string; spaceId?: string | null }

export type ArchiveParts = "archive" | "keep"

export type ColumnWrite = { field: string; hidden?: boolean; width?: number | null }

export type Content = { body?: Record<string, unknown> | null; key: string; kind: ContentKind; path?: string | null }

export type ContentKind = "switches" | "layout" | "role" | "space" | "channels" | "module" | "collection" | "form" | "automation" | "page" | "skill" | "agent"

export type CriterionOp = "add" | "edit" | "check" | "uncheck" | "remove" | "move"

export type CriterionOperationArgument = { criterion?: string | null; op: CriterionOp; position?: number | null; text?: string | null }

export type CriterionOperationOutcome = { changed: boolean; criterionId: string; op: CriterionOp }

export type CriterionValue = { completed: boolean; completedAt?: string | null; completedBy?: string | null; id: string; position: number; text: string }

export type DateBucket = "day" | "week" | "month"

export type DecisionView = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

export type Direction = "asc" | "desc"

export type EditOutcome = { changed: string[]; results: OperationResult[]; savedRevision?: number | null; sceneSeq: number }

export type FieldOp = "is" | "isNot" | "empty" | "notEmpty" | "contains" | "gt" | "gte" | "lt" | "lte" | "within" | "categoryIs" | "categoryIsNot"

export type FieldWrite = { archived?: boolean | null; description?: string | null; field?: string | null; name?: string | null; options?: OptionInput[] | null; position?: number | null; target?: string | null; type?: string | null }

export type FilterWrite = { field: string; op: FieldOp; values?: string[] }

export type HostAgentView = { accountId: string; actions: unknown[]; backends: unknown[]; hostAgentRef: string; name?: string | null; status: string }

export type HostOwnerChoice = "person" | "workspace"

export type HostView = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type InlineAutomation = { collection?: string | null; rule: unknown }

export type InlineCollection = { description?: string | null; fields?: FieldWrite[]; name: string; titleName?: string | null; views?: ViewWrite[] }

export type InlineForm = { collection: string; fields?: InlineFormField[]; intro?: string; name: string; thanks?: string }

export type InlineFormField = { field: string; required?: boolean }

export type InstallTarget = { newSpace?: NewSpace | null; spaceId?: string | null }

export type InstallView = { id: string; installedAt: string; name: string; objects: number; packageId: string; spaceId?: string | null; state: string; summary: string[]; version: string }

export type InstalledObject = { action: string; id: string; key: string; kind: string }

export type LinkArgument = { targetId: string; targetKind: string }

export type LinkView = { key?: string | null; projectId: string; projectName: string; targetId: string; targetKind: string; title: string; url: string }

export type MessageContent = { nodes: MessageNode[]; version: number }

export type MessageNode = { text: string; type: "text" } | { actorId: string; type: "mention" } | { type: "work_item_ref"; workItemId: string }

export type NavEntry = { icon?: string | null; label: string; target: unknown }

export type NavView = { icon?: string | null; id: string; label: string; target: unknown }

export type NewSpace = { key: string; name: string }

export type Operation = { arrowheads?: string | null; axis?: string | null; backgroundColor?: string | null; dataBase64?: string | null; direction?: string | null; dx?: number | null; dy?: number | null; edge?: string | null; fileId?: string | null; fillStyle?: string | null; fontSize?: number | null; frameId?: string | null; from?: string | null; gap?: number | null; height?: number | null; id?: string | null; ids?: string[] | null; link?: string | null; locked?: boolean | null; mimeType?: string | null; name?: string | null; op: string; opacity?: number | null; points?: number[][] | null; position?: string | null; roughness?: number | null; rounded?: boolean | null; strokeColor?: string | null; strokeStyle?: string | null; strokeWidth?: number | null; text?: string | null; textAlign?: string | null; to?: string | null; type?: string | null; width?: number | null; x?: number | null; y?: number | null }

export type OperationResult = { ids: string[]; op: string }

export type OptionInput = { category?: string | null; color?: string | null; id?: string | null; name: string }

export type OwnerView = { id: string; kind: string; name: string }

export type Package = { contents: Content[]; description?: string; digest?: string | null; format: string; id: string; name: string; needsAdmin?: string[]; publisher?: Publisher | null; requires?: Requires; target: PackageTarget; version: string }

export type PackageTarget = "space" | "workspace"

export type PartRef = { id: string; kind: string }

export type PartView = { id: string; kind: string; name?: string | null }

export type PlanItem = { action: string; existingId?: string | null; key: string; kind: string; name: string; needsAdmin: boolean; reason?: string | null }

export type Publisher = { name: string; url?: string | null }

export type RaisedRequestOutput = { activityId: string; clientRequestId?: string | null; requestId: string }

export type ReactionSummary = { byYou: boolean; count: number; emoji: string; name: string; people: string[] }

export type RemoveParts = "archive" | "keep"

export type Requires = { contract?: string | null; modules?: string[] }

export type RevisionItem = { redacted?: boolean; revision: number; text: string; truncated?: boolean; via?: string | null; writtenAt: string; writtenBy: string }

export type RunActivityInput = { at?: string | null; id: string; kind: string; parentRef?: string | null; payload?: unknown; stepRef?: string | null; summary?: string | null }

export type RunCauseInput = { conversationId?: string | null; kind: string; messageId?: string | null; ref?: string | null; taskId?: string | null }

export type RunRecord = { activityCount: number; cancelRequestedAt?: string | null; clientRunId?: string | null; externalUrl?: string | null; finishedAt?: string | null; liveness: string; reportIntervalSeconds: number; runId: string; state: string; statusLine?: string | null; title: string; version: number }

export type RunResultInput = { branch?: string | null; id?: string | null; kind: string; number?: number | null; thingKind?: string | null; title?: string | null; url?: string | null }

export type ScheduledLine = { conversationId: string; failure?: string | null; id: string; place: string; sendAt: string; status: string; text: string; url: string }

export type SortWrite = { direction?: Direction; field: string }

export type ThreadRef = { id: string; name: string; status: string }

export type ViewWrite = { columns?: ColumnWrite[] | null; dateField?: string | null; filters?: FilterWrite[] | null; groupBucket?: DateBucket | null; groupBy?: string | null; name?: string | null; position?: number | null; remove?: boolean | null; sort?: SortWrite[] | null; titleWidth?: number | null; type?: string | null; view?: string | null }

export type WhiteboardLine = { createdAt: string; createdBy?: string | null; deleted: boolean; elementCount: number; id: string; linkCount: number; organizationId: string; organizationName: string; projectId?: string | null; projectName?: string | null; sceneSeq: number; sceneUpdatedAt: string; title: string; updatedAt: string; updatedBy?: string | null; updatedByKind?: string | null; url: string; version: number }

export type AgentManagersSetInput = { add?: string[]; agent: string; remove?: string[] }

export type AgentManagersSetOutput = { agent: string; managers: string[] }

export type AgentTemplateListInput = { spaceId?: string | null }

export type AgentTemplateListOutput = { templates: AgentTemplateView[] }

export type ConversationMarkReadInput = { conversationId: string; fromMessageId?: string | null; unread?: boolean | null; upToSequence?: number | null }

export type ConversationMarkReadOutput = { conversationId: string; lastReadSequence: number; unreadCount: number }

export type DecisionListInput = { includeSuperseded?: boolean; limit?: number | null; spaceId: string }

export type DecisionListOutput = { decisions: DecisionView[] }

export type DecisionRecordInput = { detail?: string | null; pin?: boolean; sourceConversationId?: string | null; sourceMessageId?: string | null; spaceId?: string | null; supersedes?: string | null; title: string }

export type DecisionRecordOutput = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

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

export type PackageExportInput = { includePages?: boolean; includeSkills?: boolean; module?: string | null; spaceId?: string | null; wholeWorkspace?: boolean }

export type PackageExportOutput = { digest: string; package: Package }

export type PackageInstallInput = { package: Record<string, unknown>; planHash: string; target?: InstallTarget }

export type PackageInstallOutput = { changes: string[]; installId: string; objects: InstalledObject[]; packageId: string; skipped: string[]; spaceId?: string | null; version: string }

export type PackageListInput = { spaceId?: string | null }

export type PackageListOutput = { installs: InstallView[] }

export type PackagePreviewInput = { package: Record<string, unknown>; target?: InstallTarget }

export type PackagePreviewOutput = { digest: string; items: PlanItem[]; leaves: string[]; name: string; packageId: string; planHash: string; roles: unknown; target: unknown; turnsOff: string[]; turnsOn: string[]; version: string }

export type PackageRemoveInput = { installId: string; parts: RemoveParts }

export type PackageRemoveOutput = { id: string; installedAt: string; name: string; objects: number; packageId: string; spaceId?: string | null; state: string; summary: string[]; version: string }

export type RunActivityReportInput = { activities: RunActivityInput[]; runId: string }

export type RunActivityReportOutput = { accepted: number; duplicates: number; lastSeq: number; overLimit?: number; requests?: RaisedRequestOutput[]; state: string; version: number }

export type RunCancelInput = { reason?: string | null; runId: string }

export type RunCancelOutput = { idempotentReplay?: boolean; run: RunRecord }

export type RunCreateInput = { cause: RunCauseInput; clientRunId?: string | null; parentRunId?: string | null; reportIntervalSeconds?: number | null; settings?: unknown; spaceId: string; state?: string | null; title: string }

export type RunCreateOutput = { created: boolean; run: RunRecord }

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

export type TaskCommentCreateInput = { attachmentIds?: string[] | null; replyToCommentId?: string | null; taskId: string; text: string }

export type TaskCommentCreateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskCommentDeleteInput = { commentId: string; taskId: string }

export type TaskCommentDeleteOutput = { alreadyDeleted: boolean; commentId: string; note: string; taskId: string }

export type TaskCommentUpdateInput = { commentId: string; expectedRevision?: number | null; taskId: string; text: string }

export type TaskCommentUpdateOutput = { changed: boolean; commentId: string; revision: number; taskId: string }

export type TaskCreateInput = { acceptanceCriteria?: string[] | null; afterTaskId?: string | null; assignee?: string | null; beforeTaskId?: string | null; collection?: string | null; description?: string | null; dueDate?: string | null; estimate?: number | null; fields?: Record<string, unknown> | null; labels?: string[] | null; milestone?: string | null; parentId?: string | null; position?: string | null; priority?: string | null; projectId?: string | null; startDate?: string | null; status?: string | null; title: string; type?: string | null }

export type TaskCreateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskCriteriaUpdateInput = { expectedVersion: number; operations: CriterionOperationArgument[]; taskId: string }

export type TaskCriteriaUpdateOutput = { completed: number; criteria: CriterionValue[]; idempotentReplay?: boolean; key: string; operations: CriterionOperationOutcome[]; taskId: string; total: number; url: string; version: number }

export type TaskRestoreInput = { expectedVersion: number; taskId: string }

export type TaskRestoreOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

export type TaskUpdateInput = { addLabels?: string[] | null; afterTaskId?: string | null; archived?: boolean | null; assignee?: string | null; beforeTaskId?: string | null; blockedReason?: string | null; clearAssignee?: boolean | null; clearEstimate?: boolean | null; clearParent?: boolean | null; description?: string | null; dueDate?: string | null; estimate?: number | null; expectedVersion: number; fields?: Record<string, unknown> | null; milestone?: string | null; parentId?: string | null; position?: string | null; priority?: string | null; readyForReview?: boolean | null; removeLabels?: string[] | null; startDate?: string | null; status?: string | null; taskId: string; title?: string | null; type?: string | null }

export type TaskUpdateOutput = { activityId?: string | null; changedFields: string[]; idempotentReplay: boolean; key?: string | null; labels?: string[] | null; message?: string | null; resultId: string; url?: string | null; version: number; workItemId: string }

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

export type Operations = {
  agent_managers_set: { input: AgentManagersSetInput; output: AgentManagersSetOutput }
  agent_template_list: { input: AgentTemplateListInput; output: AgentTemplateListOutput }
  conversation_mark_read: { input: ConversationMarkReadInput; output: ConversationMarkReadOutput }
  decision_list: { input: DecisionListInput; output: DecisionListOutput }
  decision_record: { input: DecisionRecordInput; output: DecisionRecordOutput }
  host_agent_bind: { input: HostAgentBindInput; output: HostAgentBindOutput }
  host_agent_unbind: { input: HostAgentUnbindInput; output: HostAgentUnbindOutput }
  host_enrolment_create: { input: HostEnrolmentCreateInput; output: HostEnrolmentCreateOutput }
  host_list: { input: HostListInput; output: HostListOutput }
  host_unlink: { input: HostUnlinkInput; output: HostUnlinkOutput }
  message_delete: { input: MessageDeleteInput; output: MessageDeleteOutput }
  message_forward: { input: MessageForwardInput; output: MessageForwardOutput }
  message_history: { input: MessageHistoryInput; output: MessageHistoryOutput }
  message_preview_hide: { input: MessagePreviewHideInput; output: MessagePreviewHideOutput }
  message_react: { input: MessageReactInput; output: MessageReactOutput }
  message_schedule: { input: MessageScheduleInput; output: MessageScheduleOutput }
  message_send: { input: MessageSendInput; output: MessageSendOutput }
  message_update: { input: MessageUpdateInput; output: MessageUpdateOutput }
  module_archive: { input: ModuleArchiveInput; output: ModuleArchiveOutput }
  module_create: { input: ModuleCreateInput; output: ModuleCreateOutput }
  module_open: { input: ModuleOpenInput; output: ModuleOpenOutput }
  module_part_link: { input: ModulePartLinkInput; output: ModulePartLinkOutput }
  module_part_unlink: { input: ModulePartUnlinkInput; output: ModulePartUnlinkOutput }
  module_restore: { input: ModuleRestoreInput; output: ModuleRestoreOutput }
  module_set: { input: ModuleSetInput; output: ModuleSetOutput }
  module_update: { input: ModuleUpdateInput; output: ModuleUpdateOutput }
  package_export: { input: PackageExportInput; output: PackageExportOutput }
  package_install: { input: PackageInstallInput; output: PackageInstallOutput }
  package_list: { input: PackageListInput; output: PackageListOutput }
  package_preview: { input: PackagePreviewInput; output: PackagePreviewOutput }
  package_remove: { input: PackageRemoveInput; output: PackageRemoveOutput }
  run_activity_report: { input: RunActivityReportInput; output: RunActivityReportOutput }
  run_cancel: { input: RunCancelInput; output: RunCancelOutput }
  run_create: { input: RunCreateInput; output: RunCreateOutput }
  run_mark_not_started: { input: RunMarkNotStartedInput; output: RunMarkNotStartedOutput }
  run_result_report: { input: RunResultReportInput; output: RunResultReportOutput }
  run_update: { input: RunUpdateInput; output: RunUpdateOutput }
  run_usage_report: { input: RunUsageReportInput; output: RunUsageReportOutput }
  scheduled_message_update: { input: ScheduledMessageUpdateInput; output: ScheduledMessageUpdateOutput }
  task_archive: { input: TaskArchiveInput; output: TaskArchiveOutput }
  task_comment_create: { input: TaskCommentCreateInput; output: TaskCommentCreateOutput }
  task_comment_delete: { input: TaskCommentDeleteInput; output: TaskCommentDeleteOutput }
  task_comment_update: { input: TaskCommentUpdateInput; output: TaskCommentUpdateOutput }
  task_create: { input: TaskCreateInput; output: TaskCreateOutput }
  task_criteria_update: { input: TaskCriteriaUpdateInput; output: TaskCriteriaUpdateOutput }
  task_restore: { input: TaskRestoreInput; output: TaskRestoreOutput }
  task_update: { input: TaskUpdateInput; output: TaskUpdateOutput }
  whiteboard_create: { input: WhiteboardCreateInput; output: WhiteboardCreateOutput }
  whiteboard_duplicate: { input: WhiteboardDuplicateInput; output: WhiteboardDuplicateOutput }
  whiteboard_link: { input: WhiteboardLinkInput; output: WhiteboardLinkOutput }
  whiteboard_update: { input: WhiteboardUpdateInput; output: WhiteboardUpdateOutput }
  whiteboard_update_elements: { input: WhiteboardUpdateElementsInput; output: WhiteboardUpdateElementsOutput }
}

export type OpName = keyof Operations
