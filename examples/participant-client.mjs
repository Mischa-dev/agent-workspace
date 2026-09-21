import { createHmac, timingSafeEqual, randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

export class WorkspaceClient {
  constructor(origin, token) {
    this.origin = new URL(origin).origin;
    this.token = token;
  }
  async command(type, input = {}, options = {}) {
    if (!options.id)
      throw new Error(
        "Supply a stable command id; retain it when retrying the same action.",
      );
    const response = await fetch(`${this.origin}/api/agent/commands`, {
      method: "POST",
      redirect: "error",
      signal: AbortSignal.timeout(15000),
      headers: {
        Authorization: `Bearer ${this.token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        id: options.id,
        type,
        input,
        ...(options.requestId ? { requestId: options.requestId } : {}),
      }),
    });
    const result = await response.json();
    if (!response.ok)
      throw new Error(`Workspace rejected the command (${response.status}).`);
    return result.value;
  }
}
export function verifyWakeup(rawBody, headers, secret, now = Date.now()) {
  const timestamp = headers["x-workspace-timestamp"],
    signature = headers["x-workspace-signature"],
    delivery = headers["x-workspace-delivery"];
  if (
    typeof timestamp !== "string" ||
    !/^\d{10,12}$/.test(timestamp) ||
    Math.abs(now - Number(timestamp) * 1000) > 300000 ||
    typeof signature !== "string" ||
    !/^sha256=[a-f0-9]{64}$/.test(signature) ||
    typeof delivery !== "string"
  )
    return null;
  const expected = createHmac("sha256", secret)
    .update(timestamp + ".")
    .update(rawBody)
    .digest();
  if (!timingSafeEqual(expected, Buffer.from(signature.slice(7), "hex")))
    return null;
  let event;
  try {
    event = JSON.parse(rawBody.toString("utf8"));
  } catch {
    return null;
  }
  if (
    event.event !== "inbox.available" ||
    event.id !== delivery ||
    typeof event.agentId !== "string" ||
    (typeof event.requestId === "string") ===
      (typeof event.taskId === "string") ||
    !Number.isInteger(event.generation) ||
    event.generation < 1
  )
    return null;
  return event;
}

async function main() {
  const origin = process.env.WORKSPACE_URL,
    token = process.env.WORKSPACE_AGENT_TOKEN;
  if (!origin || !token)
    throw new Error(
      "Set WORKSPACE_URL and WORKSPACE_AGENT_TOKEN through your private environment.",
    );
  const client = new WorkspaceClient(origin, token);
  const me = await client.command("agent.me", {}, { id: randomUUID() });
  let checking = false;
  async function check() {
    if (checking) return;
    checking = true;
    try {
      const inbox = await client.command(
        "inbox.list",
        {},
        { id: randomUUID() },
      );
      // Hand these IDs to your own model/runtime. This example observes; it does not
      // claim work, fabricate replies, or create a hidden agent execution loop.
      console.log(
        `Inbox page checked: ${(inbox.requests || []).length} request(s), ${(inbox.tasks || []).length} task(s).`,
      );
    } catch {
      console.error(
        "Inbox check failed; the next bounded poll will try again.",
      );
    } finally {
      checking = false;
    }
  }
  await check();
  const mode = process.argv[2] || "once";
  if (mode === "once") return;
  if (!["poll", "wake"].includes(mode))
    throw new Error("Use once, poll, or wake.");
  const intervalSeconds = Number(
    process.env.WORKSPACE_POLL_SECONDS ||
      Math.max(60, Number(me.agent?.data?.checkInMinutes || 60) * 60),
  );
  if (
    !Number.isFinite(intervalSeconds) ||
    intervalSeconds < 60 ||
    intervalSeconds > 604800
  )
    throw new Error("Polling interval must be 60–604800 seconds.");
  const wakeSecret = process.env.WORKSPACE_WAKE_SECRET;
  if (mode === "wake" && (!wakeSecret || wakeSecret.length < 16))
    throw new Error("Set a webhook signing secret of at least 16 characters.");
  const timer = setInterval(() => void check(), intervalSeconds * 1000);
  let server;
  if (mode === "wake") {
    const secret = wakeSecret;
    const seen = new Map();
    server = createServer(async (req, res) => {
      try {
        if (req.method !== "POST" || req.url !== "/wake") {
          res.writeHead(404).end();
          return;
        }
        const chunks = [];
        let size = 0;
        for await (const chunk of req) {
          size += chunk.length;
          if (size > 16384) {
            res.writeHead(413).end();
            return;
          }
          chunks.push(chunk);
        }
        const event = verifyWakeup(Buffer.concat(chunks), req.headers, secret);
        if (!event || event.agentId !== me.agent?.id) {
          res.writeHead(401).end();
          return;
        }
        const now = Date.now();
        for (const [id, expires] of seen) if (expires < now) seen.delete(id);
        if (!seen.has(event.id)) {
          if (seen.size >= 10000) {
            res.writeHead(429).end();
            return;
          }
          seen.set(event.id, now + 900000);
          void check();
        }
        res.writeHead(202).end();
      } catch {
        if (!res.headersSent) res.writeHead(400);
        res.end();
      }
    });
    server.listen(Number(process.env.WORKSPACE_WAKE_PORT || 4315), "127.0.0.1");
    console.log(
      "Signed wakeup receiver listening on loopback /wake; fallback polling remains active.",
    );
  }
  for (const signal of ["SIGINT", "SIGTERM"])
    process.once(signal, () => {
      clearInterval(timer);
      server?.close();
    });
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
