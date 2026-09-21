import { createHmac } from "node:crypto";
import { pool } from "../db.js";
import { transact } from "./commands.js";
import { readiness } from "./policy.js";
import type { Task } from "../../../../packages/shared/src/index.js";

export async function expireExternalClaims() {
  await transact((s) => {
    for (const req of s.all("request")) {
      if (
        !req.data.credentialId ||
        !["running", "claimed"].includes(req.data.status)
      )
        continue;
      if (Date.parse(req.data.leaseUntil || "") > Date.now()) continue;
      s.patch(req.id, {
        status: "needs_reconciliation",
        error:
          "Connection stopped checking in. Inspect its progress before retrying.",
      });
      if (req.data.runId)
        s.patch(req.data.runId, {
          status: "needs_reconciliation",
          error: "External claim lease expired; automatic replay withheld.",
        });
    }
  }, "connection.expiry");
}
export async function deliverWebhooks() {
  const tasks = (
    await transact(
      (s) =>
        s
          .all<Task>("task")
          .filter(
            (t) =>
              t.data.status === "ready" &&
              t.data.execution === "chat" &&
              t.data.assigneeId &&
              s.get(t.data.assigneeId, "agent").data.enabled &&
              s.get(t.data.assigneeId, "agent").data.deliveryMode ===
                "webhook" &&
              readiness(s, t).ready,
          )
          .map((t) => ({
            id: `${t.id}:${t.revision}`,
            request_id: t.id,
            generation: t.revision,
          })),
      "delivery.tasks",
    )
  ).value;
  if (tasks.length)
    await pool.query(
      `INSERT INTO webhook_deliveries(id,request_id,generation) SELECT id,request_id,generation FROM jsonb_to_recordset($1::jsonb) AS x(id text,request_id text,generation int) ON CONFLICT(id) DO NOTHING`,
      [JSON.stringify(tasks)],
    );
  const taskPending = tasks.length
    ? (
        await pool.query(
          `SELECT d.*,a.id agent_id,a.data->>'webhookUrl' url,secret.webhook_secret,true task_delivery FROM webhook_deliveries d JOIN records t ON t.id=d.request_id JOIN records a ON a.id=t.data->>'assigneeId' JOIN agent_connections secret ON secret.agent_id=a.id WHERE d.id=ANY($1) AND d.delivered_at IS NULL AND d.attempts<8 AND d.next_attempt_at<=now() ORDER BY d.next_attempt_at LIMIT 10`,
          [tasks.map((t) => t.id)],
        )
      ).rows
    : [];
  await pool.query(`INSERT INTO webhook_deliveries(id,request_id,generation)
    SELECT r.id || ':' || coalesce(r.data->>'generation','1'),r.id,coalesce((r.data->>'generation')::int,1)
    FROM records r JOIN records a ON a.id=r.data->>'agentId'
    JOIN records c ON c.id=r.data->>'conversationId' JOIN records b ON b.id=r.data->>'burstId'
    WHERE r.kind='request' AND r.data->>'status'='pending' AND a.data->>'deliveryMode'='webhook'
    AND a.data->>'enabled'='true' AND c.data->>'stopped'='false' AND b.data->>'stopped'='false'
    AND (b.data->>'remaining')::int>0 ON CONFLICT(id) DO NOTHING`);
  const pending =
    await pool.query(`SELECT d.*,a.id agent_id,a.data->>'webhookUrl' url,secret.webhook_secret
    FROM webhook_deliveries d JOIN records r ON r.id=d.request_id
    JOIN records a ON a.id=r.data->>'agentId' JOIN agent_connections secret ON secret.agent_id=a.id
    JOIN records c ON c.id=r.data->>'conversationId' JOIN records b ON b.id=r.data->>'burstId'
    WHERE d.delivered_at IS NULL AND d.attempts<8 AND d.next_attempt_at<=now()
    AND r.data->>'status'='pending' AND (r.data->>'generation')::int=d.generation
    AND a.data->>'deliveryMode'='webhook' AND a.data->>'enabled'='true'
    AND c.data->>'stopped'='false' AND b.data->>'stopped'='false' AND (b.data->>'remaining')::int>0
    ORDER BY d.next_attempt_at LIMIT 10`);
  await Promise.allSettled(
    [...pending.rows, ...taskPending].map(async (row) => {
      const body = JSON.stringify({
        id: row.id,
        event: "inbox.available",
        agentId: row.agent_id,
        ...(row.task_delivery
          ? { taskId: row.request_id }
          : { requestId: row.request_id }),
        generation: row.generation,
      });
      const timestamp = Math.floor(Date.now() / 1000).toString();
      const signature = createHmac("sha256", row.webhook_secret)
        .update(`${timestamp}.${body}`)
        .digest("hex");
      let error: string | undefined;
      try {
        const response = await fetch(row.url, {
          method: "POST",
          redirect: "error",
          signal: AbortSignal.timeout(8000),
          headers: {
            "Content-Type": "application/json",
            "X-Workspace-Delivery": row.id,
            "X-Workspace-Timestamp": timestamp,
            "X-Workspace-Signature": `sha256=${signature}`,
          },
          body,
        });
        if (!response.ok) error = `HTTP ${response.status}`;
        await response.body?.cancel();
      } catch {
        error = "Webhook delivery failed or timed out.";
      }
      const attempts = row.attempts + 1;
      await pool.query(
        `UPDATE webhook_deliveries SET attempts=$2,next_attempt_at=$3,delivered_at=$4,last_error=$5 WHERE id=$1`,
        [
          row.id,
          attempts,
          new Date(Date.now() + Math.min(300000, 5000 * 2 ** attempts)),
          error ? null : new Date(),
          error || null,
        ],
      );
      await transact((s) => {
        const a = s.get(row.agent_id, "agent");
        if (a.data.deliveryMode === "webhook")
          s.patch(a.id, {
            lastWakeupAt: new Date().toISOString(),
            lastWakeupStatus: error
              ? attempts >= 8
                ? "failed"
                : "retrying"
              : "delivered",
            lastWakeupError: error,
          });
      }, "connection.wakeup");
    }),
  );
}
