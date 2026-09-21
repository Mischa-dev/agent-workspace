import assert from "node:assert/strict";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import {
  Client,
  StreamableHTTPClientTransport,
} from "@modelcontextprotocol/client";

// Explicit deployment check. Creates a labelled verification conversation and
// revokes every temporary agent credential before returning.
const origin = process.env.WORKSPACE_URL;
const tokenFile = process.env.WORKSPACE_OWNER_TOKEN_FILE;
assert(
  origin?.startsWith("https://") && tokenFile,
  "Supply the HTTPS origin and private owner-token file.",
);
const checks: string[] = [];
const request = (route: string, init: RequestInit = {}) =>
  fetch(`${origin}${route}`, { signal: AbortSignal.timeout(20000), ...init });
const login = await request("/api/login", {
  method: "POST",
  headers: { Origin: origin, "Content-Type": "application/json" },
  body: JSON.stringify({ token: readFileSync(tokenFile, "utf8").trim() }),
});
assert.equal(login.status, 200, "Hosted owner login failed.");
assert.match(login.headers.get("set-cookie") || "", /Secure/i);
assert.match(login.headers.get("cache-control") || "", /no-store/);
const cookie = login.headers.get("set-cookie")!.split(";")[0];
const { csrf } = (await login.json()) as any;
const ownerHeaders = {
  Origin: origin,
  "Content-Type": "application/json",
  Cookie: cookie,
  "X-CSRF-Token": csrf,
};
async function owner(type: string, input: any = {}) {
  const r = await request("/api/commands", {
    method: "POST",
    headers: ownerHeaders,
    body: JSON.stringify({ id: randomUUID(), type, input }),
  });
  const body = (await r.json()) as any;
  assert.equal(r.status, 200, `${type}: ${body.error || "request failed"}`);
  return body.value;
}
async function snapshot() {
  const r = await request("/api/snapshot", { headers: { Cookie: cookie } });
  assert.equal(r.status, 200);
  return (await r.json()) as any;
}
async function participant(
  token: string,
  type: string,
  input: any = {},
  requestId?: string,
) {
  const r = await request("/api/agent/commands", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ id: randomUUID(), type, input, requestId }),
  });
  const body = (await r.json()) as any;
  assert.equal(r.status, 200, `${type}: ${body.error || "request failed"}`);
  return body.value;
}
const before = await snapshot();
assert(
  !before.records.some(
    (r: any) =>
      ["request", "run", "workSession"].includes(r.kind) &&
      ["pending", "running", "claimed", "active"].includes(r.data.status),
  ),
  "Do not qualify while real work is active.",
);
const agents = before.records
  .filter((r: any) => r.kind === "agent")
  .slice(0, 2);
