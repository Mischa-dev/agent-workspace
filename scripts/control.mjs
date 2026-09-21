import {
  existsSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
  chmodSync,
  openSync,
  closeSync,
  cpSync,
} from "node:fs";
import { spawnSync, spawn } from "node:child_process";
import { randomBytes, createHash } from "node:crypto";
import path from "node:path";
const root = path.resolve(import.meta.dirname, ".."),
  local = path.join(root, ".local");
const node24 = path.join(
  process.env.HOME,
  ".local/share/fnm/node-versions/v24.18.1/installation/bin",
);
const env = {
  ...process.env,
  PATH: `${node24}:/opt/homebrew/bin:${process.env.PATH}`,
  DEVELOPER_DIR: "/Library/Developer/CommandLineTools",
  WORKSPACE_ROOT: root,
};
mkdirSync(local, { recursive: true, mode: 0o700 });
chmodSync(local, 0o700);
const configPath =
  process.env.WORKSPACE_CONFIG_PATH || path.join(local, "config.json");
const run = (cmd, args, options = {}) => {
  const r = spawnSync(cmd, args, {
    cwd: root,
    env,
    stdio: "inherit",
    ...options,
  });
  if (r.status !== 0) throw new Error(`${cmd} exited ${r.status}`);
  return r;
};
const pgData = path.join(local, "postgres"),
  pgLog = path.join(local, "postgres.log");
