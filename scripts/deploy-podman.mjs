#!/usr/bin/env node
// Explicit operator actions only. Merely importing/building the app does not deploy it.
import {
  mkdirSync,
  writeFileSync,
  existsSync,
  chmodSync,
  copyFileSync,
  openSync,
  closeSync,
  statSync,
  createReadStream,
} from "node:fs";
import { randomBytes, createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import path from "node:path";
const root = path.resolve(import.meta.dirname, ".."),
  directory = path.resolve(
    process.env.WORKSPACE_DEPLOY_DIR || path.join(root, ".local/deployment"),
  );
const action = process.argv[2] || "help";
const image =
  process.env.WORKSPACE_IMAGE || "localhost/agent-workspace:connected";
const prefix = process.env.WORKSPACE_CONTAINER_PREFIX || "workspace";
const hostPort = Number(process.env.WORKSPACE_HOST_PORT || 4310);
if (!Number.isInteger(hostPort) || hostPort < 1024 || hostPort > 65535)
  throw new Error("WORKSPACE_HOST_PORT must be an unprivileged TCP port.");
const limits = (role) => [
  "--restart",
  "unless-stopped",
  "--memory",
  process.env[`WORKSPACE_${role}_MEMORY`] || "384m",
  "--cpus",
  process.env[`WORKSPACE_${role}_CPUS`] || "1",
  "--pids-limit",
  "256",
];
if (!/^[a-z][a-z0-9-]{0,40}$/.test(prefix))
  throw new Error("Use a simple lowercase container prefix.");
const envFile = path.join(directory, "runtime.env");
const run = (args, options = {}) => {
  const result = spawnSync("podman", args, {
    cwd: root,
    stdio: "inherit",
    ...options,
  });
  if (result.status !== 0)
    throw new Error(`Podman action failed (${args[0]}).`);
  return result;
};
const exists = (kind, name) =>
  spawnSync("podman", [kind, "exists", name], { stdio: "ignore" }).status === 0;
const names = {
  db: `${prefix}-postgres`,
  server: `${prefix}-server`,
  worker: `${prefix}-worker`,
  network: `${prefix}-network`,
  files: `${prefix}-files`,
  postgres: `${prefix}-postgres-data`,
};
const databaseReady = async () => {
  for (let i = 0; i < 30; i++) {
    if (
      spawnSync(
        "podman",
        [
          "exec",
          names.db,
          "pg_isready",
          "-h",
          "127.0.0.1",
          "-U",
          "workspace",
          "-d",
          "workspace",
        ],
        { stdio: "ignore" },
      ).status === 0
    )
      return;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(
    "PostgreSQL did not become ready; inspect its container log.",
  );
};
const applicationReady = async (expectWorker = true) => {
  for (let attempt = 0; attempt < 30; attempt++) {
    const result = spawnSync(
      "podman",
      [
        "exec",
        names.server,
        "node",
        "dist/apps/server/src/healthcheck.js",
        ...(expectWorker ? ["--worker"] : []),
      ],
      { stdio: "ignore", timeout: 5000 },
    );
    if (result.status === 0) {
      run(["healthcheck", "run", names.server], { stdio: "ignore" });
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(
    "Application did not become healthy; inspect the server and worker logs.",
  );
};
if (action === "prepare") {
  if (existsSync(envFile))
    throw new Error(
      "Deployment config already exists; preserve it and edit deliberately.",
    );
  const origin = process.env.PUBLIC_ORIGIN || "http://127.0.0.1:4310",
    url = new URL(origin);
  if (
    url.origin !== origin ||
    url.username ||
    url.password ||
    (!["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) &&
      url.protocol !== "https:")
  )
    throw new Error("Set PUBLIC_ORIGIN to an HTTPS origin, or loopback HTTP.");
  const databasePassword = randomBytes(32).toString("base64url"),
    ownerToken = randomBytes(32).toString("base64url"),
    workHookToken = randomBytes(32).toString("base64url");
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  chmodSync(directory, 0o700);
  writeFileSync(
    envFile,
    [
      `POSTGRES_USER=workspace`,
      `POSTGRES_DB=workspace`,
      `POSTGRES_PASSWORD=${databasePassword}`,
      `DATABASE_URL=postgresql://workspace:${databasePassword}@${names.db}:5432/workspace`,
      `OWNER_TOKEN_HASH=${createHash("sha256").update(ownerToken).digest("hex")}`,
      `WORK_HOOK_TOKEN_HASH=${createHash("sha256").update(workHookToken).digest("hex")}`,
      `PUBLIC_ORIGIN=${origin}`,
      `BIND_HOST=0.0.0.0`,
      `DATA_DIR=/var/lib/workspace`,
      "",
    ].join("\n"),
    { mode: 0o600, flag: "wx" },
  );
  writeFileSync(path.join(directory, "owner-token.txt"), ownerToken + "\n", {
    mode: 0o600,
    flag: "wx",
  });
  writeFileSync(
    path.join(directory, "work-hook-token.txt"),
    workHookToken + "\n",
    {
      mode: 0o600,
      flag: "wx",
    },
  );
  console.log(
    `Prepared private deployment files in ${directory}. No service started; no credential printed.`,
  );
} else if (action === "build")
  run([
    "build",
    "--format=docker",
    "--file",
    "Containerfile",
    "--tag",
    image,
    ".",
  ]);
else if (action === "start") {
  if (!existsSync(envFile)) throw new Error("Run prepare first.");
  if (exists("container", names.server) || exists("container", names.worker))
    throw new Error(
      "Application containers already exist. Inspect them before changing or replacing this deployment.",
    );
  if (!exists("network", names.network))
    run(["network", "create", names.network]);
  for (const volume of [names.files, names.postgres])
    if (!exists("volume", volume)) run(["volume", "create", volume]);
  if (!exists("container", names.db))
    run([
      "run",
      "-d",
      "--name",
      names.db,
      ...limits("DATABASE"),
      "--network",
      names.network,
      "--env-file",
      envFile,
      "--volume",
      `${names.postgres}:/var/lib/postgresql`,
      "docker.io/library/postgres:18.6",
      "postgres",
      "-c",
      "shared_buffers=64MB",
      "-c",
      "max_connections=50",
    ]);
  await databaseReady();
  const shared = [
    "--network",
    names.network,
    "--env-file",
    envFile,
    "--volume",
    `${names.files}:/var/lib/workspace:U`,
  ];
  run([
    "run",
    "--rm",
    "--no-healthcheck",
    ...shared,
    image,
    "node",
    "--import",
    "tsx",
    "apps/server/src/migrate.ts",
  ]);
  run([
    "run",
    "-d",
    "--name",
    names.server,
    "--health-interval",
    process.env.WORKSPACE_HEALTH_INTERVAL || "disable",
    ...limits("SERVER"),
    ...shared,
    "--env",
    "NODE_OPTIONS=--max-old-space-size=256",
    "--publish",
    `127.0.0.1:${hostPort}:4310`,
    image,
  ]);
  run([
    "run",
    "-d",
    "--name",
    names.worker,
    ...limits("WORKER"),
    "--no-healthcheck",
    ...shared,
    "--env",
    "NODE_OPTIONS=--max-old-space-size=256",
    image,
    "node",
    "dist/apps/server/src/worker.js",
  ]);
  await applicationReady();
  console.log(
    "Containers started on host loopback only. Configure a trusted HTTPS proxy separately after host approval.",
  );
} else if (action === "resume") {
  for (const name of [names.db, names.server, names.worker])
    if (!exists("container", name))
      throw new Error(
        "Run the initial start action before installing the service.",
      );
  run(["start", names.db]);
  await databaseReady();
  run(["start", names.server, names.worker]);
  await applicationReady();
} else if (action === "backup") {
  if (!existsSync(envFile))
    throw new Error("Deployment configuration is missing.");
  const timestamp = new Date().toISOString().replaceAll(":", "-");
  const target = path.join(
    directory,
    "backups",
    `${timestamp}-${randomBytes(4).toString("hex")}`,
  );
  mkdirSync(target, { recursive: true, mode: 0o700 });
  chmodSync(path.dirname(target), 0o700);
  const running = [names.server, names.worker].filter((name) => {
    const result = spawnSync(
      "podman",
      ["inspect", "--format", "{{.State.Running}}", name],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    );
    return result.status === 0 && result.stdout.trim() === "true";
  });
  const capture = (filename, args) => {
    const fd = openSync(path.join(target, filename), "wx", 0o600);
    try {
      run(args, { stdio: ["ignore", fd, "inherit"] });
    } finally {
      closeSync(fd);
    }
    if (statSync(path.join(target, filename)).size === 0)
      throw new Error(`Empty backup artifact: ${filename}`);
  };
  const verify = (filename, args) => {
    const fd = openSync(path.join(target, filename), "r");
    try {
      run(args, { stdio: [fd, "ignore", "inherit"] });
    } finally {
      closeSync(fd);
    }
  };
  try {
    if (running.length) run(["stop", "--time", "30", ...running]);
    capture("database.dump", [
      "exec",
      names.db,
      "pg_dump",
      "-U",
      "workspace",
      "-d",
      "workspace",
      "-Fc",
    ]);
    capture("files.tar.gz", [
      "run",
      "--rm",
      "--no-healthcheck",
      "--volume",
      `${names.files}:/var/lib/workspace:ro`,
      image,
      "tar",
      "-C",
      "/var/lib/workspace",
      "-czf",
      "-",
      ".",
    ]);
    verify("database.dump", ["exec", "-i", names.db, "pg_restore", "--list"]);
    verify("files.tar.gz", [
      "run",
      "--rm",
      "--no-healthcheck",
      "-i",
      image,
      "tar",
      "-tzf",
      "-",
    ]);
    const files = ["database.dump", "files.tar.gz"];
    for (const filename of [
      "runtime.env",
      "owner-token.txt",
      "work-hook-token.txt",
    ]) {
      const source = path.join(directory, filename);
      if (existsSync(source)) {
        copyFileSync(source, path.join(target, filename));
        chmodSync(path.join(target, filename), 0o600);
        files.push(filename);
      }
    }
    const digests = await Promise.all(
      files.map(async (filename) => {
        const checksum = createHash("sha256");
        for await (const chunk of createReadStream(path.join(target, filename)))
          checksum.update(chunk);
        return {
          filename,
          bytes: statSync(path.join(target, filename)).size,
          sha256: checksum.digest("hex"),
        };
      }),
    );
    const manifest = {
      createdAt: new Date().toISOString(),
      prefix,
      image,
      consistent: true,
      files: digests,
    };
    writeFileSync(
      path.join(target, "manifest.json"),
      JSON.stringify(manifest, null, 2) + "\n",
      { mode: 0o600, flag: "wx" },
    );
    console.log(
      `Private backup verified at ${target}. No old backups were deleted.`,
    );
  } finally {
    if (running.length) run(["start", ...running]);
    if (running.includes(names.server))
      await applicationReady(running.includes(names.worker));
  }
} else if (action === "stop")
  run(["stop", "--time", "30", names.worker, names.server]);
else if (action === "status") run(["ps", "--filter", `name=${prefix}-`]);
else
  console.log(
    "Usage: node scripts/deploy-podman.mjs prepare|build|start|resume|stop|status|backup\nDefaults publish only 127.0.0.1:4310. Deployment files and credentials stay in .local/deployment.",
  );
