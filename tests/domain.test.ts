import { test, before, after, beforeEach } from "node:test";
import assert from "node:assert/strict";
import pg from "pg";
import { readFileSync, mkdirSync, writeFileSync, symlinkSync } from "node:fs";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
const cfg = JSON.parse(readFileSync(".local/config.json", "utf8"));
const database = `workspace_test_${Date.now()}`,
  u = new URL(cfg.databaseUrl);
u.pathname = "/postgres";
const admin = new pg.Pool({ connectionString: u.toString() });
await admin.query(`CREATE DATABASE ${database}`);
u.pathname = `/${database}`;
process.env.DATABASE_URL = u.toString();
process.env.NODE_ENV = "test";
process.env.OWNER_TOKEN_HASH = createHash("sha256")
  .update("test-owner-only")
  .digest("hex");
process.env.DATA_DIR = path.resolve(`.local/tests/${database}`);
const { pool, db } = await import("../apps/server/src/db.js");
const { migrate } = await import("drizzle-orm/node-postgres/migrator");
await migrate(db, { migrationsFolder: "apps/server/drizzle" });
const { execute, transact, seed } =
  await import("../apps/server/src/domain/commands.js");
const { claimResponse, runPrepared, setRuntimeForTesting, claimTask } =
  await import("../apps/server/src/worker.js");
const { buildServer } = await import("../apps/server/src/server.js");
const { safePath, repoTool } =
  await import("../apps/server/src/domain/repository.js");
