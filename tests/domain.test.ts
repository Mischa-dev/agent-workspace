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
    "TRUNCATE records,events,commands,connector_credentials,owner_sessions RESTART IDENTITY",
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
    const body = Buffer.from(
      `--${boundary}\r\nContent-Disposition: form-data; name="title"\r\n\r\nUploaded note\r\n--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="note.txt"\r\nContent-Type: text/plain\r\n\r\nimmutable bytes\r\n--${boundary}--\r\n`,
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