assert.equal(agents.length, 2);
const credentialIds = new Set<string>();
let conversation: any;
let client: Client | undefined;
try {
  // This host has no managed runtime. Keep the imported identities usable as
  // editable polling accounts rather than implying a remote model is installed.
  for (const agent of agents)
    await owner("agent.configure", {
      id: agent.id,
      expectedRevision: agent.revision,
      deliveryMode: "poll",
      checkInMinutes: 60,
    });
  checks.push(
    "HTTPS owner sign-in, Secure cookie and private-response cache headers",
  );
  const unauthenticated = await request("/mcp", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: "{}",
  });
  assert.equal(unauthenticated.status, 401);
  assert.match(
    unauthenticated.headers.get("www-authenticate") || "",
    /oauth-protected-resource/,
  );
  assert.equal((await request("/api/snapshot")).status, 401);
  const discovery = await request("/.well-known/oauth-protected-resource/mcp");
  assert.equal(((await discovery.json()) as any).resource, `${origin}/mcp`);
  const authorization = await request(
    "/.well-known/oauth-authorization-server",
  );
  assert.equal(((await authorization.json()) as any).issuer, origin);
  const registration = await request("/oauth/register", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      client_name: "Hosted deployment verification",
      redirect_uris: ["http://127.0.0.1:34987/callback"],
      token_endpoint_auth_method: "none",
      grant_types: ["authorization_code", "refresh_token"],
    }),
  });
  assert.equal(registration.status, 201);
  const registered = (await registration.json()) as any;
  const verifier = randomBytes(32).toString("base64url");
  const query = new URLSearchParams({
    client_id: registered.client_id,
    redirect_uri: registered.redirect_uris[0],
    response_type: "code",
    code_challenge: createHash("sha256").update(verifier).digest("base64url"),
    code_challenge_method: "S256",
    state: randomUUID(),
    resource: `${origin}/mcp`,
    scope: "workspace:participate",
  });
  const page = await request(`/oauth/authorize?${query}`, {
    headers: { Cookie: cookie },
  });
  const consentId = /data-consent="([^"]+)"/.exec(await page.text())?.[1];
  assert(consentId, "OAuth consent page was not available.");
  const consent = await request("/api/oauth/consent", {
    method: "POST",
    headers: ownerHeaders,
    body: JSON.stringify({ consentId, agentId: agents[0].id, allow: true }),
  });
  assert.equal(consent.status, 200);
  const redirect = new URL(((await consent.json()) as any).redirect);
  assert.equal(redirect.searchParams.get("state"), query.get("state"));
  const exchange = await request("/oauth/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      client_id: registered.client_id,
      redirect_uri: registered.redirect_uris[0],
      code: redirect.searchParams.get("code")!,
      code_verifier: verifier,
      resource: `${origin}/mcp`,
    }),
  });
  assert.equal(exchange.status, 200, "Public OAuth exchange failed.");
  const granted = (await exchange.json()) as any;
  for (const c of (await owner("credential.list", { agentId: agents[0].id }))
    .credentials)
    if (c.label.includes("Hosted deployment verification"))
      credentialIds.add(c.id);
  const refreshed = await request("/oauth/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "refresh_token",
      client_id: registered.client_id,
      refresh_token: granted.refresh_token,
      resource: `${origin}/mcp`,
    }),
  });
  assert.equal(refreshed.status, 200);
  const renewed = (await refreshed.json()) as any;
  assert.notEqual(renewed.access_token, granted.access_token);
  const identity = await participant(renewed.access_token, "agent.me");
  assert.equal(identity.agent.id, agents[0].id);
  checks.push(
    "Public OAuth discovery, registration, PKCE consent, exchange and refresh",
  );
  client = new Client({
    name: "hosted-workspace-qualification",
    version: "1.0.0",
  });
  await client.connect(
    new StreamableHTTPClientTransport(new URL(`${origin}/mcp`), {
      requestInit: {
        headers: { authorization: `Bearer ${renewed.access_token}` },
      },
    }),
  );
  const tools = await client.listTools();
  assert(tools.tools.some((t) => t.name === "inbox_list"));
  const result = await client.callTool({
    name: "agent_me",
    arguments: { commandId: randomUUID(), input: {} },
  });
  assert(!result.isError);
  checks.push(
    "Official MCP client discovers tools and calls through Cloudflare Tunnel",
  );
  const second = await owner("credential.issue", {
    agentId: agents[1].id,
    label: "Hosted handoff verification",
    expiresInDays: 1,
  });
  credentialIds.add(second.id);
  conversation = await owner("conversation.create", {
    title: "Deployment verification",
    kind: "channel",
    agentIds: agents.map((a: any) => a.id),
  });
  await owner("settings.dispatch", { enabled: true });
  const sent = await owner("message.send", {
    conversationId: conversation.id,
    body: "Deployment verification: exercise the hosted inbox and file handoff. This is an automated protocol test, not an AI model response.",
    recipientIds: [agents[0].id],
  });
  const task = sent.requests[0];
  assert.equal(
    (await participant(renewed.access_token, "request.claim", { id: task.id }))
      .claimed,
    true,
  );
  const bytes = Buffer.from(
    "Hosted Workspace connection verification. This file passed through two scoped accounts over HTTPS.\n",
  );
  const form = new FormData();
  form.append("commandId", randomUUID());
  form.append("requestId", task.id);
  form.append(
    "input",
    JSON.stringify({ title: "Hosted connection verification", type: "file" }),
  );
  form.append(
    "file",
    new Blob([bytes], { type: "text/plain" }),
    "connection-verification.txt",
  );
  const uploaded = await request("/api/agent/uploads", {
    method: "POST",
    headers: { Authorization: `Bearer ${renewed.access_token}` },
    body: form,
  });
  assert.equal(uploaded.status, 200);
  const saved = ((await uploaded.json()) as any).value;
  const pin = { resourceId: saved.resource.id, revisionId: saved.revision.id };
  const download = await request(
    `/api/agent/resources/${pin.resourceId}/revisions/${pin.revisionId}/download?requestId=${task.id}`,
    { headers: { Authorization: `Bearer ${renewed.access_token}` } },
  );
  assert.equal(download.status, 200);
  assert.deepEqual(Buffer.from(await download.arrayBuffer()), bytes);
  const handoff = await participant(
    renewed.access_token,
    "request.handoff",
    {
      agentId: agents[1].id,
      body: "Automated deployment check: verify this file and complete the handoff.",
      attachments: [pin],
    },
    task.id,
  );
  const next = handoff.requests[0];
  assert.equal(
    (await participant(second.token, "request.claim", { id: next.id })).claimed,
    true,
  );
  const read = await participant(
    second.token,
    "resource.read",
    { id: pin.resourceId, revisionId: pin.revisionId },
    next.id,
  );
  assert.equal(read.revision.data.text, bytes.toString());
  await participant(
    second.token,
    "result.submit",
    {
      summary:
        "Hosted file transfer and handoff passed. This was a deterministic deployment test; no model was invoked.",
      attachments: [pin],
    },
    next.id,
  );
  checks.push(
    "Real hosted inbox claim, file upload/download checksum, cross-account handoff and result",
  );
  for (const id of credentialIds) await owner("credential.revoke", { id });
  assert.equal(
    (
      await request("/api/agent/commands", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${renewed.access_token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ id: randomUUID(), type: "agent.me", input: {} }),
      })
    ).status,
    401,
  );
  checks.push("Temporary credentials revoked and subsequent access denied");
  const report = {
    passed: true,
    verifiedAt: new Date().toISOString(),
    origin,
    checks,
    externalModelCalls: 0,
    fileChecksum: createHash("sha256").update(bytes).digest("hex"),
    recordCount: (await snapshot()).records.length,
  };
  writeFileSync(
    "evidence/hosted-protocol.json",
    JSON.stringify(report, null, 2) + "\n",
  );
  console.log(JSON.stringify(report, null, 2));
} finally {
  await client?.close();
  for (const id of credentialIds) await owner("credential.revoke", { id });
  if (conversation) await owner("conversation.stop", { id: conversation.id });
  await request("/api/logout", {
    method: "POST",
    headers: ownerHeaders,
    body: "{}",
  });
}
