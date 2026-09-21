import test from "node:test";
import assert from "node:assert/strict";
import Fastify from "fastify";
import multipart from "@fastify/multipart";
import { createHash, createHmac, randomUUID } from "node:crypto";
import {
  registerAgentHttp,
  participantCommand,
  type AgentTransportDependencies,
} from "../apps/server/src/transport/agent-http.js";
import { registerMcp } from "../apps/server/src/transport/mcp.js";
import {
  registerOAuth,
  type OAuthState,
} from "../apps/server/src/transport/oauth.js";
import { AppError } from "../apps/server/src/domain/store.js";
import {
  Client,
  StreamableHTTPClientTransport,
} from "@modelcontextprotocol/client";
import { MCP_INPUTS } from "../apps/server/src/transport/mcp-inputs.js";
import { verifyWakeup } from "../examples/participant-client.mjs";

const token = "fixture_agent_bearer_123456789";
const actor = {
  id: randomUUID(),
  participant: true,
  credentialId: randomUUID(),
};
function setup() {
  const calls: any[] = [];
  let revoked = false;
  const deps: AgentTransportDependencies = {
    authenticate: async (value) => {
      if (value !== token || revoked)
        throw new AppError(401, "Invalid agent credential");
      return actor;
    },
    execute: async (a, command) => {
      calls.push({ actor: a, command });
      return { value: { agentId: a.id, operation: command.type } };
    },
    storage: {
      put: async (bytes) => ({
        key: randomUUID(),
        size: bytes.length,
        checksum: createHash("sha256").update(bytes).digest("hex"),
      }),
      get: async () => Buffer.from("file proof"),
      reconcile: async () => ({ missing: [], quarantined: [] }),
    },
    upload: async (a, command, metadata) => {
      calls.push({ actor: a, command, metadata });
      return { value: { uploaded: true } };
    },
    download: async (a, id, revisionId) => {
      calls.push({ actor: a, id, revisionId });
      if (id !== "permitted") throw new AppError(404, "Resource not found");
      return {
        storageKey: "checked-key",
        checksum: createHash("sha256").update("file proof").digest("hex"),
      };
    },
  };
  return {
    deps,
    calls,
    revoke: () => {
      revoked = true;
    },
  };
}
function server() {
  const app = Fastify();
  app.setErrorHandler((e: any, _req, reply) =>
    reply.code(e.status || e.statusCode || 500).send({ error: e.message }),
  );
  return app;
}

test("participant HTTP authenticates each command, binds request identity and rejects authority inputs", async () => {
  const app = server(),
    { deps, calls, revoke } = setup();
  await registerAgentHttp(app, deps);
  try {
    const requestId = randomUUID(),
      payload = {
        id: "same-command",
        type: "request.claim",
        input: {},
        requestId,
      };
    assert.equal(
      (
        await app.inject({
          method: "POST",
          url: "/api/agent/commands",
          payload,
        })
      ).statusCode,
      401,
    );
    const result = await app.inject({
      method: "POST",
      url: "/api/agent/commands",
      headers: { authorization: `Bearer ${token}` },
      payload,
    });
    assert.equal(result.statusCode, 200);
    assert.equal(calls[0].actor.requestId, requestId);
    assert.equal(calls[0].actor.owner, undefined);
    assert.throws(() =>
      participantCommand({ ...payload, type: "scope.approve" }),
    );
    assert.throws(() =>
      participantCommand({ ...payload, input: { storageKey: "stolen" } }),
    );
    assert.throws(() =>
      participantCommand({ ...payload, actor: { owner: true } }),
    );
    revoke();
    assert.equal(
      (
        await app.inject({
          method: "POST",
          url: "/api/agent/commands",
          headers: { authorization: `Bearer ${token}` },
          payload,
        })
      ).statusCode,
      401,
    );
  } finally {
    await app.close();
  }
});