const owner = { id: "owner", owner: true };
const cmd = async (type: string, input: any, id = randomUUID()) => {
  if (type === "scope.approve" && !input.expectedRevisions)
    input = {
      ...input,
      expectedProjectRevisions: (
        await transact((s) =>
          Object.fromEntries(
            input.taskIds
              .map((id: string) => s.get(id).data.projectId)
              .filter(Boolean)
              .map((id: string) => [id, s.get(id).revision]),
          ),
        )
      ).value,
      expectedRevisions: (
        await transact((s) =>
          Object.fromEntries(
            input.taskIds.map((id: string) => [id, s.get(id).revision]),
          ),
        )
      ).value,
    };
  return execute(owner, { id, type, input });
};
let agents: any[], conv: any;
beforeEach(async () => {
  await pool.query(
    "TRUNCATE records,events,commands,connector_credentials,owner_sessions,agent_credentials,agent_connections,webhook_deliveries,oauth_state RESTART IDENTITY",
  );
  await seed();
  const result = await transact((s) => ({
    agents: s.all("agent"),
    conv: s.all("conversation").find((c) => c.data.kind === "group"),
  }));
  agents = result.value.agents;
  conv = result.value.conv;
});
after(async () => {
  await pool.end();
  await admin.query(`DROP DATABASE ${database} WITH (FORCE)`);
  await admin.end();
});
test("command retries return original message and conflicting reuse rejects", async () => {
  const id = randomUUID(),
    input = {
      conversationId: conv.id,
      body: "Please plan a useful feature",
      recipientIds: [agents[0].id],
    };
  const a = await cmd("message.send", input, id),
    b = await cmd("message.send", input, id);
  assert.equal(a.value.message.id, b.value.message.id);
  await assert.rejects(
    cmd("message.send", { ...input, body: "Different" }, id),
    /different input/,
  );
  assert.equal((await transact((s) => s.all("request").length)).value, 1);
});
test("routing honors explicit mentions, replies, focus and inert quote mentions", async () => {
  const a = await cmd("message.send", {
    conversationId: conv.id,
    body: `@${agents[1].data.name} please review this code`,
  });
  assert.equal(a.value.requests[0].data.agentId, agents[1].id);
  const b = await cmd("message.send", {
    conversationId: conv.id,
    body: `> @${agents[0].data.name} plan this\nYes, use the second option`,
  });
  assert.equal(b.value.requests[0].data.agentId, agents[1].id);
});
test("acknowledgements cannot renew a burst; substantive owner input can", async () => {
  const a = await cmd("message.send", {
    conversationId: conv.id,
    body: "Plan a feature",
    recipientIds: [agents[0].id],
  });
  await transact((s) =>
    s.patch(a.value.requests[0].data.burstId, { remaining: 0 }),
  );
  const thanks = await cmd("message.send", {
    conversationId: conv.id,
    body: "Thanks",
  });
  assert.equal(thanks.value.requests.length, 0);
  assert.equal(thanks.value.message.data.burstId, a.value.message.data.burstId);
  const b = await cmd("message.send", {
    conversationId: conv.id,
    body: "Now consider a different feature",
    recipientIds: [agents[0].id],
  });
  assert.notEqual(a.value.message.data.burstId, b.value.message.data.burstId);
});
test("atomic claims and Stop prevent duplicate dispatch; stop persists across new messages", async () => {
  const a = await cmd("message.send", {
    conversationId: conv.id,
    body: "Plan a feature",
    recipientIds: [agents[0].id],
  });
  const claims = await Promise.all([
    claimResponse(a.value.requests[0].id),
    claimResponse(a.value.requests[0].id),
  ]);
  assert.equal(claims.filter((c) => c.value).length, 1);
  await cmd("conversation.stop", { id: conv.id });
  await assert.rejects(
    cmd("message.send", { conversationId: conv.id, body: "Do more" }),
    /stopped/,
  );
  const run = (await transact((s) => s.all("run")[0])).value;
  assert.equal(run.data.stopRequested, true);
});
test("chat tools create persistent Resources but cannot access another conversation", async () => {
  const dm = (
    await transact((s) =>
      s.all("conversation").find((c) => c.data.kind === "dm"),
    )
  ).value!;
  const privateResource = (
    await cmd("resource.create", {
      title: "Private secret",
      text: "sensitive-marker",
      conversationIds: [dm.id],
    })
  ).value.resource;
  const message = await cmd("message.send", {
    conversationId: conv.id,
    body: "Save a note",
    recipientIds: [agents[0].id],
  });
  const run = (await claimResponse(message.value.requests[0].id)).value!;
  const actor = {
    id: agents[0].id,
    requestId: message.value.requests[0].id,
    runId: run.id,
  };
  await assert.rejects(
    execute(actor, {
      id: randomUUID(),
      type: "resource.read",
      input: { id: privateResource.id },
    }),
    /not found/,
  );
  const made = await execute(actor, {
    id: "tool-1",
    type: "resource.create",
    input: { title: "Actual note", text: "Persistent content" },
  });
  assert.equal(made.value.revision.data.text, "Persistent content");
  const list = await execute(actor, {
    id: randomUUID(),
    type: "resource.list",
    input: {},
  });
  assert.equal(list.value.length, 1);
  await assert.rejects(
    execute(actor, {
      id: randomUUID(),
      type: "scope.approve",
      input: { taskIds: [] },
    }),
    /owner/,
  );
});
test("Resource revisions conflict and old pinned inputs stay unchanged", async () => {
  const a = (
    await cmd("resource.create", {
      title: "Requirements",
      text: "version one",
      conversationIds: [conv.id],
    })
  ).value;
  const b = await cmd("resource.revise", {
    id: a.resource.id,
    expectedRevision: a.resource.revision,
    text: "version two",
  });
  await assert.rejects(
    cmd("resource.revise", {
      id: a.resource.id,
      expectedRevision: a.resource.revision,
      text: "lost edit",
    }),
    /preserved/,
  );
  const old = (
    await cmd("resource.read", { id: a.resource.id, revisionId: a.revision.id })
  ).value;
  assert.equal(old.revision.data.text, "version one");
  assert.notEqual(b.value.revision.id, a.revision.id);
});
test("independent opinions freeze shared revisions and hide peers through retrieval", async () => {
  const r = (
    await cmd("resource.create", {
      title: "Input",
      text: "frozen",
      conversationIds: [conv.id],
    })
  ).value;
  const m = await cmd("message.send", {
    conversationId: conv.id,
    body: "Give independent opinions",
    mode: "independent",
  });
  await cmd("resource.revise", {
    id: r.resource.id,
    expectedRevision: r.resource.revision,
    text: "later edit",
  });
  const claim = await claimResponse(m.value.requests[0].id);
  const actor = {
    id: m.value.requests[0].data.agentId,
    requestId: m.value.requests[0].id,
  };
  const read = await execute(actor, {
    id: randomUUID(),
    type: "resource.read",
    input: { id: r.resource.id },
  });
  assert.equal(read.value.revision.data.text, "frozen");
  await assert.rejects(
    execute(actor, {
      id: randomUUID(),
      type: "resource.revise",
      input: {
        id: r.resource.id,
        expectedRevision: r.resource.revision + 1,
        text: "side channel",
      },
    }),
    /read-only/,
  );
});
test("dependency cycles reject, cancelled prerequisites stay blocked, changed approval rejects", async () => {
  const p = (await cmd("project.create", { name: "Test" })).value;
  const a = (
    await cmd("task.create", {
      title: "A",
      description: "Deliver A",
      criteria: ["A works"],
      projectId: p.id,
    })
  ).value;
  const b = (
    await cmd("task.create", {
      title: "B",
      description: "Deliver B",
      criteria: ["B works"],
      dependencies: [a.id],
      projectId: p.id,
    })
  ).value;
  await assert.rejects(
    cmd("task.update", {
      id: a.id,
      expectedRevision: a.revision,
      dependencies: [b.id],
    }),
    /cycle/,
  );
  const scope = (
    await cmd("scope.approve", { title: "Scope", taskIds: [a.id, b.id] })
  ).value;
  await cmd("task.update", {
    id: a.id,
    expectedRevision: a.revision,
    title: "A revised",
  });
  await assert.rejects(cmd("session.start", { scopeId: scope.id }), /changed/);
});
test("HTTP authentication, origin and CSRF enforced; events survive new server", async () => {
  const app = await buildServer();
  const headers = { host: "127.0.0.1:4310" };
  assert.equal(
    (await app.inject({ url: "/api/snapshot", headers })).statusCode,
    401,
  );
  assert.equal(
    (
      await app.inject({
        url: "/health",
        headers: { host: "attacker.example" },
      })
    ).statusCode,
    403,
  );
  const login = await app.inject({
    method: "POST",
    url: "/api/login",
    headers: { ...headers, origin: "http://127.0.0.1:4310" },
    payload: { token: "test-owner-only" },
  });
  assert.equal(login.statusCode, 200);
  const cookie = login.cookies[0].name + "=" + login.cookies[0].value;
  assert.equal(
    (
      await app.inject({
        method: "POST",
        url: "/api/commands",
        headers: { ...headers, cookie, origin: "http://attacker.example" },
        payload: {},
      })
    ).statusCode,
    403,
  );
  assert.equal(
    (
      await app.inject({
        method: "POST",
        url: "/api/commands",
        headers: { ...headers, cookie, origin: "http://127.0.0.1:4310" },
        payload: {},
      })
    ).statusCode,
    403,
  );
  await cmd("space.create", { name: "Persistent" });
  const snapshot = await app.inject({
    url: "/api/snapshot",
    headers: { ...headers, cookie },
  });
  assert.ok(snapshot.json().cursor > 0);
  await app.close();
  const second = await buildServer();
  const again = await second.inject({
    url: "/api/snapshot",
    headers: { ...headers, cookie },
  });
  assert.equal(again.json().cursor, snapshot.json().cursor);
  assert.ok(
    again.json().records.some((r: any) => r.data.name === "Persistent"),
  );
  await second.close();
});
test("execution path blocks traversal, credentials and symlink reads", async () => {
  const root = path.resolve(process.env.DATA_DIR!, "safe");
  mkdirSync(root, { recursive: true });
  writeFileSync(path.join(root, "hello.txt"), "hello");
  symlinkSync("/etc/passwd", path.join(root, "link"));
  assert.equal(
    (await repoTool(root, "repo.read", { path: "hello.txt" })).text,
    "hello",
  );
  for (const unsafe of ["../config.json", ".git/config", ".env", "link"])
    await assert.rejects(safePath(root, unsafe));
  await repoTool(root, "repo.write", { path: "nested/file.txt", text: "made" });
  assert.equal(
    (await repoTool(root, "repo.read", { path: "nested/file.txt" })).text,
    "made",
  );
});
test("fixture connector uses checked DB commands and preserves terminal outcome", async () => {
  setRuntimeForTesting({
    async capabilities() {
      return { fixture: true };
    },
    async run(input) {
      await input.tool(
        "workspace",
        {
          operation: "resource.create",
          input: {
            title: "Fixture output",
            text: "stored by real domain command",
          },
        },
        {
          callId: "call-1",
          threadId: "fixture-thread",
          turnId: "fixture-turn",
        },
      );
      return {
        status: "completed",
        text: "Saved the note.",
        threadId: "fixture-thread",
        turnId: "fixture-turn",
      };
    },
  });
  const m = await cmd("message.send", {
    conversationId: conv.id,
    body: "Save this",
    recipientIds: [agents[0].id],
  });
  const run = (await claimResponse(m.value.requests[0].id)).value!;
  await runPrepared(run);
  const result = (
    await transact((s) => ({
      request: s.get(m.value.requests[0].id),
      resources: s.all("resource"),
      run: s.get(run.id),
    }))
  ).value;
  assert.equal(result.request.data.status, "answered");
  assert.equal(result.resources.length, 1);
  assert.equal(result.run.data.status, "completed");
});
test("proposal application rejects mismatched Resource revision ownership", async () => {
  const first = (
    await cmd("resource.create", { title: "Public", text: "public" })
  ).value;
  const second = (
    await cmd("resource.create", { title: "Private", text: "private" })
  ).value;
  const proposal = (
    await cmd("proposal.create", {
      title: "Forged pin",
      tasks: [
        {
          title: "Read it",
          resources: [
            { resourceId: first.resource.id, revisionId: second.revision.id },
          ],
        },
      ],
    })
  ).value;
  await assert.rejects(
    cmd("proposal.apply", { id: proposal.id }),
    /different Resource/,
  );
});
test("Stop after claim prevents the runtime invocation", async () => {
  let called = false;
  setRuntimeForTesting({
    async capabilities() {
      return {};
    },
    async run() {
      called = true;
      return { status: "completed", text: "unexpected" };
    },
  });
  const m = await cmd("message.send", {
    conversationId: conv.id,
    body: "Plan work",
    recipientIds: [agents[0].id],
  });
  const run = (await claimResponse(m.value.requests[0].id)).value!;
  await cmd("conversation.stop", { id: conv.id });
  await runPrepared(run);
  assert.equal(called, false);
});
test("Resource revocation after claim prevents dispatch of frozen content", async () => {
  let called = false;
  setRuntimeForTesting({
    async capabilities() {
      return {};
    },
    async run() {
      called = true;
      return { status: "completed", text: "unexpected" };
    },
  });
  const r = (
    await cmd("resource.create", {
      title: "Revocable",
      text: "do not dispatch after revoke",
      conversationIds: [conv.id],
    })
  ).value;
  const m = await cmd("message.send", {
    conversationId: conv.id,
    body: "Read attachment",
    recipientIds: [agents[0].id],
    attachments: [{ resourceId: r.resource.id, revisionId: r.revision.id }],
  });
  const run = (await claimResponse(m.value.requests[0].id)).value!;
  await cmd("resource.grant", {
    id: r.resource.id,
    expectedRevision: r.resource.revision,
    conversationIds: [],
    agentIds: [],
    projectIds: [],
    spaceIds: [],
  });
  await runPrepared(run);
  assert.equal(called, false);
});
test("research tasks on repository-linked projects never receive execution mode or checkout", async () => {
  let observed: any;
  setRuntimeForTesting({
    async capabilities() {
      return {};
    },
    async run(input) {
      observed = input;
      return { status: "completed", text: "Research completed" };
    },
  });
  const p = (
    await cmd("project.create", {
      name: "Research",
      repository: "/definitely/not/an/authorized/test/repo",
    })
  ).value;
  const t = (
    await cmd("task.create", {
      title: "Research only",
      description: "Summarize options",
      criteria: ["Useful summary"],
      execution: "chat",
      projectId: p.id,
    })
  ).value;
  const scope = (await cmd("scope.approve", { taskIds: [t.id], minutes: 1 }))
    .value;
  const session = (await cmd("session.start", { scopeId: scope.id })).value;
  const run = (await claimTask(session.id)).value!;
  await runPrepared(run);
  assert.equal(observed.mode, "chat");
  assert.ok(!observed.cwd.includes("/definitely"));
  assert.equal(
    (await transact((s) => s.get(run.id))).value.data.status,
    "completed",
  );
});

