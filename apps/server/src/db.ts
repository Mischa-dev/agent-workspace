import pg from "pg";
import { drizzle } from "drizzle-orm/node-postgres";
import { config } from "./config.js";
export const pool = new pg.Pool({
  connectionString: config.databaseUrl,
  max: 8,
});
export const db = drizzle(pool);
export async function setting(key: string, fallback: any = null) {
  const r = await pool.query("SELECT value FROM settings WHERE key=$1", [key]);
  return r.rows[0]?.value ?? fallback;
}
export async function setSetting(key: string, value: any) {
  await pool.query(
    "INSERT INTO settings(key,value) VALUES($1,$2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    [key, JSON.stringify(value)],
  );
}