function pgRunning() {
  return (
    spawnSync("/opt/homebrew/bin/pg_ctl", ["-D", pgData, "status"], {
      env,
      stdio: "ignore",
    }).status === 0
  );
}
function pgStart() {
  if (!pgRunning())
    run("/opt/homebrew/bin/pg_ctl", ["-D", pgData, "-l", pgLog, "-w", "start"]);
}
function config() {
  if (!existsSync(configPath)) throw new Error("Run pnpm setup first.");
  return JSON.parse(readFileSync(configPath, "utf8"));
}
function databaseEnvironment(databaseUrl) {
  const url = new URL(databaseUrl);
  return {
    ...env,
    PGHOST: url.hostname,
    PGPORT: url.port || "5432",
    PGUSER: decodeURIComponent(url.username),
    PGPASSWORD: decodeURIComponent(url.password),
    PGDATABASE: url.pathname.slice(1),
  };
}
function start(name, args) {
  const pidPath = path.join(local, `${name}.pid`);
  if (existsSync(pidPath)) {
    try {
      const priorPid = Number(readFileSync(pidPath, "utf8"));
      if (isOurProcess(priorPid)) {
        console.log(`${name} already running`);
        return;
      }
    } catch {}
  }
  const fd = openSync(path.join(local, `${name}.log`), "a", 0o600);
  const absoluteArgs = args.map((a) =>
    a.startsWith("apps/") || a.startsWith("dist/") ? path.join(root, a) : a,
  );
  const child = spawn(path.join(node24, "node"), absoluteArgs, {
    cwd: root,
    env: {
      ...env,
      ...(process.argv[2] === "dev" ? { WORKSPACE_DEV: "1" } : {}),
    },
    detached: true,
    stdio: ["ignore", fd, fd],
  });
  writeFileSync(pidPath, String(child.pid));
  closeSync(fd);
  child.unref();
  console.log(`${name} started (${child.pid}).`);
}
function isOurProcess(pid) {
  const r = spawnSync("/bin/ps", ["-p", String(pid), "-o", "command="], {
    encoding: "utf8",
  });
  return r.status === 0 && r.stdout.includes(root);
}
async function stop() {
  const pids = [];
  for (const name of ["vite", "worker", "server"]) {
    const file = path.join(local, `${name}.pid`);
    if (!existsSync(file)) continue;
    const pid = Number(readFileSync(file, "utf8"));
    if (!isOurProcess(pid)) continue;
    pids.push(pid);
    try {
      process.kill(-pid, "SIGTERM");
    } catch {}
  }
  const deadline = Date.now() + 30000;
  while (pids.some(isOurProcess) && Date.now() < deadline)
    await new Promise((r) => setTimeout(r, 250));
  if (pids.some(isOurProcess))
    throw new Error(
      "Shutdown did not finish within 30 seconds. Data preserved; inspect process logs before backup.",
    );
  console.log(
    "Application stopped; logs and data preserved. PostgreSQL remains available.",
  );
}
const action = process.argv[2];
if (action === "setup") {
  if (!existsSync(configPath)) {
    const password = randomBytes(24).toString("hex"),
      token = randomBytes(32).toString("base64url");
    const pwfile = path.join(local, "pg-password");
    writeFileSync(pwfile, password, { mode: 0o600 });
    if (!existsSync(pgData))
      run("/opt/homebrew/bin/initdb", [
        "-D",
        pgData,
        "-U",
        "workspace",
        "--auth-local=trust",
        "--auth-host=scram-sha-256",
        "--pwfile",
        pwfile,
        "--encoding=UTF8",
      ]);
    const pgconfig = path.join(pgData, "postgresql.conf");
    writeFileSync(
      pgconfig,
      readFileSync(pgconfig, "utf8") +
        `\nlisten_addresses='127.0.0.1'\nport=55439\nunix_socket_directories='${local.replaceAll("'", "''")}'\n`,
    );
    writeFileSync(
      configPath,
      JSON.stringify(
        {
          port: 4310,
          databaseUrl: `postgresql://workspace:${password}@127.0.0.1:55439/workspace`,
          dataDir: path.join(local, "data"),
          ownerTokenHash: createHash("sha256").update(token).digest("hex"),
        },
        null,
        2,
      ),
      { mode: 0o600 },
    );
    writeFileSync(path.join(local, "owner-token"), token, { mode: 0o600 });
  }
  const existing = config();
  if (!existing.webhookTokenHash) {
    const hook = randomBytes(32).toString("base64url");
    writeFileSync(path.join(local, "work-hook-token"), hook, { mode: 0o600 });
    writeFileSync(
      configPath,
      JSON.stringify(
        {
          ...existing,
          webhookTokenHash: createHash("sha256").update(hook).digest("hex"),
        },
        null,
        2,
      ),
      { mode: 0o600 },
    );
  }
  pgStart();
  const cfg = config();
  const u = new URL(cfg.databaseUrl);
  const has = spawnSync(
    "/opt/homebrew/bin/psql",
    [
      "-h",
      local,
      "-p",
      "55439",
      "-U",
      "workspace",
      "-d",
      "postgres",
      "-tAc",
      "SELECT 1 FROM pg_database WHERE datname='workspace'",
    ],
    { env, encoding: "utf8" },
  );
  if (!has.stdout.trim())
    run("/opt/homebrew/bin/createdb", [
      "-h",
      local,
      "-p",
      "55439",
      "-U",
      "workspace",
      "workspace",
    ]);
  run("pnpm", ["db:migrate"]);
  mkdirSync(cfg.dataDir, { recursive: true, mode: 0o700 });
  console.log(
    "Setup complete. Owner sign-in token is in .local/owner-token (private local file).",
  );
} else if (action === "start" || action === "dev") {
  config();
  pgStart();
  run("pnpm", ["db:migrate"]);
  if (action === "start") {
    if (!existsSync(path.join(root, "dist/apps/server/src/server.js")))
      throw new Error("Run pnpm build first.");
    start("server", ["dist/apps/server/src/server.js"]);
    start("worker", ["dist/apps/server/src/worker.js"]);
  } else {
    start("server", [
      "--watch",
      "--import",
      "tsx",
      "apps/server/src/server.ts",
    ]);
    start("worker", [
      "--watch",
      "--import",
      "tsx",
      "apps/server/src/worker.ts",
    ]);
    start("vite", [
      path.join(root, "apps/web/node_modules/vite/bin/vite.js"),
      "--host",
      "127.0.0.1",
      "--config",
      path.join(root, "apps/web/vite.config.ts"),
      path.join(root, "apps/web"),
    ]);
  }
  console.log(
    `Workspace: http://127.0.0.1:${action === "dev" ? 4311 : 4310} — logs in .local/server.log and .local/worker.log`,
  );
} else if (action === "stop") await stop();
else if (action === "status") {
  console.log(`PostgreSQL: ${pgRunning() ? "running" : "stopped"}`);
  try {
    console.log(await (await fetch("http://127.0.0.1:4310/health")).text());
  } catch {
    console.log("Application: stopped");
  }
} else if (action === "backup") {
  await stop();
  const cfg = config();
  const dir = path.join(
    local,
    `backup-${new Date().toISOString().replaceAll(":", "-")}`,
  );
  mkdirSync(dir, { mode: 0o700 });
  run(
    "/opt/homebrew/bin/pg_dump",
    ["--format=custom", "--file", path.join(dir, "database.dump")],
    {
      env: databaseEnvironment(cfg.databaseUrl),
      stdio: ["ignore", "inherit", "inherit"],
    },
  );
  cpSync(cfg.dataDir, path.join(dir, "data"), { recursive: true });
  writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({
      version: 1,
      createdAt: new Date().toISOString(),
      dispatchOnRestore: false,
    }),
  );
  console.log(`Backup created: ${dir}`);
} else if (action === "restore") {
  const dir = process.argv[3];
  if (!dir || !existsSync(path.join(dir, "manifest.json")))
    throw new Error(
      "Supply a backup directory. Restore creates a NEW database and file directory; original preserved.",
    );
  await stop();
  const old = config(),
    suffix = Date.now(),
    name = `workspace_restore_${suffix}`,
    dataDir = path.join(local, `restored-data-${suffix}`);
  run("/opt/homebrew/bin/createdb", [
    "-h",
    local,
    "-p",
    "55439",
    "-U",
    "workspace",
    name,
  ]);
  const u = new URL(old.databaseUrl);
  u.pathname = `/${name}`;
  run(
    "/opt/homebrew/bin/pg_restore",
    ["--dbname", name, "--no-owner", path.join(dir, "database.dump")],
    { env: databaseEnvironment(u.toString()) },
  );
  cpSync(path.join(dir, "data"), dataDir, { recursive: true });
  run(
    "/opt/homebrew/bin/psql",
    [
      "-c",
      `INSERT INTO settings(key,value) VALUES('dispatchEnabled','false') ON CONFLICT(key) DO UPDATE SET value='false'; DELETE FROM owner_sessions; DELETE FROM connector_credentials; UPDATE records SET data=jsonb_set(data,'{status}','"needs_reconciliation"') WHERE kind IN ('run','request','workSession') AND data->>'status' IN ('prepared','running','dispatching','claimed','active'); UPDATE records SET data=data || '{\"status\":\"cancelled\",\"error\":\"Cancelled on restore; create a new request deliberately.\"}'::jsonb WHERE kind='request' AND data->>'status'='pending';`,
    ],
    { env: databaseEnvironment(u.toString()) },
  );
  writeFileSync(
    path.join(local, `config-before-restore-${suffix}.json`),
    JSON.stringify(old),
    { mode: 0o600 },
  );
  writeFileSync(
    configPath,
    JSON.stringify({ ...old, databaseUrl: u.toString(), dataDir }, null, 2),
    { mode: 0o600 },
  );
  console.log(
    "Restored into a new database and data directory. Dispatch DISABLED. Sign in again and reconcile before enabling.",
  );
} else
  throw new Error(
    "Usage: pnpm setup|dev|build|start|stop|status|backup|restore <backup-directory>",
  );