test("scope approval requires the displayed task and project revisions", async () => {
  const p = (await cmd("project.create", { name: "Race" })).value;
  const t = (
    await cmd("task.create", {
      title: "Task",
      description: "Useful outcome",
      criteria: ["Done"],
      projectId: p.id,
    })
  ).value;
  await assert.rejects(
    execute(owner, {
      id: randomUUID(),
      type: "scope.approve",
      input: { taskIds: [t.id] },
    }),
    /revision|preview/i,
  );
  await cmd("project.update", {
    id: p.id,
    expectedRevision: p.revision,
    name: "Changed project",
  });
  await assert.rejects(
    execute(owner, {
      id: randomUUID(),
      type: "scope.approve",
      input: {
        taskIds: [t.id],
        expectedRevisions: { [t.id]: t.revision },
        expectedProjectRevisions: { [p.id]: p.revision },
      },
    }),
    /changed/i,
  );
});
test("parallel tasks respect shared locks and release admission after review", async () => {
  const p = (await cmd("project.create", { name: "Locks" })).value;
  const ids = [];
  for (const [title, locks] of [
    ["First", ["schema"]],
    ["Second", ["schema"]],
    ["Third", ["ui"]],
  ] as const)
    ids.push(
      (
        await cmd("task.create", {
          title,
          description: "Bounded output",
          criteria: ["Done"],
          locks,
          projectId: p.id,
        })
      ).value.id,
    );
  const scope = (await cmd("scope.approve", { taskIds: ids, maxWorkers: 2 }))
    .value;
  const session = (await cmd("session.start", { scopeId: scope.id })).value;
  const first = (await claimTask(session.id)).value!;
  const third = (await claimTask(session.id)).value!;
  assert.equal(first.data.taskId, ids[0]);
  assert.equal(third.data.taskId, ids[2]);
  assert.equal((await claimTask(session.id)).value, null);
  await transact((s) => {
    s.patch(first.id, { status: "completed" });
    s.patch(ids[0], { status: "review" });
  });
  const second = (await claimTask(session.id)).value!;
  assert.equal(second.data.taskId, ids[1]);
});
test("task outputs deduplicate tools and flow as pinned prerequisite input", async () => {
  let calls = 0;
  setRuntimeForTesting({
    async capabilities() {
      return { fixture: true };
    },
    async run(input) {
      calls++;
      if (calls === 1) {
        const args = {
          operation: "resource.create",
          input: { title: "Design decision", text: "Use blue labels." },
        };
        const metadata = {
          callId: "stable-output",
          threadId: "fixture",
          turnId: "fixture",
        };
        const a = (await input.tool("workspace", args, metadata)) as any;
        const b = (await input.tool("workspace", args, metadata)) as any;
        assert.equal(a.resource.id, b.resource.id);
        await assert.rejects(
          input.tool(
            "workspace",
            { ...args, input: { ...args.input, text: "Changed" } },
            metadata,
          ),
          /different input/,
        );
      } else {
        assert.match(input.prompt, /Use blue labels/);
        assert.match(input.prompt, /prerequisiteResults/);
      }
      return { status: "completed", text: "Finished output" };
    },
  });
  const p = (await cmd("project.create", { name: "Outputs" })).value;
  const first = (
    await cmd("task.create", {
      title: "First",
      description: "Make decision",
      criteria: ["Decision saved"],
      projectId: p.id,
    })
  ).value;
  const second = (
    await cmd("task.create", {
      title: "Second",
      description: "Use decision",
      criteria: ["Uses decision"],
      projectId: p.id,
      dependencies: [first.id],
    })
  ).value;
  const scope = (await cmd("scope.approve", { taskIds: [first.id, second.id] }))
    .value;
  const session = (await cmd("session.start", { scopeId: scope.id })).value;
  await runPrepared((await claimTask(session.id)).value!);
  assert.equal((await claimTask(session.id)).value, null);
  const current = (await transact((s) => s.get(first.id))).value;
  await cmd("task.update", {
    id: first.id,
    expectedRevision: current.revision,
    status: "accepted",
  });
  await runPrepared((await claimTask(session.id)).value!);
  assert.equal(calls, 2);
  assert.equal(
    (
      await transact(
        (s) =>
          s.all("resource").filter((r) => r.data.title === "Design decision")
            .length,
      )
    ).value,
    1,
  );
});
test("work webhook is authenticated and duplicate deliveries start only one session", async () => {
  const { config } = await import("../apps/server/src/config.js");
  const previous = config.webhookTokenHash;
  config.webhookTokenHash = createHash("sha256")
    .update("fixture-hook")
    .digest("hex");
  const app = await buildServer();
  try {
    const t = (
      await cmd("task.create", {
        title: "Hook task",
        description: "An outcome",
        criteria: ["Done"],
      })
    ).value;
    const scope = (await cmd("scope.approve", { taskIds: [t.id] })).value;
    const headers = {
      host: "127.0.0.1:4310",
      "idempotency-key": "delivery-one",
      authorization: "Bearer fixture-hook",
    };
    const request = {
      method: "POST" as const,
      url: "/api/hooks/work",
      headers,
      payload: { scopeId: scope.id },
    };
    assert.equal(
      (
        await app.inject({
          ...request,
          headers: { ...headers, authorization: "Bearer wrong" },
        })
      ).statusCode,
      401,
    );
    const a = await app.inject(request),
      b = await app.inject(request);
    assert.equal(a.statusCode, 200);
    assert.equal(b.statusCode, 200);
    assert.equal(a.json().value.id, b.json().value.id);
    assert.equal((await transact((s) => s.all("workSession").length)).value, 1);
    assert.equal(
      (await app.inject({ ...request, payload: { scopeId: randomUUID() } }))
        .statusCode,
      409,
    );
  } finally {
    await app.close();
    config.webhookTokenHash = previous;
  }
});
test("authenticated upload preserves bytes, rejects checksum damage and quarantines orphans", async () => {
  const { storage } = await import("../apps/server/src/storage/local.js");
  const { auditStorage } = await import("../apps/server/src/storage/audit.js");
  const { utimes } = await import("node:fs/promises");
  const app = await buildServer();
  const headers = { host: "127.0.0.1:4310", origin: "http://127.0.0.1:4310" };
  try {
    const login = await app.inject({
      method: "POST",
      url: "/api/login",
      headers,
      payload: { token: "test-owner-only" },
    });
    const cookie = `workspace=${login.cookies[0].value}`;
    const boundary = "workspace-test-boundary";
    const space = (
      await cmd("space.create", {
        name: "File board",
        agentIds: [agents[0].id],
      })
    ).value;
    const body = Buffer.from(
      `--${boundary}\r\nContent-Disposition: form-data; name="title"\r\n\r\nUploaded note\r\n--${boundary}\r\nContent-Disposition: form-data; name="spaceIds"\r\n\r\n${JSON.stringify([space.id])}\r\n--${boundary}\r\nContent-Disposition: form-data; name="agentIds"\r\n\r\n${JSON.stringify([agents[0].id])}\r\n--${boundary}\r\nContent-Disposition: form-data; name="tags"\r\n\r\n["shared-file"]\r\n--${boundary}\r\nContent-Disposition: form-data; name="pinned"\r\n\r\ntrue\r\n--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="note.txt"\r\nContent-Type: text/plain\r\n\r\nimmutable bytes\r\n--${boundary}--\r\n`,
    );
    const upload = await app.inject({
      method: "POST",
      url: "/api/uploads",
      headers: {
        ...headers,
        cookie,
        "x-csrf-token": login.json().csrf,
        "content-type": `multipart/form-data; boundary=${boundary}`,
      },
      payload: body,
    });
    assert.equal(upload.statusCode, 200, upload.body);
    const { resource, revision } = upload.json().value;
    assert.deepEqual(resource.data.agentIds, [agents[0].id]);
    assert.deepEqual(resource.data.spaceIds, [space.id]);
    assert.deepEqual(resource.data.tags, ["shared-file"]);
    assert.equal(resource.data.pinned, true);
    const url = `/api/resources/${resource.id}/revisions/${revision.id}/download`;
    assert.equal((await app.inject({ url, headers })).statusCode, 401);
    assert.equal(
      (await app.inject({ url, headers: { ...headers, cookie } })).body,
      "immutable bytes",
    );
    const orphan = await storage.put(Buffer.from("orphan preserved"));
    const location = path.join(process.env.DATA_DIR!, "files", orphan.key);
    await utimes(location, new Date(0), new Date(0));
    const audit = await auditStorage();
    assert.ok(audit.quarantined.includes(orphan.key));
    assert.equal(audit.ok, true);
    writeFileSync(
      path.join(process.env.DATA_DIR!, "files", revision.data.storageKey),
      "corruption",
    );
    assert.equal(
      (await app.inject({ url, headers: { ...headers, cookie } })).statusCode,
      409,
    );
    assert.ok((await auditStorage()).mismatched.includes(revision.id));
    await assert.rejects(storage.get("../outside"), /Invalid storage/);
  } finally {
    await app.close();
  }
});
test("SSE replays committed changes and reconciles cursors beyond restored history", async () => {
  const app = await buildServer();
  await app.listen({ host: "127.0.0.1", port: 0 });
  const address = app.server.address() as any;
  const login = await app.inject({
    method: "POST",
    url: "/api/login",
    headers: { host: "127.0.0.1:4310", origin: "http://127.0.0.1:4310" },
    payload: { token: "test-owner-only" },
  });
  const cookie = `workspace=${login.cookies[0].value}`;
  const { request } = await import("node:http");
  const read = (after: number) =>
    new Promise<string>((resolve, reject) => {
      const req = request(
        {
          host: "127.0.0.1",
          port: address.port,
          path: `/api/events?after=${after}`,
          headers: { host: "127.0.0.1:4310", cookie },
        },
        (res) => {
          let buffer = "";
          res.on("data", (chunk) => {
            buffer += chunk;
            if (buffer.includes("event:")) {
              resolve(buffer);
              req.destroy();
            }
          });
        },
      );
      req.on("error", reject);
      req.setTimeout(4000, () => {
        req.destroy();
        reject(new Error("SSE timeout"));
      });
      req.end();
    });
  try {
    const before = (await transact(() => null)).cursor;
    await cmd("space.create", { name: "Replay marker" });
    assert.match(await read(before), /event: change/);
    assert.match(await read(999999), /event: reconcile/);
    await pool.query(
      "INSERT INTO events(type,ids) SELECT 'fixture','[]'::jsonb FROM generate_series(1,102)",
    );
    assert.match(await read(0), /event: reconcile/);
  } finally {
    app.server.closeAllConnections();
    await app.close();
  }
});

