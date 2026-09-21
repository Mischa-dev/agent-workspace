import { z } from "zod";
import type { AGENT_OPERATIONS } from "./agent-http.js";
const id = z.string().uuid();
const ids = z.array(id).max(100);
const title = z.string().min(1).max(240);
const generation = z
  .number()
  .int()
  .positive()
  .describe("Claim generation returned by request_claim; reject stale work.");
const page = {
  afterId: id
    .optional()
    .describe("nextAfterId returned by the preceding page."),
  limit: z
    .number()
    .int()
    .min(1)
    .max(50)
    .optional()
    .describe("Maximum records in this page; default 20, maximum 50."),
};
const pins = z
  .array(z.object({ resourceId: id, revisionId: id }))
  .max(20)
  .describe(
    "Immutable Resource revision attachments already readable by this agent.",
  );
const taskFields = {
  title,
  description: z.string().max(20000).optional(),
  criteria: z
    .array(z.string().max(2000))
    .max(100)
    .optional()
    .describe("Concrete acceptance criteria; the owner reviews completion."),
  projectId: id.describe(
    "A project where this agent is an explicit participant.",
  ),
  parentId: id.optional(),
  dependencies: ids
    .optional()
    .describe("Existing prerequisite task IDs in the same project."),
  kind: z.enum(["feature", "bug", "research", "chore"]).optional(),
  execution: z
    .enum(["chat", "code"])
    .optional()
    .describe(
      "chat can be claimed externally; code needs an owner-approved local session.",
    ),
  parallel: z.boolean().optional(),
  locks: z.array(z.string().max(120)).max(30).optional(),
  resources: pins.optional(),
  assigneeId: id.optional(),
  blocker: z.string().max(2000).optional(),
  selected: z.boolean().optional(),
  position: z.number().optional(),
};
const taskDraft = z.object(taskFields);
const requestResult = z.object({
  body: z
    .string()
    .min(1)
    .max(20000)
    .describe(
      "Answer or completion summary; describe only actions actually completed.",
    ),
  generation,
  resources: pins.optional(),
});

export const MCP_INPUTS: Record<
  (typeof AGENT_OPERATIONS)[number],
  z.ZodType<Record<string, unknown>>
> = {
  "agent.me": z.object({}),
  "agent.list": z.object({
    ...page,
    query: z
      .string()
      .max(200)
      .optional()
      .describe(
        "Optional name or role search within the colleagues visible through shared membership.",
      ),
  }),
  "inbox.list": z.object({
    afterRequestId: id.optional(),
    afterTaskId: id.optional(),
    limit: page.limit,
  }),
  "conversation.list": z.object(page),
  "space.list": z.object(page),
  "project.list": z.object(page),
  "request.claim": z
    .object({
      id: id
        .optional()
        .describe("Pending request ID from inbox_list. Supply this OR taskId."),
      taskId: id
        .optional()
        .describe(
          "Assigned, ready, unblocked chat task to pick up; supply this OR id.",
        ),
    })
    .refine(
      (value) => !!value.id !== !!value.taskId,
      "Supply exactly one id or taskId.",
    ),
  "request.renew": z.object({ generation }),
  "request.release": z.object({
    generation,
    reason: z.string().max(2000),
    summary: z.string().max(20000).optional(),
  }),
  "message.reply": requestResult,
  "result.submit": requestResult,
  "request.handoff": z.object({
    agentId: id.describe(
      "Existing participant in this request's conversation.",
    ),
    body: z
      .string()
      .min(1)
      .max(20000)
      .describe("Handoff explanation and remaining work."),
    generation,
    attachments: pins.optional(),
  }),
  "message.send": z.object({
    conversationId: id,
    body: z.string().min(1).max(20000),
    recipientIds: ids
      .optional()
      .describe(
        "Explicit existing agent recipients; respects the conversation's finite response allowance.",
      ),
    replyTo: id.optional(),
    attachments: pins.optional(),
  }),
  "conversation.read": z.object({
    id: id
      .optional()
      .describe(
        "Conversation ID when no top-level requestId is supplied. Claimed context uses its frozen conversation.",
      ),
    beforeId: id.optional(),
    limit: z.number().int().min(1).max(50).optional(),
  }),
  "resource.list": z.object({
    ...page,
    spaceId: id.optional(),
    projectId: id.optional(),
  }),
  "resource.read": z.object({
    id: id.describe("Resource ID from resource_list or a saved attachment."),
    revisionId: id
      .optional()
      .describe(
        "Pinned revision; independent rounds may read only their frozen revision.",
      ),
  }),
  "resource.create": z.object({
    title,
    text: z
      .string()
      .max(256000)
      .describe("Persistent note, paste, or result text."),
    type: z.enum(["note", "paste", "output"]).optional(),
    spaceIds: ids.optional(),
    projectIds: ids.optional(),
    conversationIds: ids.optional(),
    agentIds: ids
      .optional()
      .describe(
        "Explicit audience within the chosen spaces/projects/conversations. File bytes use the upload endpoint.",
      ),
  }),
  "resource.revise": z.object({
    id,
    expectedRevision: z
      .number()
      .int()
      .positive()
      .describe(
        "Current Resource record revision for conflict detection, not revision ID.",
      ),
    text: z.string().max(256000),
    title: title.optional(),
  }),
  "task.list": z.object({ ...page, projectId: id.optional() }),
  "task.create": taskDraft,
  "task.update": taskDraft
    .omit({ projectId: true, execution: true })
    .partial()
    .extend({
      id,
      expectedRevision: z
        .number()
        .int()
        .positive()
        .describe(
          "Current task record revision. Participants can edit draft/ready tasks; edits return them to draft.",
        ),
    }),
  "plan.propose": z.object({
    title,
    projectId: id
      .optional()
      .describe(
        "Required outside a claimed request; inside one, its project is used.",
      ),
    conversationId: id.optional(),
    tasks: z
      .array(
        taskDraft.omit({ projectId: true }).extend({
          dependencyIndexes: z
            .array(z.number().int().min(0))
            .optional()
            .describe("Zero-based prerequisites within this proposal."),
          parentIndex: z.number().int().min(0).optional(),
        }),
      )
      .min(1)
      .max(100),
  }),
};
export const MCP_DESCRIPTIONS: Partial<
  Record<(typeof AGENT_OPERATIONS)[number], string>
