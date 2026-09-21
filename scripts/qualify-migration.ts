import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import assert from "node:assert/strict";
import pg from "pg";
import { drizzle } from "drizzle-orm/node-postgres";
import { migrate } from "drizzle-orm/node-postgres/migrator";
const cfg = JSON.parse(readFileSync(".local/config.json", "utf8"));
const pool = new pg.Pool({ connectionString: cfg.databaseUrl });
try {
  const fingerprint = async () => {
    const rows = (await pool.query("SELECT * FROM records ORDER BY id")).rows;
    return {
      count: rows.length,
      digest: createHash("sha256").update(JSON.stringify(rows)).digest("hex"),
    };
  };
  const before = await fingerprint();
  await migrate(drizzle(pool), { migrationsFolder: "apps/server/drizzle" });
  const after = await fingerprint();
  assert.deepEqual(after, before);
  const report = {
    passed: true,
    verifiedAt: new Date().toISOString(),
    recordsPreserved: before.count,
    recordFingerprintUnchanged: true,
    agentTablesAvailable: (
      await pool.query(
        "SELECT to_regclass('public.agent_credentials') IS NOT NULL AND to_regclass('public.oauth_state') IS NOT NULL ok",
      )
    ).rows[0].ok,
    modelCalls: 0,
  };
  writeFileSync(
    "evidence/connected-migration.json",
    JSON.stringify(report, null, 2) + "\n",
  );
  console.log(JSON.stringify(report, null, 2));
} finally {
  await pool.end();
}
