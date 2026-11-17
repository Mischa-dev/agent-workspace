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

export type ActivityLine = { kind: string; occurredAt: string; parentRef?: string | null; payload: unknown; seq: number; stepRef?: string | null; summary: string }

export type AgentUsageLine = { accountId: string; cachedInputTokens: number; confidence: string; cost?: number | null; currency?: string | null; handle?: string | null; inputTokens: number; name?: string | null; outputTokens: number; runs: number }

export type AudienceInput = { accounts?: string[]; roles?: string[] }

export type CommentThreadView = { anchor?: unknown; createdAt: string; id: string; messageCount: number; name: string; outcome?: string | null; status: string; unread: boolean }

export type Content = { body?: Record<string, unknown> | null; key: string; kind: ContentKind; path?: string | null }

export type ContentKind = "switches" | "layout" | "role" | "space" | "channels" | "module" | "collection" | "form" | "automation" | "page" | "skill" | "agent"

export type CustomEmojiView = { createdAt: string; createdByAccountId: string; id: string; imageUrl: string; name: string; retired: boolean }

export type DecisionView = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

export type DraftView = { conversationId: string; conversationKind: string; conversationName?: string | null; draft: unknown; previous?: unknown; spaceId: string; updatedAt: string }

export type FollowedThreadView = { id: string; lastActivityAt: string; messageCount: number; name: string; originMessageId?: string | null; outcome?: string | null; parentId: string; parentKind: string; parentName?: string | null; spaceId: string; spaceName: string; status: string; unread: boolean }

export type HistoryCaps = { diff: boolean; list: boolean; restore: boolean; showVersion: boolean }

export type HistoryEntry = { actor?: string | null; actorId?: string | null; at: string; changed: string[]; summary: string; version: number; via?: string | null }

export type HostAgentView = { accountId: string; actions: unknown[]; backends: unknown[]; hostAgentRef: string; name?: string | null; status: string }

export type HostOwnerChoice = "person" | "workspace"

export type HostView = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type ModelUsageLine = { cachedInputTokens: number; confidence: string; cost?: number | null; currency?: string | null; inputTokens: number; model: string; outputTokens: number }

export type OwnerView = { id: string; kind: string; name: string }

export type Package = { contents: Content[]; description?: string; digest?: string | null; format: string; id: string; name: string; needsAdmin?: string[]; publisher?: Publisher | null; requires?: Requires; target: PackageTarget; version: string }

export type PackageTarget = "space" | "workspace"

export type PageTemplateView = { builtIn: boolean; excerpt: string; id: string; projectName?: string | null; title: string }

export type Publisher = { name: string; url?: string | null }

export type PullRequest = { baseRef: string; checksState: string; draft: boolean; headRef: string; id: string; mergedAt?: string | null; number: number; repository: string; repositoryId: string; reviewState: string; spaceId: string; state: string; title: string; updatedAt: string; url: string }

export type RaisedRequestOutput = { activityId: string; clientRequestId?: string | null; requestId: string }

export type RequestAnswerView = { label?: string | null; optionId?: string | null; text?: string | null }

export type RequestLine = { answeredAt?: string | null; conversationId?: string | null; createdAt: string; id: string; kind: string; status: string; title: string }

export type RequestOption = { id: string; label: string; style?: string | null }

export type RequestOptionInput = { id: string; label: string; style?: string | null }

export type RequestView = { addressedAccounts: string[]; addressedRoles: string[]; allowText: boolean; answer?: RequestAnswerView | null; answeredAt?: string | null; answeredBy?: string | null; conversationId?: string | null; createdAt: string; detail?: string | null; expiresAt?: string | null; id: string; kind: string; options: RequestOption[]; raisedBy: string; runId?: string | null; spaceId?: string | null; status: string; taskId?: string | null; title: string; version: number }

export type Requires = { contract?: string | null; modules?: string[] }

export type RunActivityInput = { at?: string | null; id: string; kind: string; parentRef?: string | null; payload?: unknown; stepRef?: string | null; summary?: string | null }

