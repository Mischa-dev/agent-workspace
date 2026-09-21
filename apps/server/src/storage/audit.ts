import { pool, setSetting } from "../db.js";
import { storage } from "./local.js";
import { createHash } from "node:crypto";
export async function auditStorage() {
  const rows = (
    await pool.query(
      "SELECT id,data FROM records WHERE kind='revision' AND data ? 'storageKey'",
    )
  ).rows;
  const report = await storage.reconcile(
    new Set(rows.map((r) => r.data.storageKey)),
  );
  const mismatched: string[] = [];
  for (const r of rows) {
    if (report.missing.includes(r.data.storageKey)) continue;
    try {
      const bytes = await storage.get(r.data.storageKey);
      if (
        bytes.length !== r.data.size ||
        createHash("sha256").update(bytes).digest("hex") !== r.data.checksum
      )
        mismatched.push(r.id);
    } catch {
      mismatched.push(r.id);
    }
  }
  const result = {
    ...report,
    mismatched,
    checkedAt: new Date().toISOString(),
    ok: !report.missing.length && !mismatched.length,
  };
  await setSetting("storageHealth", result);
  return result;
}
