export type Kind =
  | "agent"
  | "conversation"
  | "message"
  | "burst"
  | "request"
  | "resource"
  | "revision"
  | "space"
  | "project"
  | "task"
  | "proposal"
  | "scope"
  | "workSession"
  | "run"
  | "link";
export interface RecordRow<T = Record<string, any>> {
  id: string;
  kind: Kind;
  revision: number;
  data: T;
  createdAt: string;
  updatedAt: string;
}
export interface Attachment {
  resourceId: string;
  revisionId: string;
}
export interface Agent {
  name: string;
  role: string;
  color: string;
  capabilities: string[];
  enabled: boolean;
  deliveryMode?: "managed" | "poll" | "webhook";
  checkInMinutes?: number;
  lastSeenAt?: string;
  webhookUrl?: string;
  webhookConfigured?: boolean;
}
export interface Conversation {
  title: string;
  kind: "dm" | "group" | "channel" | "thread";
  parentConversationId?: string;
  rootMessageId?: string;
  agentIds: string[];
  spaceId?: string;
  projectId?: string;
  focusAgentId?: string;
  burstId?: string;
  stopped: boolean;
  mode: "auto" | "independent" | "discuss";
}
export interface Message {
  conversationId: string;
  authorId: string;
  body: string;
  replyTo?: string;
  forwardedFrom?: string;
  attachments: Attachment[];
  burstId?: string;
  requestId?: string;
  staged?: boolean;
  predatesCorrection?: boolean;
}
export type RequestStatus =
  | "pending"
  | "claimed"
  | "running"
  | "answered"
  | "failed"
  | "cancelled"
  | "superseded"
  | "needs_reconciliation";
export interface ResponseRequest {
  conversationId: string;
  messageId: string;
  agentId: string;
  burstId: string;
  status: RequestStatus;
  mode: string;
  reason: string;
  snapshotIds: string[];
  runId?: string;
  error?: string;
  generation: number;
  credentialId?: string;
  leaseUntil?: string;
  taskId?: string;
}
export interface Resource {
  title: string;
  type: "note" | "paste" | "file" | "output";
  latestRevisionId: string;
  conversationIds: string[];
  projectIds: string[];
  spaceIds: string[];
  agentIds: string[];
  stagedFor?: string;
  pinned?: boolean;
  tags?: string[];
}
export interface ResourceRevision {
  resourceId: string;
  text?: string;
  storageKey?: string;
  filename?: string;
  mimeType?: string;
  size: number;
  checksum: string;
  authorId: string;
  number: number;
}
export interface Space {
  name: string;
  description: string;
  color: string;
  agentIds?: string[];
}
export interface Project {
  name: string;
  description: string;
  spaceId?: string;
  repository?: string;
  targetBranch?: string;
  github?: string;
  agentIds?: string[];
}
export interface Task {
  assigneeId?: string;
  waitingReason?: string;
  outputResources?: Attachment[];
  title: string;
  description: string;
  projectId?: string;
  parentId?: string;
  kind: "feature" | "bug" | "research" | "chore";
  status: "draft" | "ready" | "running" | "review" | "accepted" | "cancelled";
  criteria: string[];
  dependencies: string[];
  resources: Attachment[];
  parallel: boolean;
  locks: string[];
  blocker?: string;
  branch?: string;
  commit?: string;
  integratedCommit?: string;
  runId?: string;
  selected: boolean;
  execution: "chat" | "code";
  position: number;
}
export interface Proposal {
  title: string;
  projectId?: string;
  conversationId?: string;
  status: "proposed" | "applied" | "dismissed";
  tasks: Partial<Task>[];
  sourceId?: string;
}
export interface WorkSession {
  scopeId: string;
  status:
    "active" | "stopping" | "stopped" | "complete" | "needs_reconciliation";
  deadline: string;
  maxWorkers: number;
  reservePercent: number;
  taskIds: string[];
  startedAt: string;
}
export interface Run {
  kind: "response" | "task";
  requestId?: string;
  taskId?: string;
  sessionId?: string;
  agentId: string;
  status:
    | "prepared"
    | "dispatching"
    | "running"
    | "completed"
    | "failed"
    | "cancelled"
    | "needs_reconciliation";
  packet: string;
  digest: string;
  manifest: Record<string, any>;
  runtime?: Record<string, any>;
  events: { type: string; at: string; data: Record<string, any> }[];
  eventsDropped?: number;
  retrievalReceipts?: { type: string; at: string; data: Record<string, any> }[];
  output?: string;
  error?: string;
  stopRequested?: boolean;
  worktree?: string;
}
export interface Snapshot {
  records: RecordRow[];
  cursor: number;
  dispatchEnabled: boolean;
  workerOnline: boolean;
  runtime: Record<string, unknown>;
  github: Record<string, unknown>;
}
export interface Command {
  id: string;
  type: string;
  input: Record<string, any>;
}
export interface CommandResult {
  value: any;
  cursor: number;
}
// HTTP: GET /api/session -> {authenticated,csrf?}; POST /api/login {token}; POST /api/logout.
// GET /api/snapshot -> Snapshot; POST /api/commands -> CommandResult (X-CSRF-Token required).
// GET /api/events?after=<cursor> -> SSE event: change, id: cursor, data: {type,ids}; overflow => reconcile.
// POST /api/uploads multipart file + title + conversationIds(JSON) -> resource/revision; same CSRF.
// GET /api/resources/:resourceId/revisions/:revisionId/download -> authenticated immutable bytes.
// Command names: conversation.create/update, message.send, conversation.stop/continue,
// resource.create/revise/grant, space.create/update, project.create/update, task.create/update,
// proposal.create/apply/dismiss, scope.approve, session.start/stop, run.reconcile, settings.dispatch.
// Updates require expectedRevision. All ids generated by server; command ids are browser UUIDs.
