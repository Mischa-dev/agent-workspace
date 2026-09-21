import type { Actor } from "./policy.js";
import {
  canAccessProject,
  canAccessTask,
  canReadResource,
  requestContext,
  resourceAccess,
  readiness,
  validateTask,
} from "./policy.js";
import { AppError, hash, type Store } from "./store.js";
import { claimRequest } from "./admission.js";
import { taskDefaults } from "./commands.js";
import type {
  Conversation,
  Message,
  Resource,
  Task,
  ResponseRequest,
} from "../../../../packages/shared/src/index.js";
const text = (v: any, max = 20000) => {
  if (typeof v !== "string" || !v.trim() || v.length > max)
    throw new AppError(400, `Text must contain 1–${max} characters.`);
  return v.trim();
};
const strings = (v: any): string[] =>
  Array.isArray(v)
    ? ([...new Set(v.filter((x: any) => typeof x === "string"))] as string[])
    : [];
const conversation = (s: Store, a: Actor, id: string) => {
  const c = s.get<Conversation>(id, "conversation");
  if (!c.data.agentIds.includes(a.id))
    throw new AppError(404, "Conversation not found.");
  if (a.requestId && requestContext(s, a).c.id !== c.id)
    throw new AppError(403, "Conversation is outside this request.");
  return c;
};
export function participantResourceScope(s: Store, a: Actor, i: any) {
  const spaceIds = strings(i.spaceIds),
    projectIds = strings(i.projectIds),
    conversationIds = strings(i.conversationIds);
  const allowed = new Set([a.id]);
  for (const id of spaceIds) {
    const space = s.get(id, "space");
    if (!space.data.agentIds?.includes(a.id))
      throw new AppError(404, "Space not found.");
    for (const member of space.data.agentIds) allowed.add(member);
  }
  for (const id of projectIds) {
    if (!canAccessProject(s, a, id))
      throw new AppError(404, "Project not found.");
    const p = s.get(id, "project");
    for (const member of p.data.agentIds || []) allowed.add(member);
    if (p.data.spaceId)
      for (const member of s.get(p.data.spaceId, "space").data.agentIds || [])
        allowed.add(member);
  }
  for (const id of conversationIds)
    for (const member of conversation(s, a, id).data.agentIds)
      allowed.add(member);
  const agentIds = [...new Set([a.id, ...strings(i.agentIds)])];
  if (agentIds.some((id) => !allowed.has(id)))
    throw new AppError(
      403,
      "Share only with members of the selected spaces, projects, or conversations.",
    );
  return { spaceIds, projectIds, conversationIds, agentIds };
}
function pins(s: Store, a: Actor, input: any) {
  if (input !== undefined && (!Array.isArray(input) || input.length > 20))
    throw new AppError(400, "Choose at most 20 attachments.");
  return (input || []).map((p: any) => {
    resourceAccess(s, a, p.resourceId);
    if (s.get(p.revisionId, "revision").data.resourceId !== p.resourceId)
      throw new AppError(400, "Revision does not belong to Resource.");
    return { resourceId: p.resourceId, revisionId: p.revisionId };
  });
}
function visibleTask(s: Store, a: Actor, id: string) {
  const t = s.get<Task>(id, "task");
  if (!canAccessTask(s, a, t)) throw new AppError(404, "Task not found.");
  return t;
}
function validateTaskLinks(s: Store, a: Actor, t: Task) {
  if (a.requestId && requestContext(s, a).c.data.projectId !== t.projectId)
    throw new AppError(403, "Task is outside this request's project.");
  if (
    !Array.isArray(t.dependencies) ||
    !Array.isArray(t.criteria) ||
    !Array.isArray(t.locks) ||
    typeof t.description !== "string"
  )
    throw new AppError(
      400,
      "Use text for description and arrays for criteria, dependencies and locks.",
    );
  if (
    t.dependencies.length > 100 ||
    t.criteria.length > 100 ||
    t.locks.length > 100 ||
    t.description.length > 50000
  )
    throw new AppError(400, "Task input exceeds supported limits.");
  if (!t.projectId || !canAccessProject(s, a, t.projectId))
    throw new AppError(404, "Project not found.");
  for (const id of [...t.dependencies, ...(t.parentId ? [t.parentId] : [])]) {
    const linked = visibleTask(s, a, id);
    if (linked.data.projectId !== t.projectId)
      throw new AppError(400, "Related tasks must belong to the same project.");
  }
  if (t.assigneeId) {
    s.get(t.assigneeId, "agent");
    if (
      !canAccessProject(s, { id: t.assigneeId, participant: true }, t.projectId)
    )
      throw new AppError(400, "Assignee must be a project participant.");
  }
  t.resources = pins(s, a, t.resources);
  validateTask(s, t);
}
function page<T>(
  items: T[],
  input: any,
  key: (item: T) => string = (item: any) => item.id,
) {
  const offset = input.afterId
    ? items.findIndex((item) => key(item) === input.afterId) + 1
    : 0;
  if (input.afterId && !offset)
    throw new AppError(404, "Cursor is outside this view.");
  const limit = Math.min(
    50,
    Math.max(1, Math.floor(Number(input.limit) || 20)),
  );
  const selected = items.slice(offset, offset + limit);
  return {
    items: selected,
    nextAfterId:
      offset + selected.length < items.length
        ? key(selected[selected.length - 1])
        : null,
    total: items.length,
  };
}
export async function participantCommand(
  s: Store,
  a: Actor,
  type: string,
  i: any,
): Promise<{ handled: boolean; value?: any }> {
  const done = (value: any) => ({ handled: true, value });
  if (type === "agent.me") {
    const agent = s.get(a.id, "agent");
    const lastSeenAt = (
      await s.client.query(
        "SELECT max(last_seen_at) seen FROM agent_credentials WHERE agent_id=$1",
        [a.id],
      )
    ).rows[0]?.seen;
    return done({
      agent: {
        ...agent,
        data: { ...agent.data, lastSeenAt: lastSeenAt?.toISOString() },
      },
      connectionId: a.credentialId,
      deliveryMode: agent.data.deliveryMode || "managed",
      protocol: "workspace-v1",
      claimLeaseSeconds: 900,
      instructions:
        "List your inbox, claim one request, read its context, then reply or hand off. Keep requestId on contextual calls. Renew long work before the lease expires. For pending assigned chat tasks use request.claim with taskId. A heartbeat schedule belongs to your client; checkInMinutes is a suggested interval.",
    });
  }
  if (type === "agent.list") {
    const visible = new Set([a.id]);
    const add = (ids: string[] = []) => ids.forEach((id) => visible.add(id));
    if (a.requestId) add(requestContext(s, a).c.data.agentIds);
    else {
      for (const c of s.all<Conversation>("conversation"))
        if (c.data.agentIds.includes(a.id)) add(c.data.agentIds);
      for (const space of s.all("space"))
        if (space.data.agentIds?.includes(a.id)) add(space.data.agentIds);
      for (const project of s.all("project"))
        if (canAccessProject(s, a, project.id)) add(project.data.agentIds);
      for (const task of s.all<Task>("task"))
        if (task.data.assigneeId && canAccessTask(s, a, task))
          visible.add(task.data.assigneeId);
    }
    const query = String(i.query || "").toLowerCase();
    return done(
      page(
        s
          .all("agent")
          .filter(
            (r) =>
              visible.has(r.id) &&
              (!query ||
                `${r.data.name} ${r.data.role} ${(r.data.capabilities || []).join(" ")}`
                  .toLowerCase()
                  .includes(query)),
          )
          .map((r) => ({
            ...r,
            data: {
              name: r.data.name,
              role: r.data.role,
              color: r.data.color,
              capabilities: r.data.capabilities,
              enabled: r.data.enabled,
              deliveryMode: r.data.deliveryMode || "managed",
              checkInMinutes: r.data.checkInMinutes,
            },
          })),
        i,
      ),
    );
  }
  if (type === "conversation.list")
    return done(
      page(
        s
          .all<Conversation>("conversation")
          .filter(
            (c) =>
              c.data.agentIds.includes(a.id) &&
              (!a.requestId || c.id === requestContext(s, a).c.id),
          ),
        i,
      ),
    );
  if (type === "space.list")
    return done(
      page(
        s
          .all("space")
          .filter(
            (r) =>
              r.data.agentIds?.includes(a.id) &&
              (!a.requestId || r.id === requestContext(s, a).c.data.spaceId),
          ),
        i,
      ),
    );
  if (type === "project.list")
    return done(
      page(
        s
          .all("project")
          .filter(
            (r) =>
              canAccessProject(s, a, r.id) &&
              (!a.requestId || r.id === requestContext(s, a).c.data.projectId),
          ),
        i,
      ),
    );
  if (type === "inbox.list") {
    const requests = s
      .all<ResponseRequest>("request")
      .filter(
        (r) =>
          r.data.agentId === a.id &&
          ["pending", "claimed", "running", "needs_reconciliation"].includes(
            r.data.status,
          ) &&
          s
            .get(r.data.conversationId, "conversation")
            .data.agentIds.includes(a.id),
      )
      .map((r) => {
        const c = s.get(r.data.conversationId, "conversation"),
          b = s.get(r.data.burstId, "burst");
        return {
          request: r,
          conversation: { id: c.id, title: c.data.title },
          message: s.get(r.data.messageId, "message"),
          waitingReason:
            c.data.stopped || b.data.stopped
              ? "Conversation stopped"
              : b.data.remaining <= 0 && r.data.status === "pending"
                ? "Owner continuation needed"
                : undefined,
        };
      });
    const tasks = s
      .all<Task>("task")
      .filter(
        (t) =>
          t.data.assigneeId === a.id &&
          canAccessTask(s, a, t) &&
          !["accepted", "cancelled"].includes(t.data.status),
      )
      .map((t) => ({
        task: t,
        ...readiness(s, t),
        pickupSupported: t.data.execution === "chat",
      }));
    const rp = page(
      requests,
      { limit: i.limit, afterId: i.afterRequestId },
      (r) => r.request.id,
    );
    const tp = page(
      tasks,
      { limit: i.limit, afterId: i.afterTaskId },
      (t) => t.task.id,
    );
    return done({
      requests: rp.items,
      tasks: tp.items,
      nextRequestId: rp.nextAfterId,
      nextTaskId: tp.nextAfterId,
      totalRequests: rp.total,
      totalTasks: tp.total,
      checkedAt: new Date().toISOString(),
    });
  }
  if (type === "conversation.read" && !a.requestId) {
    const c = conversation(s, a, i.id || i.conversationId);
    const all = s
      .all<Message>("message")
      .filter((m) => m.data.conversationId === c.id && !m.data.staged);
    const before = i.beforeId
      ? all.findIndex((m) => m.id === i.beforeId)
      : all.length;
    if (before < 0) throw new AppError(404, "Message cursor not found.");
    const limit = Math.min(50, Math.max(1, Number(i.limit) || 30)),
      items = all.slice(Math.max(0, before - limit), before);
    return done({
      items,
      nextBeforeId: before > items.length ? items[0]?.id : null,
      totalAllowed: all.length,
    });
  }
  if (type === "resource.list")
    return done(
      page(
        s
          .all<Resource>("resource")
          .filter(
            (r) =>
              canReadResource(s, a, r) &&
              (!i.spaceId || r.data.spaceIds.includes(i.spaceId)) &&
              (!i.projectId || r.data.projectIds.includes(i.projectId)),
          ),
        i,
      ),
    );
  if (type === "task.list")
    return done(
      page(
        s
          .all<Task>("task")
          .filter(
            (t) =>
              canAccessTask(s, a, t) &&
              (!i.projectId || t.data.projectId === i.projectId),
          ),
        i,
      ),
    );
  if (type === "task.create") {
    const t = taskDefaults(i);
    validateTaskLinks(s, a, t);
    return done(s.add("task", t));
  }
  if (type === "task.update") {
    const row = visibleTask(s, a, i.id);
    if (!["draft", "ready"].includes(row.data.status))
      throw new AppError(
        409,
        "Only draft or ready tasks can be edited by participants.",
      );
    if (i.status && i.status !== "draft")
      throw new AppError(403, "The owner authorizes work and accepts results.");
    const allowed = [
      "title",
      "description",
      "criteria",
      "dependencies",
      "parentId",
      "kind",
      "resources",
      "parallel",
      "locks",
      "blocker",
      "selected",
      "position",
      "assigneeId",
    ];
    const changes = Object.fromEntries(
      Object.entries(i).filter(([k]) => allowed.includes(k)),
    );
    const t = { ...row.data, ...changes, status: "draft" as const };
    t.title = text(t.title, 240);
    validateTaskLinks(s, a, t);
    validateTask(s, t, row.id);
    return done(s.patch(row.id, t, i.expectedRevision));
  }
  if (type === "plan.propose" || type === "proposal.create") {
    if (!a.requestId) {
      if (i.conversationId) conversation(s, a, i.conversationId);
      if (!i.projectId || !canAccessProject(s, a, i.projectId))
        throw new AppError(404, "Project not found.");
    }
    return { handled: false };
  }
  if (type === "request.claim") {
    let reqId = i.id || i.requestId;
    if (i.taskId) {
      const task = visibleTask(s, a, i.taskId);
      if (
        task.data.assigneeId !== a.id ||
        task.data.status !== "ready" ||
        !readiness(s, task).ready
      )
        throw new AppError(
          409,
          "Task must be assigned to you, ready, and unblocked.",
        );
      if (task.data.execution !== "chat")
        throw new AppError(
          403,
          "Repository execution uses an owner-approved local work session.",
        );
      const project = task.data.projectId
        ? s.get(task.data.projectId, "project")
        : undefined;
      const c = s.add("conversation", {
        title: task.data.title,
        kind: "group",
        projectId: project?.id,
        spaceId: project?.data.spaceId,
        agentIds: [...new Set([a.id, ...(project?.data.agentIds || [])])],
        stopped: false,
        mode: "auto",
      });
      for (const pin of task.data.resources) {
        const r = resourceAccess(s, a, pin.resourceId);
        s.patch(r.id, {
          conversationIds: [...new Set([...r.data.conversationIds, c.id])],
        });
      }
      const burst = s.add("burst", {
        conversationId: c.id,
        remaining: 6,
        stopped: false,
        mode: "auto",
        participants: [a.id],
        published: false,
      });
      const message = s.add("message", {
        conversationId: c.id,
        authorId: "system",
        body: `Assigned task: ${task.data.title}\n\n${task.data.description}\n\nAcceptance criteria:\n${task.data.criteria.map((x) => `- ${x}`).join("\n")}`,
        attachments: task.data.resources,
        burstId: burst.id,
      });
      s.patch(c.id, { burstId: burst.id });
      const req = s.add("request", {
        conversationId: c.id,
        messageId: message.id,
        agentId: a.id,
        burstId: burst.id,
        status: "pending",
        mode: "auto",
        reason: "Assigned task",
        snapshotIds: [message.id],
        generation: 1,
        taskId: task.id,
      });
      reqId = req.id;
    }
    const req = s.get<ResponseRequest>(reqId, "request");
    if (req.data.agentId !== a.id)
      throw new AppError(404, "Request not found.");
    if (req.data.status !== "pending")
      throw new AppError(409, "Request is already claimed or resolved.");
    const enabled = (
      await s.client.query(
        "SELECT value FROM settings WHERE key='dispatchEnabled'",
      )
    ).rows[0]?.value;
    if (enabled === false)
      throw new AppError(409, "Workspace dispatch is paused.");
    const run = await claimRequest(s, req.id, {
      credentialId: a.credentialId!,
    });
    if (!run)
      return done({
        claimed: false,
        request: s.get(req.id),
        reason:
          "Stopped, blocked, busy, context unavailable, or response allowance exhausted.",
      });
    if (req.data.taskId)
      s.patch(req.data.taskId, { status: "running", runId: run.id });
    return done({
      claimed: true,
      request: s.get(req.id),
      runId: run.id,
      context: JSON.parse(run.data.packet),
      digest: run.data.digest,
    });
  }
  if (
    type === "request.renew" ||
    type === "request.release" ||
    type === "message.reply" ||
    type === "result.submit" ||
    type === "request.handoff"
  ) {
    if (!a.requestId) a = { ...a, requestId: i.id || i.requestId };
    if (type === "result.submit") {
      const req = s.get<ResponseRequest>(a.requestId!, "request");
      if (
        req.data.agentId !== a.id ||
        req.data.credentialId !== a.credentialId ||
        !req.data.runId
      )
        throw new AppError(403, "Result does not belong to this connection.");
      const run = s.get(req.data.runId, "run");
      if (i.generation !== undefined && i.generation !== req.data.generation)
        throw new AppError(409, "Claim generation changed.");
      if (
        !["running", "claimed"].includes(req.data.status) ||
        Date.parse(req.data.leaseUntil || "") <= Date.now() ||
        run.data.stopRequested
      ) {
        const body = text(i.body || i.summary || i.text);
        const resource = s.add("resource", {
          title: "Result received after work stopped",
          type: "output",
          agentIds: [a.id],
          conversationIds: [],
          projectIds: [],
          spaceIds: [],
          latestRevisionId: "",
        });
        const revision = s.add("revision", {
          resourceId: resource.id,
          text: body,
          number: 1,
          authorId: a.id,
          size: Buffer.byteLength(body),
          checksum: hash(body),
        });
        s.patch(resource.id, { latestRevisionId: revision.id });
        s.patch(run.id, {
          lateOutputResource: {
            resourceId: resource.id,
            revisionId: revision.id,
          },
        });
        return done({
          preserved: true,
          accepted: false,
          resource: s.get(resource.id),
          reason:
            "Work was stopped, resolved, or its lease expired. The owner can inspect this output; it did not complete the request.",
        });
      }
    }
    const { req, c } = requestContext(s, a);
    if (i.generation !== undefined && i.generation !== req.data.generation)
      throw new AppError(409, "Claim generation changed.");
    if (type === "request.renew")
      return done(
        s.patch(req.id, {
          leaseUntil: new Date(Date.now() + 900000).toISOString(),
        }),
      );
    if (type === "request.release") {
      s.patch(req.id, {
        status: "failed",
        error: String(i.reason || "Released by connection").slice(0, 2000),
        leaseUntil: undefined,
      });
      s.patch(req.data.runId!, {
        status: "cancelled",
        output: String(i.summary || ""),
        error: "Connection released work; owner can retry.",
      });
      if (req.data.taskId)
        s.patch(req.data.taskId, { status: "draft", runId: undefined });
      return done(s.get(req.id));
    }
    if (type === "request.handoff")
      return done(
        send(
          s,
          a,
          {
            ...i,
            conversationId: c.id,
            recipientIds: [i.agentId || i.toAgentId],
            body: i.body || i.reason || "Please take over this request.",
            replyTo: req.data.messageId,
          },
          req,
        ),
      );
    const message = s.add("message", {
      conversationId: c.id,
      authorId: a.id,
      body: text(i.body || i.summary || i.text),
      replyTo: req.data.messageId,
      requestId: req.id,
      burstId: req.data.burstId,
      attachments: pins(s, a, i.attachments || i.resources),
    });
    s.patch(req.id, { status: "answered", leaseUntil: undefined });
    s.patch(req.data.runId!, {
      status: "completed",
      output: message.data.body,
      events: [
        {
          type: "participant.result",
          at: new Date().toISOString(),
          data: { messageId: message.id, credentialId: a.credentialId },
        },
      ],
    });
    s.patch(c.id, {
      focusAgentId: a.id,
      pendingQuestion: message.data.body.endsWith("?"),
    });
    if (req.data.taskId)
      s.patch(req.data.taskId, {
        status: "review",
        outputResources: message.data.attachments,
      });
    return done({ message, request: s.get(req.id) });
  }
  if (type === "message.send") return done(send(s, a, i));
  if (
    [
      "resource.read",
      "resource.create",
      "resource.revise",
      "conversation.read",
    ].includes(type)
  )
    return { handled: false };
  throw new AppError(403, "This operation is unavailable to agent accounts.");
}
function send(s: Store, a: Actor, i: any, handedOff?: any) {
  const c = conversation(s, a, i.conversationId);
  if (c.data.stopped) throw new AppError(409, "Conversation is stopped.");
  const body = text(i.body);
  const mentioned = c.data.agentIds.filter((id) => {
    const name = s
      .get(id, "agent")
      .data.name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`(?:^|\\s)@${name}(?=\\s|[,.:!?]|$)`, "i").test(
      body
        .split("\n")
        .filter((l) => !/^\s*>/.test(l))
        .join("\n"),
    );
  });
  const recipients = handedOff
    ? strings(i.recipientIds).filter((id) => id !== a.id)
    : [...new Set([...strings(i.recipientIds), ...mentioned])].filter(
        (id) => id !== a.id,
      );
  if (
    recipients.some(
      (id) => !c.data.agentIds.includes(id) || !s.get(id, "agent").data.enabled,
    )
  )
    throw new AppError(
      400,
      "Recipient is not an enabled conversation participant.",
    );
  if (handedOff && !recipients.length)
    throw new AppError(400, "Choose a different participant for the handoff.");
  if (i.replyTo && s.get(i.replyTo, "message").data.conversationId !== c.id)
    throw new AppError(400, "Reply belongs to another conversation.");
  const burst = a.requestId
    ? s.get(requestContext(s, a).req.data.burstId, "burst")
    : c.data.burstId
      ? s.get(c.data.burstId, "burst")
      : undefined;
  if (
    recipients.length &&
    (!burst || burst.data.stopped || burst.data.remaining <= 0)
  )
    throw new AppError(
      409,
      "The owner must continue this conversation before more agent work can be requested.",
    );
  const attach = pins(s, a, i.attachments);
  for (const pin of attach) {
    const r = s.get<Resource>(pin.resourceId, "resource");
    if (
      !r.data.conversationIds.includes(c.id) ||
      recipients.some((id) => !r.data.agentIds.includes(id))
    )
      throw new AppError(
        403,
        "An attachment is not shared with the recipients and conversation.",
      );
  }
  const message = s.add("message", {
    conversationId: c.id,
    authorId: a.id,
    body,
    replyTo: i.replyTo,
    attachments: attach,
    burstId: burst?.id,
    ...(a.requestId ? { requestId: a.requestId } : {}),
  });
  const snapshotIds = s
    .all<Message>("message")
    .filter((m) => m.data.conversationId === c.id && !m.data.staged)
    .map((m) => m.id);
  const requests = recipients.map((agentId) =>
    s.add("request", {
      conversationId: c.id,
      messageId: message.id,
      agentId,
      burstId: burst!.id,
      status: "pending",
      mode: "auto",
      reason: handedOff ? "Agent handoff" : "Agent mention",
      snapshotIds,
      generation: 1,
      ...(handedOff?.data.taskId ? { taskId: handedOff.data.taskId } : {}),
    }),
  );
  if (handedOff) {
    s.patch(handedOff.id, { status: "answered", leaseUntil: undefined });
    s.patch(handedOff.data.runId, { status: "completed", output: body });
    if (handedOff.data.taskId)
      s.patch(handedOff.data.taskId, {
        assigneeId: recipients[0],
        runId: undefined,
      });
  }
  return { message, requests };
}
