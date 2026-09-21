import pg from "pg";
import {
  readFileSync,
  mkdirSync,
  writeFileSync,
  readFileSync as read,
  existsSync,
} from "node:fs";
import { spawn } from "node:child_process";
import path from "node:path";
import { randomUUID } from "node:crypto";
const cfg = JSON.parse(readFileSync(".local/config.json", "utf8")),
  dbName = `workspace_live_${Date.now()}`;
const url = new URL(cfg.databaseUrl);
url.pathname = "/postgres";
const admin = new pg.Pool({ connectionString: url.toString() });
await admin.query(`CREATE DATABASE ${dbName}`);
url.pathname = `/${dbName}`;
const dataDir = path.resolve(".local/qualification", dbName);
mkdirSync(dataDir, { recursive: true, mode: 0o700 });
process.env.DATABASE_URL = url.toString();
process.env.DATA_DIR = dataDir;
const { pool, db } = await import("../apps/server/src/db.js");
const { migrate } = await import("drizzle-orm/node-postgres/migrator");
await migrate(db, { migrationsFolder: "apps/server/drizzle" });
const { seed, execute, transact } =
  await import("../apps/server/src/domain/commands.js");
const { git } = await import("../apps/server/src/domain/repository.js");
await seed();
const cmd = async (type: string, input: any) => {
  if (type === "scope.approve")
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
  return execute(
    { id: "owner", owner: true },
    { id: randomUUID(), type, input },
  );
};
const initial = (
  await transact((s) => ({
    agents: s.all("agent"),
    conversations: s.all("conversation"),
  }))
).value;
const group = initial.conversations.find((c) => c.data.kind === "group")!,
  atlas = initial.agents.find((a) => a.data.name === "Atlas")!,
  ember = initial.agents.find((a) => a.data.name === "Ember")!,
  dm = initial.conversations.find(
    (c) => c.data.kind === "dm" && c.data.agentIds[0] === atlas.id,
  )!;