test("real worktree checkpoint preserves the target and rejects host checkout filters", async () => {
  const {
    git,
    createWorktree,
    checkpointWorktree,
    capturePatch,
    verifyIntegration,
  } = await import("../apps/server/src/domain/repository.js");
  const repo = path.join(process.env.DATA_DIR!, "checkpoint-repo");
  mkdirSync(repo, { recursive: true });
  await git(repo, ["init", "-b", "main"]);
  writeFileSync(path.join(repo, "base.txt"), "original\n");
  await git(repo, ["add", "base.txt"]);
  await git(repo, [
    "-c",
    "user.name=Test",
    "-c",
    "user.email=test@workspace.invalid",
    "commit",
    "-m",
    "fixture",
  ]);
  const base = await git(repo, ["rev-parse", "main"]);
  const checkout = await createWorktree(repo, randomUUID(), "main");
  await repoTool(checkout.dir, "repo.write", {
    path: "new.json",
    text: '{"qualified":true}\n',
  });
  const patch = await capturePatch(checkout.dir);
  assert.equal(patch.newFiles.length, 1);
  const checkpoint = await checkpointWorktree(checkout.dir, checkout.branch);
  assert.equal(checkpoint.changed, true);
  assert.equal(await git(repo, ["rev-parse", "main"]), base);
  assert.match(checkpoint.diff, /qualified/);
  await assert.rejects(
    verifyIntegration(repo, "main", checkpoint.commit),
    /not included/,
  );
  // Only a dedicated non-default fixture branch is advanced to exercise ancestry verification.
  await git(repo, ["branch", "integration-fixture", checkpoint.commit]);
  assert.equal(
    (await verifyIntegration(repo, "integration-fixture", checkpoint.commit))
      .commit,
    checkpoint.commit,
  );
  await repoTool(checkout.dir, "repo.write", {
    path: "base.txt",
    text: "bad whitespace  \n",
  });
  assert.equal((await repoTool(checkout.dir, "repo.check", {})).passed, false);
  assert.match((await capturePatch(checkout.dir)).diff, /bad whitespace/);
  await git(repo, [
    "config",
    "filter.fixture.smudge",
    "touch SHOULD_NOT_EXIST",
  ]);
  await assert.rejects(
    createWorktree(repo, randomUUID(), "main"),
    /checkout filters/,
  );
});

