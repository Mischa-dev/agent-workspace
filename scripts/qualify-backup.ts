import pg from "pg";
import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import path from "node:path";
import assert from "node:assert/strict";
const root = process.cwd(),
  configPath = path.join(root, ".local/browser-config.json");
const hash = (b: any) => createHash("sha256").update(b).digest("hex");
const mainHash = hash(readFileSync(".local/config.json"));
const original = JSON.parse(readFileSync(configPath, "utf8"));
const before = new pg.Pool({ connectionString: original.databaseUrl });
const template = (
  await before.query("SELECT data FROM records WHERE kind='request' LIMIT 1")
).rows[0].data;
const pendingId = randomUUID(),
  unknownId = randomUUID(),
  runId = randomUUID();
await before.query(
  "INSERT INTO records(id,kind,data) VALUES($1,'request',$2),($3,'request',$4),($5,'run',$6)",
  [
    pendingId,
    JSON.stringify({ ...template, status: "pending", runId: null }),
    unknownId,
    JSON.stringify({ ...template, status: "running", runId }),
    runId,
    JSON.stringify({
      kind: "response",
      agentId: template.agentId,
      requestId: unknownId,
      status: "running",
      events: [],
      packet: "Synthetic interrupted execution fixture",
      manifest: {},
      digest: "fixture",
    }),
  ],
);
const counts = (
  await before.query(
    "SELECT kind,count(*)::int n FROM records GROUP BY kind ORDER BY kind",
  )
).rows;
const files = (
  await before.query(
    "SELECT data FROM records WHERE kind='revision' AND data ? 'storageKey'",
  )
).rows;
const run = (action: string, extra: string[] = []) => {
  const r = spawnSync(path.join(root, "workspace"), [action, ...extra], {
    env: { ...process.env, WORKSPACE_CONFIG_PATH: configPath },
    encoding: "utf8",
    timeout: 60000,
  });
  if (r.status !== 0) throw new Error(`${action} failed: ${r.stderr}`);
  return r.stdout;
};
let started = false;
try {
  const output = run("backup");
  const backup = output.match(/Backup created: (.+)/)![1];
  run("restore", [backup]);
  const restored = JSON.parse(readFileSync(configPath, "utf8"));
  assert.notEqual(restored.databaseUrl, original.databaseUrl);
  assert.notEqual(restored.dataDir, original.dataDir);
  assert.equal(hash(readFileSync(".local/config.json")), mainHash);
  const after = new pg.Pool({ connectionString: restored.databaseUrl });
  try {
    assert.deepEqual(
      (
        await after.query(
          "SELECT kind,count(*)::int n FROM records GROUP BY kind ORDER BY kind",
        )
      ).rows,
      counts,
    );
    assert.equal(
      (
        await after.query(
          "SELECT value FROM settings WHERE key='dispatchEnabled'",
        )
      ).rows[0].value,
      false,
    );
    assert.equal(
      (await after.query("SELECT count(*)::int n FROM owner_sessions")).rows[0]
        .n,
      0,
    );
    assert.equal(
      (await after.query("SELECT count(*)::int n FROM connector_credentials"))
        .rows[0].n,
      0,
    );
    assert.equal(
      (await after.query("SELECT data FROM records WHERE id=$1", [pendingId]))
        .rows[0].data.status,
      "cancelled",
    );
    assert.equal(
      (await after.query("SELECT data FROM records WHERE id=$1", [runId]))
        .rows[0].data.status,
      "needs_reconciliation",
    );
    for (const { data } of files) {
      assert.equal(
        hash(
          readFileSync(path.join(restored.dataDir, "files", data.storageKey)),
        ),
        data.checksum,
      );
    }
    run("start");
    started = true;
    await new Promise((r) => setTimeout(r, 6000));
    assert.equal(
      (
        await after.query(
          "SELECT count(*)::int n FROM records WHERE kind='run'",
        )
      ).rows[0].n,
      counts.find((r: any) => r.kind === "run").n,
    );
    const health = (await (
      await fetch("http://127.0.0.1:4310/health")
    ).json()) as any;
    assert.equal(health.dispatchEnabled, false);
    assert.equal(health.workerOnline, true);
    const report = {
      passed: true,
      verifiedAt: new Date().toISOString(),
      backup,
      originalDatabase: new URL(original.databaseUrl).pathname.slice(1),
      restoredDatabase: new URL(restored.databaseUrl).pathname.slice(1),
      counts,
      filesVerified: files.length,
      checks: [
        "New database and file directory; originals preserved",
        "Main application config untouched",
        "All record counts and attachment checksums retained",
        "Owner sessions and scoped runtime credentials cleared",
        "Pending request cancelled; unknown execution held",
        "Built server and real worker restarted with dispatch disabled; no replay",
      ],
    };
    writeFileSync(
      "evidence/backup-restore.json",
      JSON.stringify(report, null, 2),
    );
    console.log(JSON.stringify(report, null, 2));
  } finally {
    await after.end();
  }
} finally {
  if (started) run("stop");
  await before.end();
}
