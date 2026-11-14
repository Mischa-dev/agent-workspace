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
  | { outcome: 'ok'; value: T; requestId?: string; replayed?: boolean; preview?: boolean }
  | OpError

export type Content = { body?: Record<string, unknown> | null; key: string; kind: ContentKind; path?: string | null }

export type ContentKind = "switches" | "layout" | "role" | "space" | "channels" | "module" | "collection" | "form" | "automation" | "page" | "skill" | "agent"

export type DecisionView = { createdAt: string; decidedBy: string; detail: string; id: string; projectId: string; sourceConversationId?: string | null; sourceConversationKind?: string | null; sourceConversationName?: string | null; sourceMessageId?: string | null; status: string; supersededByDecisionId?: string | null; title: string; via?: string | null }

export type HostAgentView = { accountId: string; actions: unknown[]; backends: unknown[]; hostAgentRef: string; name?: string | null; status: string }

export type HostOwnerChoice = "person" | "workspace"

export type HostView = { actions: unknown[]; agents: HostAgentView[]; builtin: boolean; canManage: boolean; capabilities: Record<string, unknown>; connection: string; createdAt: string; describedAgents: unknown[]; id: string; lastSeenAt?: string | null; moduleManifestUrl?: string | null; name: string; offers: unknown[]; owner: OwnerView; protocol: string; software?: string | null; softwareVersion?: string | null; status: string; version: number }

export type OwnerView = { id: string; kind: string; name: string }

export type Package = { contents: Content[]; description?: string; digest?: string | null; format: string; id: string; name: string; needsAdmin?: string[]; publisher?: Publisher | null; requires?: Requires; target: PackageTarget; version: string }

export type PackageTarget = "space" | "workspace"

export type Publisher = { name: string; url?: string | null }

export type RaisedRequestOutput = { activityId: string; clientRequestId?: string | null; requestId: string }

export type Requires = { contract?: string | null; modules?: string[] }

export type RunActivityInput = { at?: string | null; id: string; kind: string; parentRef?: string | null; payload?: unknown; stepRef?: string | null; summary?: string | null }

export type RunCauseInput = { conversationId?: string | null; kind: string; messageId?: string | null; ref?: string | null; taskId?: string | null }

export type RunRecord = { activityCount: number; cancelRequestedAt?: string | null; clientRunId?: string | null; externalUrl?: string | null; finishedAt?: string | null; liveness: string; reportIntervalSeconds: number; runId: string; state: string; statusLine?: string | null; title: string; version: number }

export type RunResultInput = { branch?: string | null; id?: string | null; kind: string; number?: number | null; thingKind?: string | null; title?: string | null; url?: string | null }

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

export type PackageExportInput = { includePages?: boolean; includeSkills?: boolean; module?: string | null; spaceId?: string | null; wholeWorkspace?: boolean }

export type PackageExportOutput = { digest: string; package: Package }

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

export type Operations = {
  decision_list: { input: DecisionListInput; output: DecisionListOutput }
  decision_record: { input: DecisionRecordInput; output: DecisionRecordOutput }
  host_agent_bind: { input: HostAgentBindInput; output: HostAgentBindOutput }
  host_agent_unbind: { input: HostAgentUnbindInput; output: HostAgentUnbindOutput }
  host_enrolment_create: { input: HostEnrolmentCreateInput; output: HostEnrolmentCreateOutput }
  host_list: { input: HostListInput; output: HostListOutput }
  host_unlink: { input: HostUnlinkInput; output: HostUnlinkOutput }
  package_export: { input: PackageExportInput; output: PackageExportOutput }
  run_activity_report: { input: RunActivityReportInput; output: RunActivityReportOutput }
  run_cancel: { input: RunCancelInput; output: RunCancelOutput }
  run_create: { input: RunCreateInput; output: RunCreateOutput }
  run_mark_not_started: { input: RunMarkNotStartedInput; output: RunMarkNotStartedOutput }
  run_result_report: { input: RunResultReportInput; output: RunResultReportOutput }
  run_update: { input: RunUpdateInput; output: RunUpdateOutput }
  run_usage_report: { input: RunUsageReportInput; output: RunUsageReportOutput }
}

export type OpName = keyof Operations
