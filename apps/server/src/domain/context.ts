import { revisionContent } from "../storage/content.js";
import type {
  ResponseRequest,
  Message,
  Resource,
  Run,
} from "../../../../packages/shared/src/index.js";
import { Store, hash, AppError } from "./store.js";
export async function responsePacket(s: Store, requestId: string) {
  const req = s.get<ResponseRequest>(requestId, "request"),
    c = s.get(req.data.conversationId, "conversation"),
    a = s.get(req.data.agentId, "agent");
  const all = req.data.snapshotIds
    .map((id) => s.get<Message>(id, "message"))
    .filter((m) => !m.data.staged);
  const recent = all.slice(-40);
  const request = s.get<Message>(req.data.messageId, "message");
  const resources = await Promise.all(
    request.data.attachments.map(async (pin) => {
      const r = s.get<Resource>(pin.resourceId, "resource");
      if (
        !r.data.agentIds.includes(a.id) ||
        !r.data.conversationIds.includes(c.id)
      )
        throw new AppError(
          403,
          `Required Resource ${r.data.title} is not shared with this conversation and agent.`,
        );
      const revision = s.get(pin.revisionId, "revision");
      const content = await revisionContent(revision.data);
      if (content.contentAvailability !== "text")
        throw new AppError(400, content.contentReason);
      return {
        resourceId: r.id,
        title: r.data.title,
        revisionId: revision.id,
        text: content.text,
        filename: revision.data.filename,
        checksum: revision.data.checksum,
      };
    }),
  );
  const packet = JSON.stringify({
    request: {
      id: request.id,
      text: request.data.body,
      revision: request.revision,
      reason: req.data.reason,
    },
    conversation: { id: c.id, title: c.data.title, mode: req.data.mode },
    recentMessages: recent.map((m) => ({
      id: m.id,
      author:
        m.data.authorId === "owner"
          ? "Owner"
          : s.rows.get(m.data.authorId)?.data.name || m.data.authorId,
      body: m.data.body,
      replyTo: m.data.replyTo,
      forwardedFrom: m.data.forwardedFrom,
      revision: m.revision,
    })),
    requiredResources: resources,
    contextCoverage: {
      brief: null,
      coveredMessageIds: recent.map((m) => m.id),
      omittedEarlierCount: all.length - recent.length,
      retrieval:
        "Use conversation.read with {beforeId,limit} to retrieve older permitted messages; it returns items and nextBeforeId. No generated brief is available.",
    },
    limits: {
      maxToolCalls: 24,
      allowedOperations: [
        "conversation.read",
        "resource.list",
        "resource.read",
        "resource.create",
        "resource.revise",
        "plan.propose",
        "task.list",
      ],
      autonomousAllowance:
        "This response consumes one reserved turn; tools cannot replenish the burst.",
    },
  });
  if (Buffer.byteLength(packet) > 180000)
    throw new AppError(
      413,
      "Required context is too large. Shorten the request or attach smaller Resource sections.",
    );
  return {
    packet,
    digest: hash(packet),
    manifest: {
      requestId: req.id,
      messageId: request.id,
      messageRevision: request.revision,
      conversationId: c.id,
      actorId: a.id,
      agentName: a.data.name,
      agentRole: a.data.role,
      intent: (req.data as any).intent || "conversation",
      sessionBinding: `${a.id}:${c.id}:${req.id}`,
      messageRevisions: recent.map((m) => ({ id: m.id, revision: m.revision })),
      resourceRevisions: resources.map((r) => r.revisionId),
      omittedEarlierCount: all.length - recent.length,
      briefCoverage: null,
      policyVersion: 1,
      providerContext:
        "Runtime system instructions are not fully observable; application context is rebuilt fresh.",
    },
  };
}
