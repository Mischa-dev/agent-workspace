import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { CodexConnector } from "../apps/server/src/connectors/codex.js";
import {
  normalizeCapacity,
  assessCapacity,
  CAPACITY_MAX_AGE_MS,
} from "../apps/server/src/domain/capacity.js";

const now = Date.parse("2026-09-21T12:00:00Z");
const observedAt = new Date(now).toISOString();
const window = (
  usedPercent: unknown,
  duration = 300,
  resetsAt: unknown = now / 1000 + 3600,
) => ({ usedPercent, windowDurationMins: duration, resetsAt });
const bucket = (primary: unknown = 20, secondary: unknown = 40) => ({
  primary: window(primary),
  secondary: window(secondary, 10080),
});
const snapshot = (value: unknown, at = observedAt) =>
  normalizeCapacity(value, at);

test("fresh applicable windows normalize consumed percentages and permit bounded admission", () => {
  const data = snapshot({ rateLimitsByLimitId: { codex: bucket() } });
  assert.equal(data.buckets[0].windows[0].remainingPercent, 80);
  assert.equal(data.buckets[0].windows[1].durationMinutes, 10080);
  assert.equal(data.buckets[0].windows[0].resetsAt, "2026-09-21T13:00:00.000Z");
  const result = assessCapacity(data, { now });
  assert.equal(result.status, "available");
  assert.equal(result.admission, "allow");
});

test("an exhausted applicable window holds work even if another window has room", () => {
  const result = assessCapacity(
    snapshot({ rateLimitsByLimitId: { codex: bucket(1, 100) } }),
    { now },
  );
  assert.equal(result.status, "exhausted");
  assert.equal(result.admission, "hold");
  assert.deepEqual(
    result.exhaustedWindows.map((w) => w.kind),
    ["secondary"],
  );
});

test("multi-bucket view is authoritative and unrelated exhausted limits do not block Codex", () => {
  const data = snapshot({
    rateLimits: bucket(100, 100),
    rateLimitsByLimitId: { codex: bucket(), other_model: bucket(100, 100) },
  });
  assert.equal(data.buckets.length, 2);
  assert.equal(assessCapacity(data, { now }).status, "available");
  assert.equal(
    assessCapacity(data, { now, applicableLimitIds: ["other_model"] }).status,
    "exhausted",
  );
  assert.equal(
    assessCapacity(data, { now, applicableLimitIds: ["codex", "missing"] })
      .status,
    "unknown",
  );
});

test("legacy fallback applies only when the multi-bucket view is unavailable", () => {
  assert.equal(
    assessCapacity(
      snapshot({ rateLimits: bucket(), rateLimitsByLimitId: null }),
      { now },
    ).status,
    "available",
  );
  assert.equal(
    assessCapacity(
      snapshot({ rateLimits: bucket(), rateLimitsByLimitId: {} }),
      { now },
    ).status,
    "unknown",
  );
  assert.equal(
    assessCapacity(
      snapshot({ rateLimits: bucket(), rateLimitsByLimitId: [] }),
      { now },
    ).status,
    "unknown",
  );
});

test("missing, null, string, nonfinite and out-of-range usage is unknown, never zero", () => {
  for (const invalid of [undefined, null, "0", NaN, Infinity, -1, 101, 20.5]) {
    const data = snapshot({
      rateLimits: { primary: window(invalid), secondary: window(5) },
    });
    assert.equal(data.buckets[0].windows[0].usedPercent, null);
    assert.equal(data.buckets[0].windows[0].remainingPercent, null);
    assert.equal(assessCapacity(data, { now }).status, "unknown");
  }
  const absent = snapshot({
    rateLimits: { primary: window(0), secondary: null },
  });
  assert.equal(absent.buckets[0].windows[1].validity, "unavailable");
  assert.equal(assessCapacity(absent, { now }).status, "unknown");
  assert.equal(assessCapacity(snapshot(null), { now }).status, "unknown");
  assert.equal(assessCapacity(undefined, { now }).admission, "bounded-only");
});