test("context retrieval is bounded, private, and ordinary pending requests catch up at claim", async () => {
  const message = (
    await cmd("message.send", {
      conversationId: conv.id,
      body: "Prepare a careful plan",
      recipientIds: [agents[0].id],
    })
  ).value;
  await transact((s) => {
    for (let n = 0; n < 70; n++)
      s.add("message", {
        conversationId: conv.id,
        authorId: "system",
        body: `Background observation ${n}`,
        attachments: [],
      });
  });
  const run = (await claimResponse(message.requests[0].id)).value!;
  assert.match(run.data.packet, /Background observation/);
  const actor = { id: agents[0].id, requestId: message.requests[0].id };
  const read = async (input: any) =>
    (
      await execute(actor, {
        id: randomUUID(),
        type: "conversation.read",
        input,
      })
    ).value;
  const page = await read({ limit: 12 });
  assert.equal(page.items.length, 12);
  assert.ok(page.nextBeforeId);
  const older = await read({ limit: 12, beforeId: page.nextBeforeId });
  assert.equal(older.items.length, 12);
  assert.ok(
    !older.items.some((r: any) => page.items.some((p: any) => p.id === r.id)),
  );
  await assert.rejects(
    read({ beforeId: randomUUID() }),
    /outside this conversation/,
  );
});
test("correction after completion labels the old response and preserves its initial manifest", async () => {
  setRuntimeForTesting({
    async capabilities() {
      return { fixture: true };
    },
    async run() {
      return { status: "completed", text: "Initial answer" };
    },
  });
  const m = (
    await cmd("message.send", {
      conversationId: conv.id,
      body: "Plan option one",
      recipientIds: [agents[0].id],
    })
  ).value;
  const run = (await claimResponse(m.requests[0].id)).value!;
  await runPrepared(run);
  await cmd("message.send", {
    conversationId: conv.id,
    body: "Correction: plan option two",
    corrects: m.message.id,
    recipientIds: [agents[0].id],
  });
  const result = (
    await transact((s) => ({
      run: s.get(run.id),
      reply: s
        .all("message")
        .find((r) => r.data.requestId === m.requests[0].id),
    }))
  ).value;
  assert.equal(result.reply!.data.predatesCorrection, true);
  assert.equal(result.run.data.packet, run.data.packet);
});

test("project change or disabling a claimed task agent prevents runtime dispatch", async () => {
  let invoked = 0;
  setRuntimeForTesting({
    async capabilities() {
      return {};
    },
    async run() {
      invoked++;
      return { status: "completed", text: "Unexpected" };
    },
  });
  for (const change of ["project", "agent"]) {
    const p = (await cmd("project.create", { name: change })).value;
    const t = (
      await cmd("task.create", {
        title: "Scoped task",
        description: "Work",
        criteria: ["Done"],
        projectId: p.id,
      })
    ).value;
    const scope = (await cmd("scope.approve", { taskIds: [t.id] })).value;
    const session = (await cmd("session.start", { scopeId: scope.id })).value;
    const run = (await claimTask(session.id)).value!;
    if (change === "project")
      await cmd("project.update", {
        id: p.id,
        expectedRevision: p.revision,
        name: "Reconfigured",
      });
    else {
      const a = (await transact((s) => s.get(run.data.agentId))).value;
      await cmd("agent.update", {
        id: a.id,
        expectedRevision: a.revision,
        enabled: false,
      });
    }
    await runPrepared(run);
    await transact((s) => s.patch(session.id, { status: "complete" }));
  }
  assert.equal(invoked, 0);
});
test("known exhausted capacity holds dispatch without a model call", async () => {
  const { setSetting } = await import("../apps/server/src/db.js");
  const { normalizeCapacity } =
    await import("../apps/server/src/domain/capacity.js");
  const exhausted = normalizeCapacity({
    rateLimits: {
      primary: { usedPercent: 100 },
      secondary: { usedPercent: 20 },
    },
  });
  await setSetting("capacity", exhausted);
  await setSetting("lastKnownCapacity", exhausted);
  let invoked = false;
  setRuntimeForTesting({
    async capabilities() {
      return {};
    },
    async run() {
      invoked = true;
      return { status: "completed", text: "Unexpected" };
    },
  });
  try {
    const m = (
      await cmd("message.send", {
        conversationId: conv.id,
        body: "A bounded request",
        recipientIds: [agents[0].id],
      })
    ).value;
    const run = (await claimResponse(m.requests[0].id)).value!;
    await runPrepared(run);
    assert.equal(invoked, false);
    assert.match(
      (await transact((s) => s.get(run.id))).value.data.error,
      /allowance is exhausted/,
    );
  } finally {
    await setSetting("capacity", null);
    await setSetting("lastKnownCapacity", null);
  }
});
test("GitHub publishing remains denied and malformed read URLs reject locally", async () => {
  const { github } = await import("../apps/server/src/connectors/github.js");
  assert.equal((await github.capabilities()).publish, false);
  await assert.rejects(github.publishDraft(), /explicit authorization/);
  await assert.rejects(
    github.readPull("https://attacker.example/pull/1"),
    /GitHub pull request URL/,
  );
});