test("MCP input schemas teach and validate claim targets, revisions and pagination", () => {
  assert.equal(
    MCP_INPUTS["request.claim"].safeParse({ id: randomUUID() }).success,
    true,
  );
  assert.equal(
    MCP_INPUTS["request.claim"].safeParse({ taskId: randomUUID() }).success,
    true,
  );
  assert.equal(
    MCP_INPUTS["request.claim"].safeParse({
      id: randomUUID(),
      taskId: randomUUID(),
    }).success,
    false,
  );
  assert.equal(MCP_INPUTS["request.claim"].safeParse({}).success, false);
  assert.equal(
    MCP_INPUTS["resource.revise"].safeParse({ id: randomUUID(), text: "new" })
      .success,
    false,
  );
  assert.equal(
    MCP_INPUTS["resource.list"].safeParse({ limit: 51 }).success,
    false,
  );
  assert.equal(
    MCP_INPUTS["resource.list"].safeParse({ limit: 20, afterId: randomUUID() })
      .success,
    true,
  );
});

test("webhook recipe verifies bytes, timestamp, delivery identity and request or task hints", () => {
  const signingSecret = "fixture-signing-secret",
    timestamp = String(Math.floor(Date.now() / 1000));
  const event = {
    id: randomUUID(),
    event: "inbox.available",
    agentId: randomUUID(),
    requestId: randomUUID(),
    generation: 1,
  };
  const signed = (value: any) => {
    const raw = Buffer.from(JSON.stringify(value));
    return {
      raw,
      headers: {
        "x-workspace-delivery": value.id,
        "x-workspace-timestamp": timestamp,
        "x-workspace-signature": `sha256=${createHmac("sha256", signingSecret)
          .update(timestamp + ".")
          .update(raw)
          .digest("hex")}`,
      },
    };
  };
  const request = signed(event);
  assert.deepEqual(
    verifyWakeup(request.raw, request.headers, signingSecret),
    event,
  );
  assert.equal(
    verifyWakeup(
      Buffer.concat([request.raw, Buffer.from(" ")]),
      request.headers,
      signingSecret,
    ),
    null,
  );
  assert.equal(
    verifyWakeup(
      request.raw,
      request.headers,
      signingSecret,
      Date.now() + 600000,
    ),
    null,
  );
  assert.equal(
    verifyWakeup(
      request.raw,
      { ...request.headers, "x-workspace-delivery": "forged" },
      signingSecret,
    ),
    null,
  );
  const task = { ...event, requestId: undefined, taskId: randomUUID() },
    wake = signed(task);
  assert.equal(
    verifyWakeup(wake.raw, wake.headers, signingSecret)?.taskId,
    task.taskId,
  );
});

test("participant files pass through checked helpers and never accept storage keys", async () => {
  const app = server(),
    { deps, calls } = setup();
  await app.register(multipart);
  await registerAgentHttp(app, deps);
  try {
    const body = [
      "--upload",
      'Content-Disposition: form-data; name="commandId"',
      "",
      "upload-proof",
      "--upload",
      'Content-Disposition: form-data; name="input"',
      "",
      '{"title":"Proof"}',
      "--upload",
      'Content-Disposition: form-data; name="file"; filename="proof.txt"',
      "Content-Type: text/plain",
      "",
      "file proof",
      "--upload--",
      "",
    ].join("\r\n");
    const upload = await app.inject({
      method: "POST",
      url: "/api/agent/uploads",
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "multipart/form-data; boundary=upload",
      },
      payload: body,
    });
    assert.equal(upload.statusCode, 200, upload.body);
    assert.equal(calls[0].metadata.size, 10);
    assert.equal(calls[0].command.input.storageKey, undefined);
    const read = await app.inject({
      url: "/api/agent/resources/permitted/revisions/rev/download",
      headers: { authorization: `Bearer ${token}` },
    });
    assert.equal(read.body, "file proof");
    assert.equal(
      (
        await app.inject({
          url: "/api/agent/resources/private/revisions/rev/download",
          headers: { authorization: `Bearer ${token}` },
        })
      ).statusCode,
      404,
    );
  } finally {
    await app.close();
  }
});