export type RunCauseInput = { conversationId?: string | null; kind: string; messageId?: string | null; ref?: string | null; taskId?: string | null }

export type RunRecord = { activityCount: number; cancelRequestedAt?: string | null; clientRunId?: string | null; externalUrl?: string | null; finishedAt?: string | null; liveness: string; reportIntervalSeconds: number; runId: string; state: string; statusLine?: string | null; title: string; version: number }

export type RunResultInput = { branch?: string | null; id?: string | null; kind: string; number?: number | null; thingKind?: string | null; title?: string | null; url?: string | null }

export type StatusView = { accountId: string; awayUntil?: string | null; clearAt?: string | null; emoji?: string | null; state?: string | null; text?: string | null; updatedAt: string; workspaceOnly: boolean }

export type SubjectKind = "page" | "sheet" | "whiteboard" | "record" | "task"

export type ThingInput = { id: string; kind: SubjectKind }

export type UnattributedUsageLine = { accountId: string; cost?: number | null; currency?: string | null; inputTokens: number; name?: string | null; outputTokens: number; reports: number }

export type WhiteboardTemplateView = { builtIn: boolean; id: string; projectName?: string | null; title: string }

export type AgentManagersSetInput = { add?: string[]; agent: string; remove?: string[] }

export type AgentManagersSetOutput = { agent: string; managers: string[] }

export type AgentUsageListInput = { days?: number | null }

export type AgentUsageListOutput = { accounts: AgentUsageLine[]; days: number; notTiedToARun: UnattributedUsageLine[] }

export type CommentCreateInput = { anchor?: unknown; attachmentIds?: string[]; content?: unknown; newThread?: boolean; replyToMessageId?: string | null; text?: string | null; thing: ThingInput; threadId?: string | null }

export type CommentCreateOutput = { conversationId: string; href?: string | null; messageId: string; threadId?: string | null }

export type CommentListInput = { status?: string | null; thing: ThingInput }

export type CommentListOutput = { acceptsComments: boolean; conversationId?: string | null; messageCount: number; spaceId?: string | null; threads: CommentThreadView[]; unread: boolean }

export type CommentThreadUpdateInput = { outcome?: string | null; status: string; threadId: string }

export type CommentThreadUpdateOutput = { outcome?: string | null; status: string; threadId: string }

export type ConversationsMarkReadInput = { spaceId?: string | null }

export type ConversationsMarkReadOutput = { marked: number; more: boolean }

export type CustomEmojiCreateInput = { imageBase64: string; name: string }

export type CustomEmojiCreateOutput = { createdAt: string; createdByAccountId: string; id: string; imageUrl: string; name: string; retired: boolean }

export type CustomEmojiListInput = { includeRetired?: boolean }

export type CustomEmojiListOutput = { emoji: CustomEmojiView[] }

export type CustomEmojiUpdateInput = { id: string; retired: boolean }

export type CustomEmojiUpdateOutput = { createdAt: string; createdByAccountId: string; id: string; imageUrl: string; name: string; retired: boolean }

export type DecisionListInput = { includeSuperseded?: boolean; limit?: number | null; spaceId: string }

export type DecisionListOutput = { decisions: DecisionView[] }

export type DecisionRecordInput = { detail?: string | null; pin?: boolean; sourceConversationId?: string | null; sourceMessageId?: string | null; spaceId?: string | null; supersedes?: string | null; title: string }

export type DecisionRecordOutput = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

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

export type MessageDraftListInput = { conversationId?: string | null }

export type MessageDraftListOutput = { drafts: DraftView[] }

export type MessageDraftSetInput = { conversationId: string; draft?: unknown; savedAt?: string | null }

export type MessageDraftSetOutput = { draft?: DraftView | null; keptNewer: boolean }

export type NotificationKeywordListInput = Record<string, never>

export type NotificationKeywordListOutput = { keywords: string[]; limit: number }

export type NotificationKeywordSetInput = { keywords: string[] }

export type NotificationKeywordSetOutput = { keywords: string[]; limit: number }

