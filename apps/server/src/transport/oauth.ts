import OAuth2Server from "@node-oauth/oauth2-server";
import {
  randomBytes,
  randomUUID,
  createHash,
  timingSafeEqual,
} from "node:crypto";
import type { FastifyInstance, FastifyRequest } from "fastify";

const scope = "workspace:participate";
const digest = (value: string) =>
  createHash("sha256").update(value).digest("hex");
const secret = () => randomBytes(32).toString("base64url");
const escape = (value: unknown) =>
  String(value).replace(
    /[&<>"']/g,
    (char) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        char
      ]!,
  );
export interface OAuthState {
  get(kind: string, id: string): Promise<any | null>;
  put(
    kind: string,
    id: string,
    payload: unknown,
    expiresAt: Date,
  ): Promise<void>;
  consume(kind: string, id: string): Promise<any | null>;
  count(kind: string): Promise<number>;
}
export function postgresOAuthState(database: {
  query: (query: string, values?: any[]) => Promise<any>;
}): OAuthState {
  return {
    get: async (kind, id) =>
      (
        await database.query(
          "SELECT payload FROM oauth_state WHERE kind=$1 AND id=$2 AND expires_at>now()",
          [kind, id],
        )
      ).rows[0]?.payload ?? null,
    put: async (kind, id, payload, expiresAt) => {
      await database.query("DELETE FROM oauth_state WHERE expires_at<=now()");
      await database.query(
        "INSERT INTO oauth_state(kind,id,payload,expires_at) VALUES($1,$2,$3,$4)",
        [kind, id, JSON.stringify(payload), expiresAt],
      );
    },
    consume: async (kind, id) =>
      (
        await database.query(
          "DELETE FROM oauth_state WHERE kind=$1 AND id=$2 AND expires_at>now() RETURNING payload",
          [kind, id],
        )
      ).rows[0]?.payload ?? null,
    count: async (kind) =>
      Number(
        (
          await database.query(
            "SELECT count(*) n FROM oauth_state WHERE kind=$1 AND expires_at>now()",
            [kind],
          )
        ).rows[0].n,
      ),
  };
}
export interface OAuthDependencies {
  state: OAuthState;
  ownerSession(req: FastifyRequest): Promise<{ csrf: string } | null>;
  agents(): Promise<{ id: string; name: string; enabled: boolean }[]>;
  issue(
    agentId: string,
    label: string,
    commandId: string,
  ): Promise<{ id: string; token?: string; expiresAt: string }>;
  refresh(
    credentialId: string,
  ): Promise<{ id: string; token: string; expiresAt: string }>;
  revoke(credentialId: string): Promise<void>;
}
function validRedirect(value: unknown): value is string {
  if (typeof value !== "string" || value.length > 2048) return false;
  try {
    const url = new URL(value);
    return (
      !url.username &&
      !url.password &&
      !url.hash &&
      (url.protocol === "https:" ||
        (url.protocol === "http:" &&
          ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)))
    );
  } catch {
    return false;
  }
}
export async function registerOAuth(
  app: FastifyInstance,
  deps: OAuthDependencies,
  origin: string,
) {
  const resource = `${origin}/mcp`;
  // Refresh grants remain usable until revoked, including after a long pause.
  // The access token still rotates daily; PostgreSQL stores a concrete date.
  const refreshTokenLifetime = Math.floor(
    (Date.UTC(9999, 11, 31) - Date.now()) / 1000,
  );
  const model: any = {
    getClient: async (id: string, provided?: string) => {
      const client = await deps.state.get("client", id);
      if (!client) return false;
      if (
        provided &&
        (!client.secretHash ||
          !timingSafeEqual(
            Buffer.from(digest(provided)),
            Buffer.from(client.secretHash),
          ))
      )
        return false;
      return {
        id,
        redirectUris: client.redirect_uris,
        grants: client.grant_types,
        accessTokenLifetime: 86400,
        refreshTokenLifetime,
        clientName: client.client_name,
      };
    },
    validateScope: async (_user: any, _client: any, scopes: string[]) =>
      !scopes?.length || scopes.every((s) => s === scope) ? [scope] : false,
    getAccessToken: async () => false,
    generateAuthorizationCode: async () => secret(),
    saveAuthorizationCode: async (code: any, client: any, user: any) => {
      const saved = {
        ...code,
        authorizationCode: undefined,
        client,
        user: { ...user, codeId: digest(code.authorizationCode) },
      };
      await deps.state.put(
        "code",
        digest(code.authorizationCode),
        saved,
        code.expiresAt,
      );
      return { ...saved, authorizationCode: code.authorizationCode };
    },
    getAuthorizationCode: async (code: string) => {
      const stored = await deps.state.get("code", digest(code));
      return stored
        ? {
            ...stored,
            authorizationCode: code,
            expiresAt: new Date(stored.expiresAt),
          }
        : false;
    },
    revokeAuthorizationCode: async (code: any) =>
      !!(await deps.state.consume("code", digest(code.authorizationCode))),
    generateAccessToken: async (client: any, user: any) => {
      if (
        !(await deps.agents()).some(
          (agent) => agent.id === user.agentId && agent.enabled,
        )
      )
        throw new Error("Agent is unavailable");
      if (user.credentialId)
        return (await deps.refresh(user.credentialId)).token;
      const issued = await deps.issue(
        user.agentId,
        `OAuth: ${String(client.clientName || client.id).slice(0, 100)}`,
        `oauth:${user.codeId}`,
      );
      if (!issued.token)
        throw new Error("This authorization code has already been used");
      user.credentialId = issued.id;
      return issued.token;
    },
    generateRefreshToken: async () => secret(),
    getRefreshToken: async (refreshToken: string) => {
      const saved = await deps.state.get("refresh", digest(refreshToken));
      return saved
        ? {
            ...saved,
            refreshToken,
            refreshTokenExpiresAt: new Date(saved.refreshTokenExpiresAt),
          }
        : false;
    },
    revokeToken: async (token: any) => {
      const saved = await deps.state.consume(
        "refresh",
        digest(token.refreshToken),
      );
      if (!saved) return false;
      await deps.state.put(
        "refresh-used",
        digest(token.refreshToken),
        { credentialId: saved.user.credentialId, clientId: saved.client.id },
        new Date(Date.now() + 30 * 86400000),
      );
      return true;
    },
    saveToken: async (token: any, client: any, user: any) => {
      const refresh = client.grants.includes("refresh_token");
      if (refresh)
        await deps.state.put(
          "refresh",
          digest(token.refreshToken),
          {
            refreshTokenExpiresAt: token.refreshTokenExpiresAt,
            scope: [scope],
            client,
            user,
          },
          token.refreshTokenExpiresAt,
        );
      return {
        accessToken: token.accessToken,
        accessTokenExpiresAt: token.accessTokenExpiresAt,
        scope: [scope],
        client,
        user,
        ...(refresh
          ? {
              refreshToken: token.refreshToken,
              refreshTokenExpiresAt: token.refreshTokenExpiresAt,
            }
          : {}),
      };
    },
  };
  // 5.3.0 implements requirePKCE/enablePlainPKCE; its declaration file omits these options.
  const oauth = new OAuth2Server({
    model,
    requirePKCE: true,
    enablePlainPKCE: false,
    authorizationCodeLifetime: 300,
    accessTokenLifetime: 86400,
    refreshTokenLifetime,
    alwaysIssueNewRefreshToken: true,
    requireClientAuthentication: {
      authorization_code: false,
      refresh_token: false,
    },
  } as any);
  const metadata = {
    issuer: origin,
    authorization_endpoint: `${origin}/oauth/authorize`,
    token_endpoint: `${origin}/oauth/token`,
    registration_endpoint: `${origin}/oauth/register`,
    response_types_supported: ["code"],
    grant_types_supported: ["authorization_code", "refresh_token"],
    token_endpoint_auth_methods_supported: [
      "none",
      "client_secret_post",
      "client_secret_basic",
    ],
    code_challenge_methods_supported: ["S256"],
    scopes_supported: [scope],
  };
  app.get("/.well-known/oauth-authorization-server", async (_req, reply) =>
    reply.header("Access-Control-Allow-Origin", "*").send(metadata),
  );
  for (const url of [
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/mcp",
  ])
    app.get(url, async (_req, reply) =>
      reply.header("Access-Control-Allow-Origin", "*").send({
        resource,
        authorization_servers: [origin],
        scopes_supported: [scope],
        bearer_methods_supported: ["header"],
      }),
    );
  const registrations = new Map<string, { count: number; until: number }>();
  app.post("/oauth/register", async (req, reply) => {
    const now = Date.now();
    for (const [ip, entry] of registrations)
      if (entry.until < now) registrations.delete(ip);
    const prior = registrations.get(req.ip);
    if (
      (prior?.count ?? 0) >= 20 ||
      registrations.size > 10000 ||
      (await deps.state.count("client")) >= 1000
    )
      return reply.code(429).send({ error: "temporarily_unavailable" });
    registrations.set(req.ip, {
      count: (prior?.count ?? 0) + 1,
      until: prior?.until ?? now + 86400000,
    });
    const body = req.body as any,
      method = body?.token_endpoint_auth_method ?? "client_secret_basic";
    if (
      !Array.isArray(body?.redirect_uris) ||
      body.redirect_uris.length < 1 ||
      body.redirect_uris.length > 5 ||
      !body.redirect_uris.every(validRedirect) ||
      !["none", "client_secret_basic", "client_secret_post"].includes(method) ||
      (body.grant_types &&
        (!Array.isArray(body.grant_types) ||
          !body.grant_types.includes("authorization_code") ||
          body.grant_types.some(
            (g: unknown) =>
              !["authorization_code", "refresh_token"].includes(String(g)),
          ))) ||
      (body.response_types &&
        (!Array.isArray(body.response_types) ||
          body.response_types.some((g: unknown) => g !== "code")))
    )
      return reply.code(400).send({ error: "invalid_client_metadata" });
    const id = randomUUID(),
      clientSecret = method === "none" ? undefined : secret();
    const client = {
      client_id: id,
      client_name:
        typeof body.client_name === "string"
          ? body.client_name.slice(0, 120)
          : "MCP client",
      redirect_uris: body.redirect_uris,
      token_endpoint_auth_method: method,
      grant_types: body.grant_types || ["authorization_code", "refresh_token"],
      response_types: ["code"],
      scope,
      client_id_issued_at: Math.floor(now / 1000),
    };
    await deps.state.put(
      "client",
      id,
      {
        ...client,
        ...(clientSecret ? { secretHash: digest(clientSecret) } : {}),
      },
      new Date(now + 365 * 86400000),
    );
    return reply
      .code(201)
      .header("Cache-Control", "no-store")
      .send({
        ...client,
        ...(clientSecret
          ? { client_secret: clientSecret, client_secret_expires_at: 0 }
          : {}),
      });
  });
  const page = (content: string) =>
    `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Connect an agent · Workspace</title></head><body><main><h1>Connect an agent</h1>${content}<p>This connection cannot approve execution, accept tasks, or administer your workspace. You can revoke its credential in Agents.</p></main><script src="/oauth/consent.js" defer></script></body></html>`;
  app.get("/oauth/consent.js", async (_req, reply) =>
    reply
      .type("application/javascript")
      .send(
        `document.querySelector('form')?.addEventListener('submit',async event=>{event.preventDefault();const form=event.target;const output=document.querySelector('[role=status]');output.textContent='Connecting…';try{if(form.dataset.login){const r=await fetch('/api/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token:form.elements.token.value})});if(!r.ok)throw new Error('Sign-in failed');location.reload();return;}const r=await fetch('/api/oauth/consent',{method:'POST',headers:{'Content-Type':'application/json','X-CSRF-Token':form.dataset.csrf},body:JSON.stringify({consentId:form.dataset.consent,agentId:form.elements.agentId.value,allow:event.submitter?.value==='allow'})});const value=await r.json();if(!r.ok)throw new Error(value.error||'Connection failed');location.assign(value.redirect);}catch(error){output.textContent=error.message;}});`,
      ),
  );
  app.get("/oauth/authorize", async (req, reply) => {
    reply.header("Cache-Control", "no-store");
    const query = req.query as any;
    const client =
      typeof query.client_id === "string"
        ? await deps.state.get("client", query.client_id)
        : null;
    if (
      !client ||
      !client.redirect_uris.includes(query.redirect_uri) ||
      query.response_type !== "code" ||
      query.code_challenge_method !== "S256" ||
      typeof query.code_challenge !== "string" ||
      !/^[A-Za-z0-9_-]{43}$/.test(query.code_challenge) ||
      typeof query.state !== "string" ||
      query.state.length < 1 ||
      query.state.length > 1024 ||
      (query.resource !== undefined && query.resource !== resource) ||
      (query.scope !== undefined && query.scope !== scope)
    )
      return reply.code(400).send({
        error: "invalid_request",
        error_description:
          "A registered redirect, state, resource and S256 PKCE challenge are required.",
      });
    const session = await deps.ownerSession(req);
    if (!session)
      return reply
        .type("text/html")
        .send(
          page(
            '<p>Sign in with your workspace owner token to review this connection.</p><form data-login="true"><label>Owner token <input name="token" type="password" required autocomplete="current-password"></label><button>Sign in</button></form><p role="status"></p>',
          ),
        );
    const agents = (await deps.agents()).filter((agent) => agent.enabled);
    if (!agents.length)
      return reply
        .type("text/html")
        .send(
          page(
            '<p>Create a connected agent in Workspace first. Choose poll or webhook delivery, then return here to connect this client.</p><p><a href="/" target="_blank" rel="noopener">Open Workspace</a></p>',
          ),
        );
    const consent = secret();
    await deps.state.put(
      "consent",
      digest(consent),
      {
        query: {
          client_id: query.client_id,
          redirect_uri: query.redirect_uri,
          response_type: "code",
          code_challenge: query.code_challenge,
          code_challenge_method: "S256",
          state: query.state,
          scope,
          resource,
        },
        csrfHash: digest(session.csrf),
      },
      new Date(Date.now() + 600000),
    );
    const choices = agents
      .map((a) => `<option value="${escape(a.id)}">${escape(a.name)}</option>`)
      .join("");
    return reply
      .type("text/html")
      .send(
        page(
          `<p><strong>${escape(client.client_name)}</strong> wants to participate as an agent. Access credentials last one day.${client.grant_types.includes("refresh_token") ? " This client may renew the connection using a rotating refresh credential until you revoke it." : ""}</p><p>Return address: ${escape(query.redirect_uri)}</p><form data-consent="${escape(consent)}" data-csrf="${escape(session.csrf)}"><label>Agent <select name="agentId" required>${choices}</select></label><button value="allow">Allow connection</button><button value="deny">Deny</button></form><p role="status"></p>`,
        ),
      );
  });
  app.post("/api/oauth/consent", async (req, reply) => {
    const session = await deps.ownerSession(req);
    if (!session || req.headers["x-csrf-token"] !== session.csrf)
      return reply.code(403).send({ error: "Owner consent required." });
    const body = req.body as any;
    if (typeof body?.consentId !== "string")
      return reply.code(400).send({ error: "Consent request required." });
    const pending = await deps.state.get("consent", digest(body.consentId));
    if (!pending || pending.csrfHash !== digest(session.csrf))
      return reply.code(400).send({ error: "Consent request expired." });
    if (
      body.allow === true &&
      !(await deps.agents()).some(
        (agent) => agent.id === body.agentId && agent.enabled,
      )
    )
      return reply.code(400).send({ error: "Select an enabled agent." });
    if (!(await deps.state.consume("consent", digest(body.consentId))))
      return reply.code(400).send({ error: "Consent already used." });
    if (body.allow !== true) {
      const redirect = new URL(pending.query.redirect_uri);
      redirect.searchParams.set("error", "access_denied");
      redirect.searchParams.set("state", pending.query.state);
      return { redirect: redirect.href };
    }
    const response = new OAuth2Server.Response();
    try {
      await oauth.authorize(
        new OAuth2Server.Request({
          method: "GET",
          query: pending.query,
          headers: {},
          body: {},
        }),
        response,
        {
          authenticateHandler: {
            handle: async () => ({ agentId: body.agentId, resource }),
          },
        },
      );
    } catch {
      return reply
        .code(400)
        .send({ error: "Authorization could not be completed." });
    }
    return reply
      .header("Cache-Control", "no-store")
      .send({ redirect: response.headers?.location });
  });
  app.post("/oauth/token", async (req, reply) => {
    reply.header("Cache-Control", "no-store").header("Pragma", "no-cache");
    const body = req.body as any;
    const isCode = body?.grant_type === "authorization_code",
      isRefresh = body?.grant_type === "refresh_token";
    if (
      (!isCode && !isRefresh) ||
      (isCode &&
        (typeof body.code !== "string" ||
          typeof body.code_verifier !== "string" ||
          !/^[A-Za-z0-9._~-]{43,128}$/.test(body.code_verifier))) ||
      (isRefresh &&
        (typeof body.refresh_token !== "string" ||
          body.refresh_token.length < 20 ||
          body.refresh_token.length > 512)) ||
      (body.resource !== undefined && body.resource !== resource)
    )
      return reply.code(400).send({ error: "invalid_request" });
    let id = body.client_id,
      provided = body.client_secret;
    const auth = req.headers.authorization;
    if (auth) {
      if (!auth.startsWith("Basic "))
        return reply.code(401).send({ error: "invalid_client" });
      const decoded = Buffer.from(auth.slice(6), "base64").toString();
      const index = decoded.indexOf(":");
      if (index < 0) return reply.code(401).send({ error: "invalid_client" });
      try {
        id = decodeURIComponent(decoded.slice(0, index));
        provided = decodeURIComponent(decoded.slice(index + 1));
      } catch {
        return reply.code(401).send({ error: "invalid_client" });
      }
    }
    const client =
      typeof id === "string" ? await deps.state.get("client", id) : null;
    if (
      !client ||
      (client.secretHash &&
        (typeof provided !== "string" ||
          !timingSafeEqual(
            Buffer.from(digest(provided)),
            Buffer.from(client.secretHash),
          ))) ||
      (client.token_endpoint_auth_method === "client_secret_basic" && !auth) ||
      (client.token_endpoint_auth_method === "client_secret_post" && auth)
    )
      return reply.code(401).send({ error: "invalid_client" });
    const grant = await deps.state.get(
      isCode ? "code" : "refresh",
      digest(isCode ? body.code : body.refresh_token),
    );
    if (
      !grant ||
      grant.user?.resource !== resource ||
      grant.client?.id !== id
    ) {
      if (isRefresh) {
        const used = await deps.state.get(
          "refresh-used",
          digest(body.refresh_token),
        );
        if (used?.clientId === id) await deps.revoke(used.credentialId);
      }
      return reply.code(400).send({ error: "invalid_grant" });
    }
    const response = new OAuth2Server.Response();
    const headers = Object.fromEntries(
      Object.entries(req.headers).filter(
        (entry): entry is [string, string] => typeof entry[1] === "string",
      ),
    );
    try {
      await oauth.token(
        new OAuth2Server.Request({ method: "POST", query: {}, headers, body }),
        response,
      );
    } catch (error) {
      const e = error as any;
      return reply.code(e.code === 401 ? 401 : 400).send({
        error: [
          "invalid_client",
          "invalid_grant",
          "invalid_request",
          "invalid_scope",
          "unsupported_grant_type",
        ].includes(e.name)
          ? e.name
          : "invalid_grant",
      });
    }
    return reply.code(response.status || 200).send(response.body);
  });
}