const evidence: any = {
  startedAt: new Date().toISOString(),
  database: dbName,
  live: true,
  checks: [],
};
const worker = spawn(
  process.execPath,
  ["--import", "tsx", "apps/server/src/worker.ts"],
  {
    env: { ...process.env, DATABASE_URL: url.toString(), DATA_DIR: dataDir },
    stdio: ["ignore", "pipe", "pipe"],
  },
);
let log = "";
worker.stdout.on("data", (b) => {
  log += b;
});
worker.stderr.on("data", (b) => {
  log += b;
});
async function waitFor(
  label: string,
  predicate: () => Promise<any>,
  timeout = 240000,
) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const result = await predicate();
    if (result) return result;
    if (worker.exitCode !== null)
      throw new Error(`Worker exited: ${log.slice(-2000)}`);
    await new Promise((r) => setTimeout(r, 1500));
  }
  throw new Error(`${label} timed out`);
}
async function waitRequest(id: string) {
  return waitFor("response", async () => {
    const r = (await transact((s) => s.get(id, "request"))).value;
    if (["failed", "cancelled", "needs_reconciliation"].includes(r.data.status))
      throw new Error(`Response ${r.data.status}: ${r.data.error}`);
    return r.data.status === "answered" ? r : null;
  });
}
try {
  const message = (
    await cmd("message.send", {
      conversationId: dm.id,
      body: "This is an authorized application qualification. Use the workspace tool to create a real note Resource titled “Live database proof” with exact text “Workspace resource roundtrip passed”. Then retrieve that Resource with resource.read and confirm the exact stored text. Do not just describe the steps.",
    })
  ).value;
  await waitRequest(message.requests[0].id);
  const resource = (
    await transact((s) =>
      s.all("resource").find((r) => r.data.title === "Live database proof"),
    )
  ).value;
  if (!resource) throw new Error("Model did not create persistent Resource");
  evidence.checks.push({
    name: "Live DM and database Resource create/read",
    passed: true,
    resourceId: resource.id,
  });
  console.log("PASS live DM and database Resource roundtrip");
  const secret = "PRIVATE_DM_SENTINEL_93A7";
  await cmd("resource.create", {
    title: "Private DM canary",
    text: secret,
    conversationIds: [dm.id],
  });
  const groupMessage = (
    await cmd("message.send", {
      conversationId: group.id,
      mode: "independent",
      body: "Give one short independent suggestion for organizing a small project. Use resource.list first. Mention whether any Resources were visible. Do not create files or Resources in this round.",
    })
  ).value;
  await Promise.all(groupMessage.requests.map((r: any) => waitRequest(r.id)));
  const groupResult = (
    await transact((s) => ({
      messages: s
        .all("message")
        .filter((m) => m.data.burstId === groupMessage.message.data.burstId),
      runs: s
        .all("run")
        .filter((r) =>
          groupMessage.requests.some((q: any) => q.id === r.data.requestId),
        ),
      burst: s.get(groupMessage.message.data.burstId),
    }))
  ).value;
  if (groupResult.messages.some((m) => m.data.body.includes(secret)))
    throw new Error("Private canary leaked");
  if (
    !groupResult.burst.data.published ||
    groupResult.messages.some((m) => m.data.staged)
  )
    throw new Error("Round did not publish");
  const threads = groupResult.runs.map((r) => r.data.runtime?.threadId);
  if (new Set(threads).size !== 2 || threads.some((t) => !t))
    throw new Error("Sessions were not isolated");
  evidence.checks.push({
    name: "Real independent group, separate runtime threads, private DM exclusion",
    passed: true,
    threads,
  });
  console.log("PASS live independent group and private-context exclusion");
  const repo = path.join(dataDir, "disposable-repo");
  mkdirSync(repo);
  await git(repo, ["init", "-b", "main"]);
  writeFileSync(
    path.join(repo, "README.md"),
    "# Disposable qualification repository\n",
  );
  await git(repo, ["add", "README.md"]);
  await git(repo, [
    "-c",
    "user.name=Workspace Test",
    "-c",
    "user.email=test@workspace.invalid",
    "commit",
    "-m",
    "Initial fixture",
  ]);
  const project = (
    await cmd("project.create", {
      name: "Execution qualification",
      repository: repo,
      targetBranch: "main",
    })
  ).value;
  const task = (
    await cmd("task.create", {
      title: "Create a greeting file",
      description:
        "Using the approved repository tools, create greeting.txt with exactly: Hello from the private workspace! followed by a newline. Run repo.check. Report what changed.",
      criteria: [
        "greeting.txt contains the exact requested text",
        "repo.check passes",
      ],
      projectId: project.id,
      execution: "code",
    })
  ).value;
  const scope = (
    await cmd("scope.approve", {
      title: "Disposable repository test",
      taskIds: [task.id],
      minutes: 5,
      maxWorkers: 1,
    })
  ).value;
  const session = (await cmd("session.start", { scopeId: scope.id })).value;
  const result = await waitFor("execution", async () => {
    const t = (await transact((s) => s.get(task.id, "task"))).value;
    if (t.data.status === "review")
      return (await transact((s) => s.get(t.data.runId, "run"))).value;
    if (t.data.blocker) throw new Error(t.data.blocker);
    return null;
  });
  if (result.data.status !== "completed")
    throw new Error(`Execution failed: ${result.data.error}`);
  if (
    read(path.join(result.data.worktree, "greeting.txt"), "utf8") !==
    "Hello from the private workspace!\n"
  )
    throw new Error("Unexpected file contents");
  if (existsSync(path.join(repo, "greeting.txt")))
    throw new Error("Target checkout was modified");
  if (!result.data.checkpoint?.commit || !result.data.checkpoint.changed)
    throw new Error("Expected real local checkpoint");
  if (!result.data.resultResourceId)
    throw new Error("Result Resource was not retained");
  evidence.checks.push({
    name: "Real bounded local work session and isolated worktree write",
    passed: true,
    worktree: result.data.worktree,
    commit: result.data.checkpoint.commit,
    resultResourceId: result.data.resultResourceId,
    sessionId: session.id,
    limitation:
      "repo.check performs patch formatting checks; arbitrary project test commands remain disabled.",
  });
  console.log(
    "PASS live execution, isolated worktree, and target checkout preservation",
  );
  await waitFor(
    "first session completion",
    async () =>
      (await transact((s) => s.get(session.id))).value.data.status ===
      "complete",
  );
  const parallelTasks = [];
  for (const filename of ["parallel-one.json", "parallel-two.json"])
    parallelTasks.push(
      (
        await cmd("task.create", {
          title: `Create ${filename}`,
          description: `In the approved worktree, create ${filename} containing exactly {"qualified":true} and a newline. Run repo.check. Do not edit any other file.`,
          criteria: ["Requested JSON exists", "repo.check passes"],
          projectId: project.id,
          execution: "code",
          parallel: true,
        })
      ).value,
    );
  const pairScope = (
    await cmd("scope.approve", {
      title: "Two-worker qualification",
      taskIds: parallelTasks.map((t) => t.id),
      minutes: 5,
      maxWorkers: 2,
    })
  ).value;
  const pairSession = (await cmd("session.start", { scopeId: pairScope.id }))
    .value;
  const pairRuns = await Promise.all(
    parallelTasks.map((task) =>
      waitFor("parallel task", async () => {
        const t = (await transact((s) => s.get(task.id))).value;
        if (t.data.status === "review")
          return (await transact((s) => s.get(t.data.runId))).value;
        if (t.data.blocker) throw new Error(t.data.blocker);
        return null;
      }),
    ),
  );
  for (let index = 0; index < pairRuns.length; index++) {
    const r = pairRuns[index];
    if (r.data.status !== "completed" || !r.data.checkpoint?.changed)
      throw new Error("Parallel task did not checkpoint");
    const filename = index === 0 ? "parallel-one.json" : "parallel-two.json";
    if (
      JSON.parse(read(path.join(r.data.worktree, filename), "utf8"))
        .qualified !== true
    )
      throw new Error("Parallel output mismatch");
    if (existsSync(path.join(repo, filename)))
      throw new Error("Parallel work changed target checkout");
  }
  if (pairRuns[0].data.worktree === pairRuns[1].data.worktree)
    throw new Error("Parallel tasks shared a checkout");
  const spans = pairRuns.map((r) => ({
    start: Date.parse(r.createdAt),
    end: Date.parse(r.updatedAt),
  }));
  if (
    Math.max(...spans.map((s) => s.start)) >=
    Math.min(...spans.map((s) => s.end))
  )
    throw new Error("Two task runs did not overlap");
  evidence.checks.push({
    name: "Two overlapping real coding runs with separate worktrees and checkpoints",
    passed: true,
    sessionId: pairSession.id,
    runs: pairRuns.map((r) => ({
      runId: r.id,
      worktree: r.data.worktree,
      commit: r.data.checkpoint.commit,
      started: r.createdAt,
      finished: r.updatedAt,
    })),
  });
  console.log(
    "PASS two overlapping real coding runs and separate local checkpoints",
  );
  evidence.passed = true;
} catch (e: any) {
  evidence.passed = false;
  evidence.error = e.message;
  console.error("QUALIFICATION FAILED:", e.message);
  process.exitCode = 1;
} finally {
  worker.kill("SIGTERM");
  await new Promise((r) => worker.once("exit", r));
  writeFileSync(path.join(dataDir, "worker.log"), log, { mode: 0o600 });
  evidence.finishedAt = new Date().toISOString();
  mkdirSync("evidence", { recursive: true });
  writeFileSync("evidence/app-live.json", JSON.stringify(evidence, null, 2));
  await pool.end();
  await admin.end();
  console.log("Evidence saved; qualification worker stopped.");
}
