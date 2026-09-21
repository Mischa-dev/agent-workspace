import pg from "pg";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { spawn } from "node:child_process";
import path from "node:path";
import { randomUUID } from "node:crypto";
const cfg = JSON.parse(readFileSync(".local/config.json", "utf8")),
  name = `workspace_recovery_${Date.now()}`,
  u = new URL(cfg.databaseUrl);
u.pathname = "/postgres";
const admin = new pg.Pool({ connectionString: u.toString() });
await admin.query(`CREATE DATABASE ${name}`);
u.pathname = `/${name}`;
process.env.DATABASE_URL = u.toString();
process.env.DATA_DIR = path.resolve(".local/qualification", name);
mkdirSync(process.env.DATA_DIR, { recursive: true });
const { pool, db } = await import("../apps/server/src/db.js");
const { migrate } = await import("drizzle-orm/node-postgres/migrator");
await migrate(db, { migrationsFolder: "apps/server/drizzle" });
const { seed, transact, execute } =
  await import("../apps/server/src/domain/commands.js");
await seed();
const c = (await transact((s) => s.all("conversation")[0])).value;
const cmd = (type: string, input: any) =>
  execute({ id: "owner", owner: true }, { id: randomUUID(), type, input });
let worker: ReturnType<typeof spawn>,
  logs = "";
const start = () => {
  worker = spawn(
    process.execPath,
    ["--import", "tsx", "apps/server/src/worker.ts"],
    { env: process.env, stdio: ["ignore", "pipe", "pipe"] },
  );
  worker.stdout!.on("data", (b) => (logs += b));
  worker.stderr!.on("data", (b) => (logs += b));
};
const stop = async () => {
  if (worker.exitCode !== null) return;
  worker.kill("SIGTERM");
  await new Promise((r) => worker.once("exit", r));
};
const evidence: any = {
  startedAt: new Date().toISOString(),
  database: name,
  checks: [],
};
const wait = async (fn: () => Promise<any>, ms = 90000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const r = await fn();
    if (r) return r;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("Qualification timed out");
};
try {
  start();
  const sent = (
    await cmd("message.send", {
      conversationId: c.id,
      body: "For a cancellation qualification, write a detailed numbered list of 100 practical questions someone could ask while planning a large personal software project. This is a synthetic local test.",
    })
  ).value;
  const req = sent.requests[0];
  const running = await wait(async () => {
    return (
      await transact((s) =>
        s
          .all("run")
          .find(
            (r) =>
              r.data.requestId === req.id &&
              r.data.events.some((e: any) => e.type === "runtime.accepted"),
          ),
      )
    ).value;
  });
  await cmd("conversation.stop", { id: c.id });
  const terminal = await wait(async () => {
    const r = (await transact((s) => s.get(running.id))).value;
    return [
      "cancelled",
      "completed",
      "needs_reconciliation",
      "failed",
    ].includes(r.data.status)
      ? r
      : null;
  });
  if (
    terminal.data.status !== "cancelled" ||
    !terminal.data.events.some(
      (e: any) =>
        e.type === "runtime.terminal" && e.data.status === "interrupted",
    )
  )
    throw new Error(
      `Expected observed interruption; got ${terminal.data.status}`,
    );
  evidence.checks.push({
    name: "Live Stop followed by observed runtime interruption",
    passed: true,
    runId: terminal.id,
  });
  console.log("PASS live cancellation observed at runtime");
  await stop();
  const { claimResponse } = await import("../apps/server/src/worker.js");
  await cmd("conversation.continue", { id: c.id });
  const second = (
    await cmd("message.send", {
      conversationId: c.id,
      body: "This queued request is used only to verify crash recovery.",
    })
  ).value;
  const claimed = (await claimResponse(second.requests[0].id)).value;
  await transact((s) =>
    s.patch(claimed!.id, {
      status: "dispatching",
      runtime: { acceptance: "unknown" },
    }),
  );
  start();
  const recovered = await wait(async () => {
    const r = (await transact((s) => s.get(claimed!.id))).value;
    return r.data.status === "needs_reconciliation" ? r : null;
  });
  await new Promise((r) => setTimeout(r, 6000));
  const after = (
    await transact((s) => ({
      run: s.get(recovered.id),
      request: s.get(second.requests[0].id),
      allRuns: s
        .all("run")
        .filter((r) => r.data.requestId === second.requests[0].id),
    }))
  ).value;
  if (
    after.request.data.status !== "needs_reconciliation" ||
    after.allRuns.length !== 1 ||
    after.run.data.events.length
  )
    throw new Error("Uncertain work was redispatched");
  evidence.checks.push({
    name: "Worker restart holds uncertain dispatch without replay",
    passed: true,
    fixture:
      "Synthetic durable uncertain receipt; real PostgreSQL, pg-boss and process restart",
  });
  console.log("PASS process restart and no redispatch of uncertain work");
  evidence.passed = true;
} catch (e: any) {
  evidence.passed = false;
  evidence.error = e.message;
  process.exitCode = 1;
  console.error(e.message);
} finally {
  await stop();
  evidence.finishedAt = new Date().toISOString();
  writeFileSync(
    "evidence/recovery-live.json",
    JSON.stringify(evidence, null, 2),
  );
  writeFileSync(path.join(process.env.DATA_DIR, "worker.log"), logs);
  await pool.end();
  await admin.end();
}