> = {
  "agent.me":
    "Read your identity, connection, delivery mode and suggested check-in interval. Start here.",
  "agent.list":
    "Find colleagues visible through shared membership as {items,nextAfterId,total}. Search with query and paginate with afterId/limit (default 20,max 50). Use agent_list to resolve a target agent ID before sending or handing off work; directory visibility does not grant conversation access.",
  "inbox.list":
    "List bounded pages of pending/active requests and assigned tasks. Returns requests,tasks,nextRequestId,nextTaskId,totalRequests,totalTasks. Continue using afterRequestId/afterTaskId; default 20, max 50. Claim pending requests before answering; inspect uncertain work before retrying.",
  "conversation.list":
    "List permitted conversations as {items,nextAfterId,total}. Continue with afterId and limit (default 20,max 50).",
  "space.list":
    "List spaces where you participate as {items,nextAfterId,total}. Continue with afterId and limit (default 20,max 50).",
  "project.list":
    "List permitted projects as {items,nextAfterId,total}. Continue with afterId and limit (default 20,max 50).",
  "resource.list":
    "List permitted Resources as {items,nextAfterId,total}, optionally filtered by space/project. Continue with afterId and limit (default 20,max 50). Required request context and independent snapshots still apply.",
  "task.list":
    "List permitted tasks as {items,nextAfterId,total}, optionally filtered by project. Continue with afterId and limit (default 20,max 50).",
  "request.claim":
    "Claim exactly one pending inbox request using input.id, or one assigned ready chat task using input.taskId. On claimed:true, retain request ID, generation, frozen context and digest. Set top-level requestId on subsequent contextual tools.",
  "request.renew":
    "Renew a claimed request's fifteen-minute lease before it expires. Requires top-level requestId and the claim generation.",
  "request.release":
    "Release a claim with an honest reason and partial summary. Requires top-level requestId; the owner decides whether to retry.",
  "message.reply":
    "Answer your claimed request, optionally attaching saved Resource revisions. Requires top-level requestId and generation. A task answer enters owner review.",
  "result.submit":
    "Submit a claimed task result and persistent Resource attachments for owner review. Requires top-level requestId and generation; never accepts the task automatically.",
  "request.handoff":
    "Transfer your claimed response to another existing conversation participant with a reason. Use agent_list first to resolve the colleague's target agent ID. Requires top-level requestId and generation; the same finite allowance applies.",
  "resource.create":
    "Persist a note, paste or output. With requestId, audience and independent-round staging come from that request. Otherwise choose permitted scopes and explicit agent audience. Only attach returned resource/revision IDs after success.",
  "plan.propose":
    "Create a reviewable proposal of 1–100 tasks with dependencies and parents. This grants no execution permission; the owner reviews and applies it.",
};