test("official MCP HTTP handler negotiates legacy clients, scopes tools and denies revoked bearers", async () => {
  const app = server(),
    { deps, calls, revoke } = setup();
  await registerMcp(app, deps, "http://localhost");
  const headers = {
    authorization: `Bearer ${token}`,
    "content-type": "application/json",
    accept: "application/json, text/event-stream",
  };
  const invoke = (method: string, params: any = {}) =>
    app.inject({
      method: "POST",
      url: "/mcp",
      headers,
      payload: { jsonrpc: "2.0", id: randomUUID(), method, params },
    });
  try {
    const missing = await app.inject({
      method: "POST",
      url: "/mcp",
      headers: { "content-type": "application/json" },
      payload: {},
    });
    assert.equal(missing.statusCode, 401);
    assert.match(
      String(missing.headers["www-authenticate"]),
      /oauth-protected-resource/,
    );
    const init = await invoke("initialize", {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "fixture", version: "1" },
    });
    assert.equal(init.statusCode, 200, init.body);
    const listed = await invoke("tools/list");
    assert.equal(listed.statusCode, 200, listed.body);
    const result = listed.body.startsWith("event:")
      ? JSON.parse(
          listed.body
            .split("\n")
            .find((line) => line.startsWith("data: "))!
            .slice(6),
        )
      : listed.json();
    const names = result.result.tools.map((tool: any) => tool.name);
    assert.ok(names.includes("resource_create"));
    assert.ok(names.includes("agent_list"));
    assert.ok(!names.includes("scope_approve"));
    const call = await invoke("tools/call", {
      name: "agent_me",
      arguments: { commandId: "mcp-proof", input: {} },
    });
    assert.equal(call.statusCode, 200, call.body);
    assert.equal(calls[0].actor.id, actor.id);
    assert.equal(calls[0].actor.owner, undefined);
    revoke();
    assert.equal((await invoke("tools/list")).statusCode, 401);
  } finally {
    await app.close();
  }
});

test("current official MCP client discovers and calls the HTTP server", async () => {
  const app = server(),
    { deps, calls } = setup();
  await registerMcp(app, deps, "http://localhost");
  const address = await app.listen({ host: "127.0.0.1", port: 0 });
  const client = new Client({
    name: "workspace-transport-qualification",
    version: "1.0.0",
  });
  try {
    await client.connect(
      new StreamableHTTPClientTransport(new URL(`${address}/mcp`), {
        requestInit: { headers: { authorization: `Bearer ${token}` } },
      }),
    );
    assert.ok(
      (await client.listTools()).tools.some(
        (tool) => tool.name === "inbox_list",
      ),
    );
    const result = await client.callTool({
      name: "inbox_list",
      arguments: { commandId: "current-sdk", input: {} },
    });
    assert.equal(result.isError, undefined);
    assert.equal(calls[0].command.type, "inbox.list");
  } finally {
    await client.close();
    await app.close();
  }
});