export type PackageExportInput = { includePages?: boolean; includeSkills?: boolean; module?: string | null; spaceId?: string | null; wholeWorkspace?: boolean }

export type PackageExportOutput = { digest: string; package: Package }

export type PageTemplateListInput = { spaceId: string }

export type PageTemplateListOutput = { templates: PageTemplateView[] }

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

export type StatusDeleteInput = { account?: string | null; workspaceOnly?: boolean }

export type StatusDeleteOutput = { cleared: boolean }

export type StatusListInput = { accounts: string[] }

export type StatusListOutput = { statuses: StatusView[] }

export type StatusSetInput = { account?: string | null; awayUntil?: string | null; clearAt?: string | null; emoji?: string | null; state?: string | null; text?: string | null; workspaceOnly?: boolean }

export type StatusSetOutput = { accountId: string; awayUntil?: string | null; clearAt?: string | null; emoji?: string | null; state?: string | null; text?: string | null; updatedAt: string; workspaceOnly: boolean }

export type ThreadFollowedListInput = { limit?: number | null; openOnly?: boolean }

export type ThreadFollowedListOutput = { threads: FollowedThreadView[] }

export type WhiteboardTemplateListInput = { spaceId?: string | null }

export type WhiteboardTemplateListOutput = { templates: WhiteboardTemplateView[] }

export type Operations = {
  agent_managers_set: { input: AgentManagersSetInput; output: AgentManagersSetOutput }
  agent_usage_list: { input: AgentUsageListInput; output: AgentUsageListOutput }
  comment_create: { input: CommentCreateInput; output: CommentCreateOutput }
  comment_list: { input: CommentListInput; output: CommentListOutput }
  comment_thread_update: { input: CommentThreadUpdateInput; output: CommentThreadUpdateOutput }
  conversations_mark_read: { input: ConversationsMarkReadInput; output: ConversationsMarkReadOutput }
  custom_emoji_create: { input: CustomEmojiCreateInput; output: CustomEmojiCreateOutput }
  custom_emoji_list: { input: CustomEmojiListInput; output: CustomEmojiListOutput }
  custom_emoji_update: { input: CustomEmojiUpdateInput; output: CustomEmojiUpdateOutput }
  decision_list: { input: DecisionListInput; output: DecisionListOutput }
  decision_record: { input: DecisionRecordInput; output: DecisionRecordOutput }
  history_list: { input: HistoryListInput; output: HistoryListOutput }
  history_restore: { input: HistoryRestoreInput; output: HistoryRestoreOutput }
  history_show: { input: HistoryShowInput; output: HistoryShowOutput }
  host_agent_bind: { input: HostAgentBindInput; output: HostAgentBindOutput }
  host_agent_unbind: { input: HostAgentUnbindInput; output: HostAgentUnbindOutput }
  host_enrolment_create: { input: HostEnrolmentCreateInput; output: HostEnrolmentCreateOutput }
  host_list: { input: HostListInput; output: HostListOutput }
  host_unlink: { input: HostUnlinkInput; output: HostUnlinkOutput }
  message_draft_list: { input: MessageDraftListInput; output: MessageDraftListOutput }
  message_draft_set: { input: MessageDraftSetInput; output: MessageDraftSetOutput }
  notification_keyword_list: { input: NotificationKeywordListInput; output: NotificationKeywordListOutput }
  notification_keyword_set: { input: NotificationKeywordSetInput; output: NotificationKeywordSetOutput }
  package_export: { input: PackageExportInput; output: PackageExportOutput }
  page_template_list: { input: PageTemplateListInput; output: PageTemplateListOutput }
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
  status_delete: { input: StatusDeleteInput; output: StatusDeleteOutput }
  status_list: { input: StatusListInput; output: StatusListOutput }
  status_set: { input: StatusSetInput; output: StatusSetOutput }
  thread_followed_list: { input: ThreadFollowedListInput; output: ThreadFollowedListOutput }
  whiteboard_template_list: { input: WhiteboardTemplateListInput; output: WhiteboardTemplateListOutput }
}

export type OpName = keyof Operations