async function connectedAgent(index = 0, mode = "poll") {
  const { authenticateAgent } =
    await import("../apps/server/src/domain/agent-access.js");
  const current = (await transact((s) => s.get(agents[index].id, "agent")))
    .value;
  await cmd("agent.configure", {
    id: current.id,
    expectedRevision: current.revision,
    deliveryMode: mode,
    checkInMinutes: 60,
    ...(mode === "webhook"
      ? {
          webhookUrl: "https://receiver.example/wake",
          webhookSecret: "test-signing-secret-for-wakeup",
        }
      : {}),
  });
  const credential = (
    await cmd("credential.issue", {
      agentId: current.id,
      label: "Test connection",
    })
  ).value;
  return { actor: await authenticateAgent(credential.token), credential };
}
async function agentCommand(
  actor: any,
  type: string,
  input: any = {},
  id = randomUUID(),
) {
  return execute(actor, { id, type, input });
}

test("agent credentials are shown once, expire or revoke, and never enter the command journal", async () => {
  const { authenticateAgent } =
    await import("../apps/server/src/domain/agent-access.js");
  const id = randomUUID(),
    input = {
      agentId: agents[0].id,
      label: "One-time credential",
      expiresInDays: 2,
    };
  const issued = (await cmd("credential.issue", input, id)).value;
  assert.match(issued.token, /^aw_/);
  const replay = (await cmd("credential.issue", input, id)).value;
  assert.equal(replay.token, undefined);
  assert.equal(replay.oneTimeTokenUnavailable, true);
  const saved = (
    await pool.query("SELECT result FROM commands WHERE id=$1", [`owner:${id}`])
  ).rows[0].result;
  assert.equal(JSON.stringify(saved).includes(issued.token), false);
  assert.equal((await authenticateAgent(issued.token)).id, agents[0].id);
  await cmd("credential.revoke", { id: issued.id });
  await assert.rejects(authenticateAgent(issued.token), /expired, revoked/);
});

test("owner can issue an until-revoked agent credential", async () => {
  const { authenticateAgent } =
    await import("../apps/server/src/domain/agent-access.js");
  const issued = (await cmd("credential.issue", {
    agentId: agents[0].id,
    label: "Persistent connection",
    expiresInDays: 0,
  })).value;
  assert.match(issued.expiresAt, /^9999-12-31T/);
  assert.equal((await authenticateAgent(issued.token)).id, agents[0].id);
  await cmd("credential.revoke", { id: issued.id });
  await assert.rejects(authenticateAgent(issued.token), /expired, revoked/);
});

test("polling accounts claim once through the shared admission path and reply as themselves", async () => {
  const { actor } = await connectedAgent();
  const sent = await cmd("message.send", {
    conversationId: conv.id,
    body: "Please research this",
    recipientIds: [actor.id],
  });
  const req = sent.value.requests[0];
  assert.equal((await claimResponse(req.id)).value, null);
  const inbox = (await agentCommand(actor, "inbox.list")).value;
  assert.equal(inbox.requests[0].request.id, req.id);
  const claims = await Promise.allSettled([
    agentCommand(actor, "request.claim", { id: req.id }),
    agentCommand(actor, "request.claim", { id: req.id }),
  ]);
  assert.equal(claims.filter((x) => x.status === "fulfilled").length, 1);
  const success = claims.find(
    (x) => x.status === "fulfilled",
  ) as PromiseFulfilledResult<any>;
  assert.equal(
    success.value.value.context.request.text,
    "Please research this",
  );
  const result = (
    await agentCommand({ ...actor, requestId: req.id }, "message.reply", {
      body: "Here is the research.",
    })
  ).value;
  assert.equal(result.message.data.authorId, actor.id);
  assert.equal(result.request.data.status, "answered");
  assert.equal(
    (await agentCommand(actor, "inbox.list")).value.requests.length,
    0,
  );
});

test("handoffs preserve the owner's allowance and another account picks up the same context", async () => {
  const first = await connectedAgent(0),
    second = await connectedAgent(1);
  const sent = await cmd("message.send", {
    conversationId: conv.id,
    body: "Investigate a design",
    recipientIds: [first.actor.id],
  });
  const req = sent.value.requests[0];
  await agentCommand(first.actor, "request.claim", { id: req.id });
  const handed = (
    await agentCommand(
      { ...first.actor, requestId: req.id },
      "request.handoff",
      { agentId: second.actor.id, body: "Please review the findings." },
    )
  ).value;
  assert.equal(handed.requests.length, 1);
  assert.equal(
    (await transact((s) => s.get(req.data.burstId, "burst").data.remaining))
      .value,
    5,
  );
  const claimed = (
    await agentCommand(second.actor, "request.claim", {
      id: handed.requests[0].id,
    })
  ).value;
  assert.equal(
    claimed.context.recentMessages.some(
      (m: any) => m.body === "Investigate a design",
    ),
    true,
  );
  assert.equal(
    (await transact((s) => s.get(req.data.burstId, "burst").data.remaining))
      .value,
    4,
  );
  await assert.rejects(
    agentCommand({ ...second.actor, requestId: req.id }, "message.reply", {
      body: "Wrong request",
    }),
    /no longer active/,
  );
});

test("space artifacts have explicit audiences and agents cannot mint storage access or approvals", async () => {
  const first = await connectedAgent(0),
    second = await connectedAgent(1);
  const space = (
    await cmd("space.create", {
      name: "Research room",
      agentIds: [first.actor.id, second.actor.id],
    })
  ).value;
  const artifact = (
    await agentCommand(first.actor, "resource.create", {
      title: "Shared findings",
      text: "A durable note",
      spaceIds: [space.id],
      agentIds: [second.actor.id],
      tags: ["research"],
      pinned: true,
    })
  ).value;
  assert.equal(artifact.resource.data.pinned, true);
  assert.equal(
    (
      await agentCommand(second.actor, "resource.read", {
        id: artifact.resource.id,
      })
    ).value.revision.data.text,
    "A durable note",
  );
  const privateNote = (
    await agentCommand(first.actor, "resource.create", {
      title: "Private",
      text: "private",
      spaceIds: [space.id],
    })
  ).value;
  await assert.rejects(
    agentCommand(second.actor, "resource.read", {
      id: privateNote.resource.id,
    }),
    /not found/,
  );
  await assert.rejects(
    agentCommand(first.actor, "resource.create", {
      title: "Forged file",
      storageKey: randomUUID(),
    }),
    /file upload/,
  );
  await assert.rejects(
    agentCommand(first.actor, "scope.approve", { taskIds: [] }),
    /unavailable/,
  );
  const organized = await cmd("resource.organize", {
    id: privateNote.resource.id,
    expectedRevision: privateNote.resource.revision,
    spaceIds: [space.id],
    pinned: true,
  });
  assert.deepEqual(organized.value.data.agentIds, [first.actor.id]);
});