function memoryState(): OAuthState {
  const rows = new Map<string, { payload: any; expires: Date }>();
  return {
    get: async (kind, id) => {
      const item = rows.get(`${kind}:${id}`);
      return item && item.expires > new Date()
        ? structuredClone(item.payload)
        : null;
    },
    put: async (kind, id, payload, expires) => {
      rows.set(`${kind}:${id}`, { payload: structuredClone(payload), expires });
    },
    consume: async (kind, id) => {
      const item = rows.get(`${kind}:${id}`);
      rows.delete(`${kind}:${id}`);
      return item && item.expires > new Date()
        ? structuredClone(item.payload)
        : null;
    },
    count: async (kind) =>
      [...rows.keys()].filter((key) => key.startsWith(`${kind}:`)).length,
  };
}
async function oauthFixture() {
  const app = server(),
    state = memoryState(),
    issues: any[] = [],
    refreshes: string[] = [];
  const agentId = randomUUID(),
    credentialId = randomUUID();
  let revoked = false;
  app.addContentTypeParser(
    "application/x-www-form-urlencoded",
    { parseAs: "string" },
    (_req, body, done) =>
      done(null, Object.fromEntries(new URLSearchParams(String(body)))),
  );
  await registerOAuth(
    app,
    {
      state,
      ownerSession: async (req) =>
        req.headers.cookie === "owner=yes" ? { csrf: "owner-csrf" } : null,
      agents: async () => [{ id: agentId, name: "Atlas", enabled: true }],
      issue: async (...args) => {
        issues.push(args);
        return {
          id: credentialId,
          token,
          expiresAt: new Date(Date.now() + 86400000).toISOString(),
        };
      },
      refresh: async (id) => {
        if (revoked) throw new Error("Revoked");
        assert.equal(id, credentialId);
        refreshes.push(id);
        return {
          id,
          token: `refreshed_access_token_${refreshes.length}`,
          expiresAt: new Date(Date.now() + 86400000).toISOString(),
        };
      },
      revoke: async (id) => {
        assert.equal(id, credentialId);
        revoked = true;
      },
    },
    "http://localhost",
  );
  const registration = await app.inject({
    method: "POST",
    url: "/oauth/register",
    payload: {
      client_name: "Fixture client",
      redirect_uris: ["https://client.example/callback"],
      token_endpoint_auth_method: "none",
    },
  });
  assert.equal(registration.statusCode, 201, registration.body);
  const client = registration.json();
  const verifier = "a".repeat(64),
    challenge = createHash("sha256").update(verifier).digest("base64url");
  const params = new URLSearchParams({
    client_id: client.client_id,
    redirect_uri: client.redirect_uris[0],
    response_type: "code",
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: "state-proof",
    scope: "workspace:participate",
    resource: "http://localhost/mcp",
  });
  const authorize = () =>
    app.inject({
      url: `/oauth/authorize?${params}`,
      headers: { cookie: "owner=yes" },
    });
  const grant = async () => {
    const page = await authorize();
    assert.equal(page.statusCode, 200, page.body);
    const consentId = /data-consent="([^"]+)"/.exec(page.body)![1];
    const result = await app.inject({
      method: "POST",
      url: "/api/oauth/consent",
      headers: { cookie: "owner=yes", "x-csrf-token": "owner-csrf" },
      payload: { consentId, agentId, allow: true },
    });
    assert.equal(result.statusCode, 200, result.body);
    const redirect = new URL(result.json().redirect);
    assert.equal(redirect.searchParams.get("state"), "state-proof");
    return redirect.searchParams.get("code")!;
  };
  const exchange = (code: string, changes: Record<string, string> = {}) =>
    app.inject({
      method: "POST",
      url: "/oauth/token",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      payload: new URLSearchParams({
        grant_type: "authorization_code",
        client_id: client.client_id,
        redirect_uri: client.redirect_uris[0],
        code,
        code_verifier: verifier,
        resource: "http://localhost/mcp",
        ...changes,
      }).toString(),
    });
  const refresh = (
    refreshToken: string,
    changes: Record<string, string> = {},
  ) =>
    app.inject({
      method: "POST",
      url: "/oauth/token",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      payload: new URLSearchParams({
        grant_type: "refresh_token",
        client_id: client.client_id,
        refresh_token: refreshToken,
        resource: "http://localhost/mcp",
        ...changes,
      }).toString(),
    });
  return {
    app,
    issues,
    refreshes,
    params,
    grant,
    exchange,
    authorize,
    refresh,
    state,
    revoke: () => {
      revoked = true;
    },
  };
}

test("OAuth discovery and explicit consent complete PKCE and issue only an agent credential", async () => {
  const f = await oauthFixture();
  try {
    const meta = (
      await f.app.inject({ url: "/.well-known/oauth-protected-resource/mcp" })
    ).json();
    assert.deepEqual(meta.authorization_servers, ["http://localhost"]);
    const page = await f.app.inject({ url: `/oauth/authorize?${f.params}` });
    assert.match(page.body, /Sign in/);
    assert.equal(f.issues.length, 0);
    const code = await f.grant();
    assert.equal(f.issues.length, 0);
    const result = await f.exchange(code);
    assert.equal(result.statusCode, 200, result.body);
    assert.equal(result.json().access_token, token);
    assert.equal(typeof result.json().refresh_token, "string");
    assert.equal(f.issues.length, 1);
    assert.equal((await f.exchange(code)).statusCode, 400);
    assert.equal(f.issues.length, 1);
  } finally {
    await f.app.close();
  }
});

