import { readFileSync, existsSync } from "node:fs";
import path from "node:path";
export const root = process.env.WORKSPACE_ROOT || process.cwd();
const envPath =
  process.env.WORKSPACE_CONFIG_PATH || path.join(root, ".local/config.json");
const local = existsSync(envPath)
  ? JSON.parse(readFileSync(envPath, "utf8"))
  : {};
const port = Number(process.env.PORT || local.port || 4310);
const publicUrl = new URL(
  process.env.PUBLIC_ORIGIN || local.publicOrigin || `http://127.0.0.1:${port}`,
);
const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(
  publicUrl.hostname,
);
if (
  publicUrl.username ||
  publicUrl.password ||
  publicUrl.pathname !== "/" ||
  publicUrl.search ||
  publicUrl.hash ||
  !["http:", "https:"].includes(publicUrl.protocol) ||
  (!loopback && publicUrl.protocol !== "https:")
)
  throw new Error(
    "PUBLIC_ORIGIN must be an HTTPS origin (loopback HTTP is allowed).",
  );
export const config = {
  root,
  port,
  publicOrigin: publicUrl.origin,
  bindHost: process.env.BIND_HOST || local.bindHost || "127.0.0.1",
  secureCookies: publicUrl.protocol === "https:",
  trustProxy: process.env.TRUST_PROXY
    ? process.env.TRUST_PROXY.split(",")
        .map((value) => value.trim())
        .filter(Boolean)
    : (false as false),
  allowedHosts: [
    ...new Set([
      publicUrl.host,
      ...(loopback ? [`127.0.0.1:${port}`, `localhost:${port}`] : []),
    ]),
  ],
  allowedOrigins: [
    ...new Set([
      publicUrl.origin,
      ...(loopback
        ? [`http://127.0.0.1:${port}`, `http://localhost:${port}`]
        : []),
      ...(process.env.WORKSPACE_DEV === "1" ? ["http://127.0.0.1:4311"] : []),
    ]),
  ],
  agentOrigins: (process.env.AGENT_ALLOWED_ORIGINS || "")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean),
  databaseUrl: process.env.DATABASE_URL || local.databaseUrl || "",
  dataDir:
    process.env.DATA_DIR || local.dataDir || path.join(root, ".local/data"),
  webhookTokenHash: local.webhookTokenHash || "",
  ownerTokenHash: process.env.OWNER_TOKEN_HASH || local.ownerTokenHash || "",
  codexPath: process.env.CODEX_PATH || local.codexPath || "codex",
  test: process.env.NODE_ENV === "test",
};
export const origin = config.publicOrigin;
if (!config.databaseUrl)
  throw new Error("Run pnpm setup first (dedicated PostgreSQL required).");
