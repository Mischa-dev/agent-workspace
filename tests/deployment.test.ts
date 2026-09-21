import test from "node:test";
import assert from "node:assert/strict";
import {
  mkdtemp,
  writeFile,
  readFile,
  readdir,
  stat,
  rm,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync, execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { promisify } from "node:util";

test("container health checks preserve the public Host and wait for the worker when requested", async () => {
  let workerOnline = false;
  const server = createServer((request, response) => {
    response.statusCode =
      request.headers.host === "workspace.example" ? 200 : 403;
    response.end(JSON.stringify({ ok: true, workerOnline }));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as { port: number };
  const probe = (worker = false) =>
    promisify(execFile)(
      process.execPath,
      [
        "--import",
        "tsx",
        "apps/server/src/healthcheck.ts",
        ...(worker ? ["--worker"] : []),
      ],
      {
        env: {
          ...process.env,
          PORT: String(address.port),
          PUBLIC_ORIGIN: "https://workspace.example",
        },
        timeout: 5000,
      },
    );
  try {
    await probe();
    await assert.rejects(probe(true));
    workerOnline = true;
    await probe(true);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

for (const fail of [false, true])
  test(`portable backup ${fail ? "preserves partial files and restarts on failure" : "writes private verified archives and a checksum manifest"}`, async () => {
    const directory = await mkdtemp(
      path.join(tmpdir(), "workspace-deploy-test-"),
    );
    const log = path.join(directory, "podman.log"),
      binary = path.join(directory, "podman");
    await writeFile(
      path.join(directory, "runtime.env"),
      "FIXTURE_ONLY=true\n",
      { mode: 0o600 },
    );
    await writeFile(
      path.join(directory, "owner-token.txt"),
      "fixture-not-a-real-token\n",
      { mode: 0o600 },
    );
    await writeFile(
      binary,
      `#!${process.execPath}
const fs=require('node:fs');const args=process.argv.slice(2);fs.appendFileSync(${JSON.stringify(log)},JSON.stringify(args)+'\\n');
if(args[0]==='inspect'){console.log('true');process.exit(0);}
if(args.includes('pg_dump')){if(${fail})process.exit(7);process.stdout.write('database-fixture');process.exit(0);}
if(args.includes('-czf'))process.stdout.write('files-fixture');
if(args.includes('-i'))process.stdin.resume();
`,
      { mode: 0o700 },
    );
    try {
      const result = spawnSync(
        process.execPath,
        ["scripts/deploy-podman.mjs", "backup"],
        {
          cwd: process.cwd(),
          env: {
            ...process.env,
            PATH: `${directory}:${process.env.PATH}`,
            WORKSPACE_DEPLOY_DIR: directory,
          },
          encoding: "utf8",
        },
      );
      assert.equal(result.status, fail ? 1 : 0, result.stderr);
      const commands = (await readFile(log, "utf8"))
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      assert.ok(
        commands.some(
          (args) =>
            args[0] === "stop" &&
            args.includes("workspace-server") &&
            args.includes("workspace-worker") &&
            !args.includes("workspace-postgres"),
        ),
      );
      assert.deepEqual(
        commands.findLast((args) => args[0] === "start"),
        ["start", "workspace-server", "workspace-worker"],
      );
      assert.ok(
        !commands.some((args) => args[0] === "rm" || args[0] === "rmi"),
      );
      const backup = path.join(
        directory,
        "backups",
        (await readdir(path.join(directory, "backups")))[0],
      );
      assert.equal((await stat(backup)).mode & 0o777, 0o700);
      if (fail) {
        assert.ok((await readdir(backup)).includes("database.dump"));
        assert.ok(!(await readdir(backup)).includes("manifest.json"));
      } else {
        const manifest = JSON.parse(
          await readFile(path.join(backup, "manifest.json"), "utf8"),
        );
        assert.equal(manifest.consistent, true);
        assert.equal(manifest.files.length, 4);
        for (const file of manifest.files) {
          const name = path.join(backup, file.filename);
          assert.equal((await stat(name)).mode & 0o777, 0o600);
          assert.equal(
            createHash("sha256")
              .update(await readFile(name))
              .digest("hex"),
            file.sha256,
          );
        }
      }
      assert.ok(!result.stdout.includes("fixture-not-a-real-token"));
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
