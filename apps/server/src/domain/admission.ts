import { responsePacket } from "./context.js";
import { AppError, type Store } from "./store.js";
import type {
  ResponseRequest,
  Run,
} from "../../../../packages/shared/src/index.js";
export async function claimRequest(
  s: Store,
  id: string,
  external?: { credentialId: string },
) {
  const req = s.get<ResponseRequest>(id, "request");
  if (req.data.status !== "pending") return null;
  const agent = s.get(req.data.agentId, "agent");
  if (
    external
      ? (agent.data.deliveryMode || "managed") === "managed"
      : (agent.data.deliveryMode || "managed") !== "managed"
  )
    return null;
  if (external && req.data.mode === "independent")
    throw new AppError(
      409,
      "This connection cannot guarantee independent context isolation. Use a managed agent or a normal request.",
    );
  const c = s.get(req.data.conversationId, "conversation"),
    b = s.get(req.data.burstId, "burst");
  if (
    !s.get(req.data.agentId, "agent").data.enabled ||
    c.data.stopped ||
    b.data.stopped ||
    !c.data.agentIds.includes(req.data.agentId)
  ) {
    s.patch(id, { status: "cancelled" });
    return null;
  }
  if (b.data.remaining <= 0) return null;
  if (req.data.mode === "discuss") {
    const preceding = b.data.participants.slice(
      0,
      b.data.participants.indexOf(req.data.agentId),
    );
    if (
      s
        .all<ResponseRequest>("request")
        .some(
          (r) =>
            r.data.burstId === b.id &&
            preceding.includes(r.data.agentId) &&
            ["pending", "claimed", "running", "needs_reconciliation"].includes(
              r.data.status,
            ),
        )
    )
      return null;
    const snapshotIds = [
      ...new Set([
        ...req.data.snapshotIds,
        ...s
          .all("message")
          .filter((m) => m.data.burstId === b.id && !m.data.staged)
          .map((m) => m.id),
      ]),
    ];
    s.patch(req.id, { snapshotIds });
  }
  if (req.data.mode === "auto") {
    s.patch(req.id, {
      snapshotIds: s
        .all("message")
        .filter((m) => m.data.conversationId === c.id && !m.data.staged)
        .map((m) => m.id),
    });
  }
  if (
    s
      .all<Run>("run")
      .some(
        (r) =>
          r.data.agentId === req.data.agentId &&
          r.data.manifest?.conversationId === c.id &&
          [
            "prepared",
            "dispatching",
            "running",
            "needs_reconciliation",
          ].includes(r.data.status),
      )
  )
    return null;
  try {
    const context = await responsePacket(s, id);
    const run = s.add("run", {
      kind: "response",
      requestId: id,
      ...(req.data.taskId ? { taskId: req.data.taskId } : {}),
      agentId: req.data.agentId,
      status: external ? "running" : "prepared",
      ...(external
        ? {
            runtime: {
              delivery: "external",
              credentialId: external.credentialId,
            },
          }
        : {}),
      ...context,
      events: [],
    });
    s.patch(b.id, { remaining: b.data.remaining - 1 });
    s.patch(id, {
      status: "running",
      runId: run.id,
      ...(external
        ? {
            credentialId: external.credentialId,
            leaseUntil: new Date(Date.now() + 15 * 60000).toISOString(),
          }
        : {}),
    });
    return run;
  } catch (e: any) {
    s.patch(id, { status: "failed", error: e.message });
    return null;
  }
}