test("trusted uploads are idempotent by content and downloads recheck Resource grants", async () => {
  const { actor } = await connectedAgent();
  const { executeUploadedResource, authorizedDownload } =
    await import("../apps/server/src/domain/agent-access.js");
  const { storage } = await import("../apps/server/src/storage/local.js");
  const bytes = Buffer.from("saved file\n"),
    one = await storage.put(bytes),
    two = await storage.put(bytes);
  const command = {
    id: randomUUID(),
    type: "resource.create",
    input: { title: "File" },
  };
  const metadata = (r: any) => ({
    storageKey: r.key,
    checksum: r.checksum,
    size: r.size,
    filename: "file.txt",
    mimeType: "text/plain",
  });
  const first = await executeUploadedResource(actor, command, metadata(one));
  const retry = await executeUploadedResource(actor, command, metadata(two));
  assert.equal(first.value.resource.id, retry.value.resource.id);
  assert.equal(
    (await authorizedDownload(actor, first.value.resource.id)).storageKey,
    one.key,
  );
  await cmd("resource.grant", {
    id: first.value.resource.id,
    expectedRevision: first.value.resource.revision,
    agentIds: [],
    conversationIds: [],
    spaceIds: [],
    projectIds: [],
  });
  await assert.rejects(
    authorizedDownload(actor, first.value.resource.id),
    /not found/,
  );
});

test("expired external work is held, and late results are preserved without completing it", async () => {
  const { actor } = await connectedAgent();
  const sent = await cmd("message.send", {
    conversationId: conv.id,
    body: "Investigate",
    recipientIds: [actor.id],
  });
  const req = sent.value.requests[0];
  await agentCommand(actor, "request.claim", { id: req.id });
  await transact((s) =>
    s.patch(req.id, { leaseUntil: new Date(Date.now() - 1000).toISOString() }),
  );
  const { expireExternalClaims } =
    await import("../apps/server/src/domain/delivery.js");
  await expireExternalClaims();
  await assert.rejects(
    agentCommand(actor, "request.claim", { id: req.id }),
    /already claimed/,
  );
  const result = (
    await agentCommand({ ...actor, requestId: req.id }, "result.submit", {
      summary: "Partial result survived",
    })
  ).value;
  assert.equal(result.preserved, true);
  assert.equal(result.accepted, false);
  assert.equal(
    (await transact((s) => s.get(req.id, "request").data.status)).value,
    "needs_reconciliation",
  );
  assert.equal(
    (
      await transact(
        (s) =>
          s.get(result.resource.data.latestRevisionId, "revision").data.text,
      )
    ).value,
    "Partial result survived",
  );
});

test("revocation and stop prevent live replies and old read retries use current permissions", async () => {
  const { actor, credential } = await connectedAgent();
  const note = (
    await agentCommand(actor, "resource.create", {
      title: "Draft",
      text: "restricted",
    })
  ).value;
  const readId = randomUUID();
  await agentCommand(actor, "resource.read", { id: note.resource.id }, readId);
  await cmd("resource.grant", {
    id: note.resource.id,
    expectedRevision: note.resource.revision,
    agentIds: [],
    conversationIds: [],
  });
  await assert.rejects(
    agentCommand(actor, "resource.read", { id: note.resource.id }, readId),
    /not found/,
  );
  const sent = await cmd("message.send", {
    conversationId: conv.id,
    body: "Take a look",
    recipientIds: [actor.id],
  });
  await agentCommand(actor, "request.claim", { id: sent.value.requests[0].id });
  await cmd("conversation.stop", { id: conv.id });
  await assert.rejects(
    agentCommand(
      { ...actor, requestId: sent.value.requests[0].id },
      "message.reply",
      { body: "stopped" },
    ),
    /stopped/,
  );
  await cmd("credential.revoke", { id: credential.id });
  await assert.rejects(agentCommand(actor, "inbox.list"), /revoked/);
});

test("external accounts can pick up assigned chat tasks but cannot accept results or code without approval", async () => {
  const { actor } = await connectedAgent();
  const project = (
    await cmd("project.create", {
      name: "Agent research",
      agentIds: [actor.id],
    })
  ).value;
  const task = (
    await cmd("task.create", {
      title: "Compare designs",
      description: "Produce a comparison",
      criteria: ["Explain choices"],
      projectId: project.id,
      assigneeId: actor.id,
      execution: "chat",
    })
  ).value;
  await assert.rejects(
    agentCommand(actor, "request.claim", { taskId: task.id }),
    /ready/,
  );
  await cmd("task.update", {
    id: task.id,
    expectedRevision: task.revision,
    status: "ready",
  });
  const claim = (
    await agentCommand(actor, "request.claim", { taskId: task.id })
  ).value;
  await agentCommand(
    { ...actor, requestId: claim.request.id },
    "result.submit",
    { summary: "The comparison is ready." },
  );
  const final = (await transact((s) => s.get(task.id, "task"))).value;
  assert.equal(final.data.status, "review");
  await assert.rejects(
    agentCommand(actor, "task.update", {
      id: task.id,
      status: "accepted",
      expectedRevision: final.revision,
    }),
    /Only draft|owner/,
  );
});

test("channels and threads are real conversations and thread creation never dispatches", async () => {
  const channel = (
    await cmd("conversation.create", {
      title: "Design",
      kind: "channel",
      agentIds: agents.map((a) => a.id),
    })
  ).value;
  assert.equal(channel.data.kind, "channel");
  const original = (
    await cmd("message.send", {
      conversationId: channel.id,
      body: "A question",
      recipientIds: [agents[0].id],
    })
  ).value;
  const count = (await transact((s) => s.all("request").length)).value;
  const thread = (
    await cmd("conversation.thread", { messageId: original.message.id })
  ).value;
  assert.equal(thread.data.kind, "thread");
  assert.equal(thread.data.parentConversationId, channel.id);
  assert.equal(
    (await cmd("conversation.thread", { messageId: original.message.id })).value
      .id,
    thread.id,
  );
  assert.equal((await transact((s) => s.all("request").length)).value, count);
});

