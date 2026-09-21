import { auditStorage } from "./storage/audit.js";
import Fastify from "fastify";
import cookie from "@fastify/cookie";
import multipart from "@fastify/multipart";
import fastifyStatic from "@fastify/static";
import { randomBytes, createHash, timingSafeEqual } from "node:crypto";
import { existsSync } from "node:fs";
import path from "node:path";
import { pool, setting } from "./db.js";
import { config, origin } from "./config.js";
import { execute, seed, transact, apply } from "./domain/commands.js";
import { AppError, hash } from "./domain/store.js";
import { readiness } from "./domain/policy.js";
import { storage } from "./storage/local.js";
import { github } from "./connectors/github.js";
import {
  authenticateAgent,
  executeUploadedResource,
  authorizedDownload,
  refreshAgentCredential,
} from "./domain/agent-access.js";
import { registerAgentHttp } from "./transport/agent-http.js";
import { registerMcp } from "./transport/mcp.js";
import { registerOAuth, postgresOAuthState } from "./transport/oauth.js";
const owner = { id: "owner", owner: true };
export async function buildServer() {
  const app = Fastify({
    trustProxy: config.trustProxy,
    logger: {
      level: "info",
      redact: [
        "req.headers.cookie",
        "req.headers.authorization",
        "req.headers.x-csrf-token",
      ],
    },
    bodyLimit: 1024 * 1024,
  });
  await app.register(cookie);
  app.addContentTypeParser(
    "application/x-www-form-urlencoded",
    { parseAs: "string" },
    (_req, body, done) => {
      const values = new URLSearchParams(String(body));
      const result: Record<string, string> = Object.create(null);
      for (const [key, value] of values) {
        if (Object.hasOwn(result, key)) {
          done(new AppError(400, "Duplicate form parameter."));
          return;
        }
        result[key] = value;
      }
      done(null, result);
    },
  );
  await app.register(multipart, {
    limits: { fileSize: 10 * 1024 * 1024, files: 1, fields: 12 },
  });
  app.setErrorHandler((error, req, reply) => {
    const e = error as any;
    const code = e.status || e.statusCode || 500;
    if (code >= 500) req.log.error({ err: e }, "Request failed");
    reply.code(code).send({
      error:
        code >= 500
          ? "The request failed. Check the local server log."
          : e.message,
    });
  });
  const loginAttempts = new Map<string, { count: number; until: number }>();
  app.addHook("onRequest", async (req, reply) => {
    reply
      .header("X-Content-Type-Options", "nosniff")
      .header("Referrer-Policy", "no-referrer")
      .header("X-Frame-Options", "DENY")
      .header(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'",
      );
    const hosts = config.allowedHosts;
    if (!hosts.includes(req.headers.host || ""))
      throw new AppError(403, "Invalid host.");
    const pathname = req.url.split("?")[0];
    const agentRoute =
      pathname === "/mcp" || pathname.startsWith("/api/agent/");
    if (agentRoute) {
      const requestOrigin = req.headers.origin;
      if (
        requestOrigin &&
        ![...config.allowedOrigins, ...config.agentOrigins].includes(
          requestOrigin,
        )
      )
        throw new AppError(403, "Invalid agent request origin.");
      if (requestOrigin)
        reply
          .header("Access-Control-Allow-Origin", requestOrigin)
          .header("Vary", "Origin")
          .header(
            "Access-Control-Expose-Headers",
            "WWW-Authenticate,Mcp-Protocol-Version,Mcp-Session-Id",
          );
      if (req.method === "OPTIONS")
        return reply
          .header("Access-Control-Allow-Methods", "GET,POST,DELETE,OPTIONS")
          .header(
            "Access-Control-Allow-Headers",
            "Authorization,Content-Type,Mcp-Protocol-Version,Mcp-Session-Id,Last-Event-Id",
          )
          .code(204)
          .send();
      if (
        req.method === "POST" &&
        !req.headers["content-type"]?.startsWith("application/json") &&
        !req.headers["content-type"]?.startsWith("multipart/form-data")
      )
        throw new AppError(415, "JSON or file upload required.");
      return;
    }
    if (
      pathname.startsWith("/.well-known/") ||
      pathname.startsWith("/oauth/")
    ) {
      if (
        req.headers.origin &&
        ![...config.allowedOrigins, ...config.agentOrigins].includes(
          req.headers.origin,
        ) &&
        !["GET", "HEAD"].includes(req.method)
      )
        throw new AppError(403, "Invalid authorization request origin.");
      return;
    }
    const hook = req.url === "/api/hooks/work";
    if (hook) {
      if (
        req.headers.origin &&
        !config.allowedOrigins.includes(req.headers.origin)
      )
        throw new AppError(403, "Invalid hook origin.");
      const bearer = req.headers.authorization?.replace(/^Bearer /, "") || "";
      if (!config.webhookTokenHash || hash(bearer) !== config.webhookTokenHash)
        throw new AppError(401, "Invalid work hook credential.");
      if (!req.headers["content-type"]?.startsWith("application/json"))
        throw new AppError(415, "JSON required.");
      return;
    }
    if (!["GET", "HEAD", "OPTIONS"].includes(req.method)) {
      if (!config.allowedOrigins.includes(req.headers.origin || ""))
        throw new AppError(403, "Invalid request origin.");
      if (
        !req.headers["content-type"]?.startsWith("application/json") &&
        !req.headers["content-type"]?.startsWith("multipart/form-data")
      )
        throw new AppError(415, "JSON or file upload required.");
    }
    if (req.url.split("?")[0] === "/api/login" || req.url === "/health") return;
    if (!req.url.startsWith("/api/")) return;
    const token = req.cookies.workspace;
    const result = token
      ? await pool.query(
          "SELECT csrf FROM owner_sessions WHERE hash=$1 AND expires_at>now()",
          [hash(token)],
        )
      : null;
    if (req.url === "/api/session") {
      (req as any).auth = result?.rows[0] || null;
      return;
    }
    if (!result?.rowCount)
      throw new AppError(401, "Sign in to your workspace.");
    (req as any).auth = result.rows[0];
    if (
      !["GET", "HEAD"].includes(req.method) &&
      req.headers["x-csrf-token"] !== result.rows[0].csrf
    )
      throw new AppError(403, "Reload the page before trying again.");
  });
  app.get("/health", async () => {
    await pool.query("SELECT 1");
    return {
      ok: true,
      workerOnline:
        Date.now() - Number(await setting("workerHeartbeat", 0)) < 15000,
      dispatchEnabled: await setting("dispatchEnabled", true),
    };
  });
  app.get("/api/session", async (req) => ({
    authenticated: !!(req as any).auth,
    csrf: (req as any).auth?.csrf,
  }));
  app.post("/api/login", async (req, reply) => {
    const key = req.ip,
      now = Date.now();
    const prior = loginAttempts.get(key);
    if (prior && prior.until > now && prior.count >= 10)
      throw new AppError(429, "Too many sign-in attempts. Wait one minute.");
    const value = (req.body as any)?.token;
    const digest = hash(typeof value === "string" ? value : "");
    if (digest !== config.ownerTokenHash) {
      loginAttempts.set(key, {
        count: prior && prior.until > now ? prior.count + 1 : 1,
        until: now + 60000,
      });
      throw new AppError(401, "The sign-in token is incorrect.");
    }
    loginAttempts.delete(key);
    const token = randomBytes(32).toString("base64url"),
      csrf = randomBytes(24).toString("base64url");
    await pool.query(
      "INSERT INTO owner_sessions(hash,csrf,expires_at) VALUES($1,$2,now()+interval '14 days')",
      [hash(token), csrf],
    );
    reply.setCookie("workspace", token, {
      httpOnly: true,
      sameSite: "lax",
      secure: config.secureCookies,
      path: "/",
      maxAge: 14 * 86400,
    });
    return { authenticated: true, csrf };
  });
  app.post("/api/logout", async (req, reply) => {
    await pool.query("DELETE FROM owner_sessions WHERE hash=$1", [
      hash(req.cookies.workspace || ""),
    ]);
    reply.clearCookie("workspace", { path: "/", secure: config.secureCookies });
    return { ok: true };
  });
  app.get("/api/snapshot", async () => {
    const lastSeen = new Map(
      (
        await pool.query(
          "SELECT agent_id,max(last_seen_at) last_seen_at FROM agent_credentials GROUP BY agent_id",
        )
      ).rows.map((row) => [row.agent_id, row.last_seen_at?.toISOString()]),
    );
    const snapshot = await transact(
      (s) =>
        [...s.rows.values()].map((r) => ({
          ...r,
          data: {
            ...r.data,
            ...(r.kind === "agent" ? { lastSeenAt: lastSeen.get(r.id) } : {}),
            ...(r.kind === "task" ? { readiness: readiness(s, r as any) } : {}),
          },
        })),
      "read",
    );
    return {
      records: snapshot.value,
      cursor: snapshot.cursor,
      dispatchEnabled: await setting("dispatchEnabled", true),
      workerOnline:
        Date.now() - Number(await setting("workerHeartbeat", 0)) < 15000,
      runtime: await setting("runtimeCapabilities", { qualified: false }),
      storage: await setting("storageHealth", null),
      quota: await setting("quota", { known: false }),
      github: await github.capabilities(),
    };
  });
  app.post("/api/hooks/work", async (req) => {
    const id = req.headers["idempotency-key"];
    if (typeof id !== "string" || id.length > 120)
      throw new AppError(400, "A stable Idempotency-Key is required.");
    return execute(owner, {
      id: `hook:${id}`,
      type: "session.start",
      input: { scopeId: (req.body as any)?.scopeId },
    });
  });
  app.post("/api/commands", async (req) => {
    const command = req.body as any;
    if (command?.input?.storageKey)
      throw new AppError(400, "Use the file upload endpoint.");
    return execute(owner, command);
  });
  app.post("/api/uploads", async (req) => {
    const part = await req.file();
    if (!part) throw new AppError(400, "Choose a file.");
    const bytes = await part.toBuffer();
    if (part.file.truncated)
      throw new AppError(413, "Files are limited to 10 MB.");
    const saved = await storage.put(bytes);
    const field = (name: string) => {
      const f = part.fields[name] as any;
      return f?.value;
    };
    let conversationIds: string[] = [];
    let projectIds: string[] = [];
    let spaceIds: string[] = [];
    let agentIds: string[] | undefined;
    let tags: string[] = [];
    try {
      conversationIds = JSON.parse(field("conversationIds") || "[]");
      projectIds = JSON.parse(field("projectIds") || "[]");
      spaceIds = JSON.parse(field("spaceIds") || "[]");
      agentIds =
        field("agentIds") === undefined
          ? undefined
          : JSON.parse(field("agentIds"));
      tags = JSON.parse(field("tags") || "[]");
    } catch {
      throw new AppError(400, "Invalid conversation selection.");
    }
    return execute(owner, {
      id: randomBytes(20).toString("hex"),
      type: "resource.create",
      input: {
        title: field("title") || part.filename,
        conversationIds,
        projectIds,
        spaceIds,
        agentIds,
        tags,
        pinned: field("pinned") === "true",
        storageKey: saved.key,
        size: saved.size,
        checksum: saved.checksum,
        filename: part.filename,
        mimeType: part.mimetype,
      },
    });
  });
  app.get(
    "/api/resources/:id/revisions/:revisionId/download",
    async (req, reply) => {
      const params = req.params as any;
      const result = await transact((s) => {
        const r = s.get(params.id, "resource"),
          rev = s.get(params.revisionId, "revision");
        if (rev.data.resourceId !== r.id || !rev.data.storageKey)
          throw new AppError(404, "File not found.");
        return rev.data;
      }, "read");
      const bytes = await storage.get(result.value.storageKey);
      if (
        createHash("sha256").update(bytes).digest("hex") !==
        result.value.checksum
      )
        throw new AppError(
          409,
          "File checksum mismatch. Preserve files and inspect storage.",
        );
      reply
        .type("application/octet-stream")
        .header(
          "Content-Disposition",
          `attachment; filename*=UTF-8''${encodeURIComponent(result.value.filename || "download")}`,
        );
      return bytes;
    },
  );
  app.get("/api/github/pull", async (req) =>
    github.readPull(String((req.query as any).url || "")),
  );
  const participantDependencies = {
    authenticate: authenticateAgent,
    execute,
    storage,
    upload: executeUploadedResource,
    download: async (
      actor: Parameters<typeof authorizedDownload>[0],
      id: string,
      revisionId: string,
    ) => {
      const revision = await authorizedDownload(actor, id, revisionId);
      if (!revision.storageKey) throw new AppError(404, "File not found.");
      return {
        storageKey: revision.storageKey,
        checksum: revision.checksum,
        filename: revision.filename,
      };
    },
  };
  await registerAgentHttp(app, participantDependencies);
  await registerMcp(app, participantDependencies, origin);
  app.options("/mcp", async (_req, reply) => reply.code(204).send());
  app.options("/api/agent/*", async (_req, reply) => reply.code(204).send());
  await registerOAuth(
    app,
    {
      state: postgresOAuthState(pool),
      ownerSession: async (req) => {
        const token = req.cookies.workspace;
        if (!token) return null;
        return (
          (
            await pool.query(
              "SELECT csrf FROM owner_sessions WHERE hash=$1 AND expires_at>now()",
              [hash(token)],
            )
          ).rows[0] || null
        );
      },
      agents: async () =>
        (
          await transact(
            (s) =>
              s
                .all("agent")
                .filter(
                  (a) =>
                    a.data.enabled &&
                    ["poll", "webhook"].includes(a.data.deliveryMode),
                )
                .map((a) => ({
                  id: a.id,
                  name: a.data.name,
                  enabled: a.data.enabled,
                })),
            "read",
          )
        ).value,
      issue: async (agentId, label, commandId) =>
        (
          await execute(owner, {
            id: commandId,
            type: "credential.issue",
            input: { agentId, label, expiresInDays: 1 },
          })
        ).value,
      refresh: refreshAgentCredential,
      revoke: async (id) => {
        await execute(owner, {
          id: `oauth-revoke:${id}`,
          type: "credential.revoke",
          input: { id },
        });
      },
    },
    origin,
  );
  app.get("/api/events", async (req, reply) => {
    let after = Number(
      req.headers["last-event-id"] || (req.query as any).after || 0,
    );
    if (!Number.isSafeInteger(after) || after < 0)
      throw new AppError(400, "Invalid event cursor.");
    reply.hijack();
    reply.raw.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    });
    let closed = false,
      busy = false;
    req.raw.on("close", () => {
      closed = true;
      clearInterval(timer);
    });
    async function send() {
      if (closed || busy) return;
      busy = true;
      try {
        const latest = Number(
          (await pool.query("SELECT coalesce(max(id),0) id FROM events"))
            .rows[0].id,
        );
        if (after > latest) {
          after = latest;
          reply.raw.write(`id: ${after}\nevent: reconcile\ndata: {}\n\n`);
        }
        const events = await pool.query(
          "SELECT id,type,ids FROM events WHERE id>$1 ORDER BY id LIMIT 101",
          [after],
        );
        if (events.rows.length > 100) {
          after = latest;
          reply.raw.write(`id: ${after}\nevent: reconcile\ndata: {}\n\n`);
        } else
          for (const e of events.rows) {
            after = Number(e.id);
            reply.raw.write(
              `id: ${after}\nevent: change\ndata: ${JSON.stringify({ type: e.type, ids: e.ids })}\n\n`,
            );
          }
        reply.raw.write(": heartbeat\n\n");
      } catch {
        reply.raw.end();
      } finally {
        busy = false;
      }
    }
    const timer = setInterval(send, 1500);
    await send();
  });
  const web = path.join(config.root, "apps/web/dist");
  if (existsSync(web)) {
    await app.register(fastifyStatic, { root: web });
    app.setNotFoundHandler((req, reply) =>
      req.url.startsWith("/api/")
        ? reply.code(404).send({ error: "Not found" })
        : reply.sendFile("index.html"),
    );
  } else
    app.get("/", async (_, reply) =>
      reply
        .type("text/html")
        .send("<p>Frontend has not been built. Run pnpm build.</p>"),
    );
  return app;
}
if (process.argv[1] && /server\.(ts|js)$/.test(process.argv[1])) {
  await seed();
  await auditStorage();
  const app = await buildServer();
  await app.listen({ host: config.bindHost, port: config.port });
  for (const signal of ["SIGINT", "SIGTERM"])
    process.on(signal, async () => {
      app.server.closeAllConnections();
      await app.close();
      await pool.end();
      process.exit(0);
    });
}