test("stale positive telemetry cannot promise availability and future timestamps are invalid", () => {
  const data = snapshot({ rateLimits: bucket() });
  assert.equal(
    assessCapacity(data, { now: now + CAPACITY_MAX_AGE_MS + 1 }).status,
    "unknown",
  );
  assert.equal(
    assessCapacity(data, { now: now + 301000, maxAgeMs: Infinity }).status,
    "unknown",
  );
  assert.equal(
    assessCapacity(
      snapshot({ rateLimits: bucket() }, new Date(now + 6000).toISOString()),
      { now },
    ).status,
    "unknown",
  );
  assert.equal(
    assessCapacity(snapshot({ rateLimits: bucket() }, "invalid-date"), { now })
      .status,
    "unknown",
  );
});

test("a predicted reset cannot replenish exhausted telemetry, including after staleness", () => {
  const data = snapshot({
    rateLimits: {
      primary: window(100, 300, now / 1000 + 1),
      secondary: window(30),
    },
  });
  const passedReset = assessCapacity(data, { now: now + 2000 });
  assert.equal(passedReset.status, "exhausted");
  assert.equal(passedReset.admission, "hold");
  const stale = assessCapacity(data, { now: now + CAPACITY_MAX_AGE_MS + 1 });
  assert.equal(stale.status, "unknown");
  assert.equal(stale.admission, "hold");
});

test("failed telemetry refresh preserves a prior exhausted hold; fresh actual recovery releases it", () => {
  const exhausted = snapshot({ rateLimits: bucket(100, 20) });
  const refreshTime = now + 120000;
  const missing = snapshot(null, new Date(refreshTime).toISOString());
  assert.equal(
    assessCapacity(missing, { now: refreshTime, lastKnown: exhausted })
      .admission,
    "hold",
  );
  const actualRecovery = snapshot(
    { rateLimits: bucket(0, 20) },
    new Date(refreshTime).toISOString(),
  );
  assert.equal(
    assessCapacity(actualRecovery, { now: refreshTime, lastKnown: exhausted })
      .admission,
    "allow",
  );
});

test("normalization drops billing values, credentials and reset credit identifiers", () => {
  const data = snapshot({
    rateLimits: { ...bucket(), credits: { balance: "private-balance" } },
    rateLimitResetCredits: { credits: [{ id: "private-reset-id" }] },
    token: "never-retain",
  });
  const encoded = JSON.stringify(data);
  for (const forbidden of [
    "private-balance",
    "private-reset-id",
    "never-retain",
    "credits",
    "token",
  ])
    assert.ok(!encoded.includes(forbidden));
});

test("capacity reads initialize and account telemetry only, with no model or billing methods", async () => {
  const dir = await mkdtemp(join(tmpdir(), "workspace-capacity-test-"));
  const binary = join(dir, "codex-fixture"),
    log = join(dir, "methods.jsonl");
  await writeFile(
    binary,
    `#!${process.execPath}
const fs = require('node:fs');
if(process.argv.includes('--version')) {console.log('codex-cli 0.148.0'); process.exit(0)}
require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
 const m=JSON.parse(line);fs.appendFileSync(${JSON.stringify(log)},JSON.stringify(m.method)+'\\n');
 if(m.method==='initialize') return console.log(JSON.stringify({id:m.id,result:{}}));
 if(m.method==='initialized') return;
 if(m.method==='account/rateLimits/read') return console.log(JSON.stringify({id:m.id,result:${JSON.stringify({ rateLimitsByLimitId: { codex: bucket() } })}}));
 process.exit(8);
});`,
    { mode: 0o700 },
  );
  try {
    const result = await new CodexConnector({
      binary,
      requestTimeoutMs: 1000,
    }).capacity();
    assert.equal(result.format, "multi-bucket");
    assert.deepEqual(
      (await readFile(log, "utf8"))
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line)),
      ["initialize", "initialized", "account/rateLimits/read"],
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("capacity transport timeout returns unknown and cleans up without retries", async () => {
  const dir = await mkdtemp(join(tmpdir(), "workspace-capacity-timeout-"));
  const binary = join(dir, "codex-fixture");
  await writeFile(
    binary,
    `#!${process.execPath}
if(process.argv.includes('--version')) {console.log('codex-cli 0.148.0'); process.exit(0)}
require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
 const m=JSON.parse(line);if(m.method==='initialize') console.log(JSON.stringify({id:m.id,result:{}}));
});`,
    { mode: 0o700 },
  );
  try {
    const started = Date.now();
    const result = await new CodexConnector({
      binary,
      requestTimeoutMs: 100,
    }).capacity();
    assert.equal(result.format, "unavailable");
    assert.equal(result.unavailableReason, "read-failed");
    assert.ok(Date.now() - started < 5000);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