test("webhook wakeups are signed, durable, limited to request references, and do not claim work", async () => {
  const { actor } = await connectedAgent(0, "webhook");
  const sent = await cmd("message.send", {
    conversationId: conv.id,
    body: "Private request text",
    recipientIds: [actor.id],
  });
  const originalFetch = globalThis.fetch,
    calls: any[] = [];
  globalThis.fetch = (async (url: any, init: any) => {
    calls.push({ url, init });
    return new Response("", { status: 202 });
  }) as typeof fetch;
  try {
    const { deliverWebhooks } =
      await import("../apps/server/src/domain/delivery.js");
    await deliverWebhooks();
    await deliverWebhooks();
    assert.equal(calls.length, 1);
    const { createHmac } = await import("node:crypto");
    const headers = calls[0].init.headers;
    assert.equal(
      headers["X-Workspace-Signature"],
      `sha256=${createHmac("sha256", "test-signing-secret-for-wakeup").update(`${headers["X-Workspace-Timestamp"]}.${calls[0].init.body}`).digest("hex")}`,
    );
    assert.equal(calls[0].init.body.includes("Private request text"), false);
    assert.equal(
      (
        await transact(
          (s) => s.get(sent.value.requests[0].id, "request").data.status,
        )
      ).value,
      "pending",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("real HTTP and MCP routes use durable agent credentials and deny owner APIs", async () => {
  const { actor, credential } = await connectedAgent();
  const app = await buildServer();
  const headers = {
    host: "127.0.0.1:4310",
    authorization: `Bearer ${credential.token}`,
  };
  try {
    const response = await app.inject({
      method: "POST",
      url: "/api/agent/commands",
      headers,
      payload: { id: randomUUID(), type: "agent.me", input: {} },
    });
    assert.equal(response.statusCode, 200);
    assert.equal(response.json().value.agent.id, actor.id);
    assert.equal(
      (await app.inject({ method: "GET", url: "/api/snapshot", headers }))
        .statusCode,
      401,
    );
    const initialized = await app.inject({
      method: "POST",
      url: "/mcp",
      headers: { ...headers, accept: "application/json, text/event-stream" },
      payload: {
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: "2025-03-26",
          capabilities: {},
          clientInfo: { name: "connected-release-check", version: "1" },
        },
      },
    });
    const rpc = (response: any) =>
      response.headers["content-type"]?.startsWith("text/event-stream")
        ? JSON.parse(
            response.body
              .split("\n")
              .find((line: string) => line.startsWith("data: "))
              .slice(6),
          )
        : response.json();
    assert.equal(initialized.statusCode, 200);
    assert.equal(rpc(initialized).result.serverInfo.name, "agent-workspace");
    const call = await app.inject({
      method: "POST",
      url: "/mcp",
      headers: {
        ...headers,
        accept: "application/json, text/event-stream",
        "mcp-protocol-version": "2025-03-26",
      },
      payload: {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: {
          name: "agent_me",
          arguments: { commandId: randomUUID(), input: {} },
        },
      },
    });
    assert.equal(call.statusCode, 200);
    assert.equal(
      JSON.parse(rpc(call).result.content[0].text).value.agent.id,
      actor.id,
    );
  } finally {
    await app.close();
  }
});

test("standalone assignment is visible and task wakeup does not grant broader project access", async () => {
  const { actor } = await connectedAgent(0, "webhook");
  const task = (
    await cmd("task.create", {
      title: "Read a note",
      description: "Summarize the idea",
      criteria: ["One summary"],
      assigneeId: actor.id,
    })
  ).value;
  await cmd("task.update", {
    id: task.id,
    expectedRevision: task.revision,
    status: "ready",
  });
  const inbox = (await agentCommand(actor, "inbox.list")).value;
  assert.equal(inbox.tasks[0].task.id, task.id);
  const originalFetch = globalThis.fetch,
    calls: any[] = [];
  globalThis.fetch = (async (_url: any, init: any) => {
    calls.push(JSON.parse(init.body));
    return new Response(null, { status: 204 });
  }) as typeof fetch;
  try {
    const { deliverWebhooks } =
      await import("../apps/server/src/domain/delivery.js");
    await deliverWebhooks();
    assert.equal(calls[0].taskId, task.id);
  } finally {
    globalThis.fetch = originalFetch;
  }
  const claimed = (
    await agentCommand(actor, "request.claim", { taskId: task.id })
  ).value;
  assert.equal(claimed.claimed, true);
  assert.equal(
    (
      await agentCommand(
        { ...actor, requestId: claimed.request.id },
        "task.list",
      )
    ).value.items.length,
    1,
  );
  assert.equal(
    (await agentCommand(actor, "project.list")).value.items.length,
    0,
  );
});

test("parent membership edits revoke thread access and renamed accounts keep identity", async () => {
  const sent = (
    await cmd("message.send", {
      conversationId: conv.id,
      body: "Discuss this",
      recipientIds: [agents[0].id],
    })
  ).value;
  const thread = (
    await cmd("conversation.thread", { messageId: sent.message.id })
  ).value;
  const current = (await transact((s) => s.get(conv.id, "conversation"))).value;
  await cmd("conversation.update", {
    id: conv.id,
    expectedRevision: current.revision,
    agentIds: [agents[1].id],
  });
  assert.deepEqual(
    (await transact((s) => s.get(thread.id, "conversation").data.agentIds))
      .value,
    [agents[1].id],
  );
  const agent = (await transact((s) => s.get(agents[0].id, "agent"))).value;
  const updated = (
    await cmd("agent.update", {
      id: agent.id,
      expectedRevision: agent.revision,
      name: "Custom account",
      color: "#336699",
    })
  ).value;
  assert.equal(updated.id, agent.id);
  assert.equal(updated.data.name, "Custom account");
  assert.equal(updated.data.color, "#336699");
});

test("agents discover permitted colleagues without private endpoints and lists have bounded cursors", async () => {
  const { actor } = await connectedAgent();
  const hidden = (
    await cmd("agent.create", {
      name: "Private specialist",
      role: "Not in this room",
    })
  ).value;
  const visible = (await agentCommand(actor, "agent.list")).value;
  assert.equal(
    visible.items.some((r: any) => r.id === hidden.id),
    false,
  );
  assert.equal(
    visible.items.some((r: any) => r.id === agents[1].id),
    true,
  );
  assert.equal(
    visible.items.some((r: any) => "webhookUrl" in r.data),
    false,
  );
  for (let n = 0; n < 3; n++)
    await agentCommand(actor, "resource.create", {
      title: `Note ${n}`,
      text: "saved",
    });
  const first = (await agentCommand(actor, "resource.list", { limit: 2 }))
    .value;
  assert.equal(first.items.length, 2);
  assert(first.nextAfterId);
  const second = (
    await agentCommand(actor, "resource.list", {
      limit: 2,
      afterId: first.nextAfterId,
    })
  ).value;
  assert.equal(second.items.length, 1);
  assert.equal(second.nextAfterId, null);
});

test("OAuth access rotation preserves connection identity and revocation while replacing the bearer", async () => {
  const { credential } = await connectedAgent();
  const { refreshAgentCredential, authenticateAgent } =
    await import("../apps/server/src/domain/agent-access.js");
  await pool.query(
    "UPDATE agent_credentials SET expires_at=now()-interval '1 minute' WHERE id=$1",
    [credential.id],
  );
  const renewed = await refreshAgentCredential(credential.id);
  assert.equal(renewed.id, credential.id);
  await assert.rejects(authenticateAgent(credential.token), /expired, revoked/);
  assert.equal(
    (await authenticateAgent(renewed.token)).credentialId,
    credential.id,
  );
  await cmd("credential.revoke", { id: credential.id });
  await assert.rejects(refreshAgentCredential(credential.id), /revoked/);
});