test("OAuth refresh rotates one native credential, binds client and revokes the family on replay", async () => {
  const f = await oauthFixture();
  try {
    const initial = (await f.exchange(await f.grant())).json();
    const stranger = await f.app.inject({
      method: "POST",
      url: "/oauth/register",
      payload: {
        redirect_uris: ["https://other.example/callback"],
        token_endpoint_auth_method: "none",
      },
    });
    assert.equal(
      (
        await f.refresh(initial.refresh_token, {
          client_id: stranger.json().client_id,
        })
      ).statusCode,
      400,
    );
    assert.equal(f.refreshes.length, 0);
    const renewed = await f.refresh(initial.refresh_token);
    assert.equal(renewed.statusCode, 200, renewed.body);
    assert.notEqual(renewed.json().access_token, initial.access_token);
    assert.notEqual(renewed.json().refresh_token, initial.refresh_token);
    assert.equal(f.refreshes.length, 1);
    assert.equal(f.issues.length, 1);
    const stored = await f.state.get(
      "refresh",
      createHash("sha256").update(renewed.json().refresh_token).digest("hex"),
    );
    assert.ok(!JSON.stringify(stored).includes(renewed.json().refresh_token));
    assert.ok(!JSON.stringify(stored).includes(renewed.json().access_token));
    assert.equal((await f.refresh(initial.refresh_token)).statusCode, 400);
    assert.equal(
      (await f.refresh(renewed.json().refresh_token)).statusCode,
      400,
    );
    assert.equal(f.refreshes.length, 1);
  } finally {
    await f.app.close();
  }
});

test("OAuth refresh rejects native revocation and concurrent reuse", async () => {
  const f = await oauthFixture();
  try {
    const initial = (await f.exchange(await f.grant())).json();
    const results = await Promise.all([
      f.refresh(initial.refresh_token),
      f.refresh(initial.refresh_token),
    ]);
    assert.equal(
      results.filter((result) => result.statusCode === 200).length,
      1,
    );
    assert.equal(f.refreshes.length, 1);
    const next = results
      .find((result) => result.statusCode === 200)!
      .json().refresh_token;
    f.revoke();
    assert.equal((await f.refresh(next)).statusCode, 400);
    assert.equal(f.refreshes.length, 1);
  } finally {
    await f.app.close();
  }
});

test("OAuth rejects missing PKCE, bad redirects, CSRF, wrong verifiers and resource substitution", async () => {
  const f = await oauthFixture();
  try {
    const bad = new URLSearchParams(f.params);
    bad.set("redirect_uri", "https://attacker.example/callback");
    assert.equal(
      (
        await f.app.inject({
          url: `/oauth/authorize?${bad}`,
          headers: { cookie: "owner=yes" },
        })
      ).statusCode,
      400,
    );
    bad.set("redirect_uri", f.params.get("redirect_uri")!);
    bad.delete("code_challenge");
    assert.equal(
      (
        await f.app.inject({
          url: `/oauth/authorize?${bad}`,
          headers: { cookie: "owner=yes" },
        })
      ).statusCode,
      400,
    );
    assert.equal(
      (
        await f.app.inject({
          method: "POST",
          url: "/api/oauth/consent",
          headers: { cookie: "owner=yes" },
          payload: { consentId: "forged", allow: true },
        })
      ).statusCode,
      403,
    );
    const code = await f.grant();
    assert.equal(
      (await f.exchange(code, { code_verifier: "b".repeat(64) })).statusCode,
      400,
    );
    assert.equal(f.issues.length, 0);
    assert.equal((await f.exchange(code)).statusCode, 400);
    const other = await f.grant();
    assert.equal(
      (await f.exchange(other, { resource: "https://attacker.example/mcp" }))
        .statusCode,
      400,
    );
    assert.equal(f.issues.length, 0);
  } finally {
    await f.app.close();
  }
});
