import { randomBytes, randomUUID } from "node:crypto";
import { pool } from "../db.js";
import { AppError, hash, type Store } from "./store.js";
import { execute, transact } from "./commands.js";
import { resourceAccess, type Actor } from "./policy.js";
import type {
  Command,
  ResourceRevision,
} from "../../../../packages/shared/src/index.js";

export async function authenticateAgent(token: string): Promise<Actor> {
  if (!token || token.length > 512)
    throw new AppError(401, "Agent credential required.");
  const result = await pool.query(
    `UPDATE agent_credentials c SET last_seen_at=now() FROM records a
     WHERE c.hash=$1 AND c.revoked_at IS NULL AND c.expires_at>now()
     AND a.id=c.agent_id AND a.kind='agent' AND a.data->>'enabled'='true'
     RETURNING c.id,c.agent_id`,
    [hash(token)],
  );
  if (!result.rowCount)
    throw new AppError(401, "Agent credential expired, revoked, or disabled.");
  const row = result.rows[0];
  return { id: row.agent_id, participant: true, credentialId: row.id };
}
export async function validateParticipant(s: Store, actor: Actor) {
  const result = await s.client.query(
    `SELECT id FROM agent_credentials WHERE id=$1 AND agent_id=$2 AND revoked_at IS NULL AND expires_at>now()`,
    [actor.credentialId, actor.id],
  );
  if (!result.rowCount || !s.get(actor.id, "agent").data.enabled)
    throw new AppError(401, "Agent credential expired, revoked, or disabled.");
}
const metadata = (r: any) => ({
  id: r.id,
  agentId: r.agent_id,
  label: r.label,
  createdAt: r.created_at?.toISOString(),
  expiresAt: r.expires_at?.toISOString(),
  revokedAt: r.revoked_at?.toISOString(),
  lastSeenAt: r.last_seen_at?.toISOString(),
});
export async function credentialCommand(s: Store, type: string, i: any) {
  if (type === "credential.issue") {
    s.get(i.agentId, "agent");
    const days = i.expiresInDays === undefined ? 90 : Number(i.expiresInDays);
    if (!Number.isInteger(days) || days < 0 || days > 365)
      throw new AppError(400, "Expiration must be 0 (until revoked) or 1–365 days.");
    const label = String(i.label || "Agent connection").trim();
    if (!label || label.length > 120)
      throw new AppError(400, "Connection label must be 1–120 characters.");
    const token = `aw_${randomBytes(32).toString("base64url")}`;
    // PostgreSQL keeps a concrete timestamp so existing authentication queries
    // still apply. Year 9999 represents an owner-revocable, non-expiring grant.
    const expiresAt = days === 0
      ? new Date("9999-12-31T00:00:00.000Z")
      : new Date(Date.now() + days * 86400000);
    const row = (
      await s.client.query(
        `INSERT INTO agent_credentials(id,hash,agent_id,label,expires_at) VALUES($1,$2,$3,$4,$5) RETURNING *`,
        [randomUUID(), hash(token), i.agentId, label, expiresAt],
      )
    ).rows[0];
    return { ...metadata(row), token };
  }
  if (type === "credential.list") {
    s.get(i.agentId, "agent");
    const rows = await s.client.query(
      "SELECT * FROM agent_credentials WHERE agent_id=$1 ORDER BY created_at DESC",
      [i.agentId],
    );
    return { credentials: rows.rows.map(metadata) };
  }
  if (type === "credential.revoke") {
    const result = await s.client.query(
      "UPDATE agent_credentials SET revoked_at=coalesce(revoked_at,now()) WHERE id=$1 RETURNING *",
      [i.id],
    );
    if (!result.rowCount) throw new AppError(404, "Connection not found.");
    for (const request of s
      .all("request")
      .filter(
        (r) =>
          r.data.credentialId === i.id &&
          ["running", "claimed"].includes(r.data.status),
      )) {
      s.patch(request.id, {
        status: "needs_reconciliation",
        error: "Connection revoked before completion.",
      });
      if (request.data.runId)
        s.patch(request.data.runId, {
          stopRequested: true,
          status: "needs_reconciliation",
          error: "Connection revoked before completion.",
        });
    }
    return metadata(result.rows[0]);
  }
}
export async function configureAgent(s: Store, i: any) {
  const agent = s.get(i.id, "agent");
  if (!["managed", "poll", "webhook"].includes(i.deliveryMode))
    throw new AppError(400, "Choose managed, poll, or webhook delivery.");
  const minutes = Number(i.checkInMinutes ?? agent.data.checkInMinutes ?? 60);
  if (!Number.isInteger(minutes) || minutes < 1 || minutes > 10080)
    throw new AppError(400, "Check-in interval must be 1–10080 minutes.");
  const url = String(i.webhookUrl ?? agent.data.webhookUrl ?? "").trim();
  if (i.deliveryMode === "webhook") {
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      throw new AppError(400, "A webhook URL is required.");
    }
    if (
      !["https:", "http:"].includes(parsed.protocol) ||
      parsed.username ||
      parsed.password ||
      parsed.hash
    )
      throw new AppError(
        400,
        "Use an HTTP(S) webhook URL without credentials or a fragment.",
      );
    if (
      parsed.protocol === "http:" &&
      !["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname)
    )
      throw new AppError(400, "Remote webhooks require HTTPS.");
  }
  if (i.webhookSecret !== undefined) {
    const secret = String(i.webhookSecret);
    if (secret.length < 16 || secret.length > 512)
      throw new AppError(
        400,
        "Webhook signing secret must be 16–512 characters.",
      );
    await s.client.query(
      "INSERT INTO agent_connections(agent_id,webhook_secret) VALUES($1,$2) ON CONFLICT(agent_id) DO UPDATE SET webhook_secret=excluded.webhook_secret",
      [agent.id, secret],
    );
  }
  const configured = !!(
    await s.client.query(
      "SELECT agent_id FROM agent_connections WHERE agent_id=$1",
      [agent.id],
    )
  ).rowCount;
  if (i.deliveryMode === "webhook" && !configured)
    throw new AppError(
      400,
      "Add a webhook signing secret before enabling wakeups.",
    );
  if (
    (agent.data.deliveryMode || "managed") !== i.deliveryMode &&
    s
      .all("request")
      .some(
        (r) =>
          r.data.agentId === agent.id &&
          ["running", "claimed", "needs_reconciliation"].includes(
            r.data.status,
          ),
      )
  )
    throw new AppError(
      409,
      "Resolve active or uncertain work before changing this agent's delivery mode.",
    );
  return s.patch(
    agent.id,
    {
      deliveryMode: i.deliveryMode,
      checkInMinutes: minutes,
      webhookUrl: url || undefined,
      webhookConfigured: configured,
    },
    i.expectedRevision,
  );
}
export async function executeUploadedResource(
  actor: Actor,
  command: Command,
  blob: NonNullable<Actor["uploadedBlob"]>,
) {
  if (command.type !== "resource.create")
    throw new AppError(400, "Upload must create a Resource.");
  return execute(
    { ...actor, uploadedBlob: blob },
    { ...command, input: { ...command.input, ...blob } },
  );
}
export async function authorizedDownload(
  actor: Actor,
  resourceId: string,
  revisionId?: string,
): Promise<ResourceRevision> {
  return (
    await transact(async (s) => {
      if (actor.participant) await validateParticipant(s, actor);
      const resource = resourceAccess(s, actor, resourceId);
      const revision = s.get<ResourceRevision>(
        revisionId || resource.data.latestRevisionId,
        "revision",
      );
      if (revision.data.resourceId !== resource.id)
        throw new AppError(404, "Revision not found.");
      if (actor.requestId) {
        const req = s.get(actor.requestId, "request");
        if (
          req.data.mode === "independent" &&
          resource.data.stagedFor !== req.id &&
          req.data.resourcePins?.[resource.id] !== revision.id
        )
          throw new AppError(
            404,
            "Revision is outside this independent snapshot.",
          );
      }
      return revision.data;
    }, "resource.download")
  ).value;
}

/** Called only by the OAuth transport after consuming a client-bound refresh grant. */
export async function refreshAgentCredential(credentialId: string) {
  return (
    await transact(async (s) => {
      const row = (
        await s.client.query(
          "SELECT * FROM agent_credentials WHERE id=$1 AND revoked_at IS NULL",
          [credentialId],
        )
      ).rows[0];
      if (!row || !s.get(row.agent_id, "agent").data.enabled)
        throw new AppError(401, "Connection revoked or disabled.");
      const token = `aw_${randomBytes(32).toString("base64url")}`,
        expiresAt = new Date(Date.now() + 86400000);
      await s.client.query(
        "UPDATE agent_credentials SET hash=$2,expires_at=$3 WHERE id=$1",
        [credentialId, hash(token), expiresAt],
      );
      return { id: credentialId, token, expiresAt: expiresAt.toISOString() };
    }, "credential.refresh")
  ).value;
}
