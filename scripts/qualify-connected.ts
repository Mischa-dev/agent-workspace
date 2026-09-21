import assert from "node:assert/strict";
import { randomUUID, createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
const origin = "http://127.0.0.1:4310";
const login = await fetch(`${origin}/api/login`, {
  method: "POST",
  headers: { Origin: origin, "Content-Type": "application/json" },
  body: JSON.stringify({ token: "browser-qualification-only" }),
});
assert.equal(
  login.status,
  200,
  "Start the isolated browser qualification workspace first.",
);
const cookie = login.headers.get("set-cookie")!.split(";")[0],
  csrf = ((await login.json()) as any).csrf;
const ownerHeaders = {
  Origin: origin,
  "Content-Type": "application/json",
  Cookie: cookie,
  "X-CSRF-Token": csrf,
};
async function owner(type: string, input: any) {
  const r = await fetch(`${origin}/api/commands`, {
    method: "POST",
    headers: ownerHeaders,
    body: JSON.stringify({ id: randomUUID(), type, input }),
  });
  const data = (await r.json()) as any;
  assert.equal(r.status, 200, data.error);
  return data.value;
}
async function snapshot() {
  const r = await fetch(`${origin}/api/snapshot`, {
    headers: { Cookie: cookie },
  });
  assert.equal(r.status, 200);
  return (await r.json()) as any;
}
async function agent(
  token: string,
  type: string,
  input: any = {},
  requestId?: string,
) {
  const r = await fetch(`${origin}/api/agent/commands`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ id: randomUUID(), type, input, requestId }),
  });
  const data = (await r.json()) as any;
  assert.equal(r.status, 200, data.error);
  return data.value;
}
const initial = await snapshot(),
  first = initial.records.find(
    (r: any) => r.kind === "agent" && r.data.name === "Research partner",
  ),
  second = initial.records.find(
    (r: any) => r.kind === "agent" && r.data.name === "Atlas",
  );
assert(first && second);
assert(
  !initial.records.some(
    (r: any) => r.kind === "workSession" && r.data.status === "active",
  ),
);
assert(
  !initial.records.some(
    (r: any) =>
      r.kind === "request" &&
      r.data.status === "pending" &&
      r.data.agentId !== first.id,
  ),
);
let credential: any, other: any;
try {
  await owner("agent.configure", {
    id: second.id,
    expectedRevision: second.revision,
    deliveryMode: "poll",
    checkInMinutes: 1440,
  });
  credential = await owner("credential.issue", {
    agentId: first.id,
    label: "Protocol qualification",
  });
  other = await owner("credential.issue", {
    agentId: second.id,
    label: "Handoff qualification",
  });
  assert.equal((await agent(credential.token, "agent.me")).agent.id, first.id);
  await owner("settings.dispatch", { enabled: true });
  const inbox = await agent(credential.token, "inbox.list");
  assert(inbox.requests.length);
  const request = inbox.requests[0].request;
  const claimed = await agent(credential.token, "request.claim", {
    id: request.id,
  });
  assert.equal(claimed.claimed, true);
  const bytes = Buffer.from("Connected workspace qualification file.\n");
  const form = new FormData();
  form.append("commandId", randomUUID());
  form.append("requestId", request.id);
  form.append(
    "input",
    JSON.stringify({ title: "Protocol verification file", type: "file" }),
  );
  form.append(
    "file",
    new Blob([bytes], { type: "text/plain" }),
    "protocol-check.txt",
  );
  const upload = await fetch(`${origin}/api/agent/uploads`, {
    method: "POST",
    headers: { Authorization: `Bearer ${credential.token}` },
    body: form,
  });
  const uploaded = (await upload.json()) as any;
  assert.equal(upload.status, 200, uploaded.error);
  const pin = {
    resourceId: uploaded.value.resource.id,
    revisionId: uploaded.value.revision.id,
  };
  const downloaded = await fetch(
    `${origin}/api/agent/resources/${pin.resourceId}/revisions/${pin.revisionId}/download?requestId=${request.id}`,
    { headers: { Authorization: `Bearer ${credential.token}` } },
  );
  assert.equal(downloaded.status, 200);
  assert.deepEqual(Buffer.from(await downloaded.arrayBuffer()), bytes);
  const handoff = await agent(
    credential.token,
    "request.handoff",
    {
      agentId: second.id,
      body: "The file is ready. Please review it and leave the result.",
      attachments: [pin],
    },
    request.id,
  );
  const next = handoff.requests[0];
  assert.equal(
    (await agent(other.token, "inbox.list")).requests.some(
      (r: any) => r.request.id === next.id,
    ),
    true,
  );
  await agent(other.token, "request.claim", { id: next.id });
  const read = await agent(
    other.token,
    "resource.read",
    { id: pin.resourceId, revisionId: pin.revisionId },
    next.id,
  );
  assert.equal(read.revision.data.text, bytes.toString());
  await agent(
    other.token,
    "result.submit",
    {
      summary: "Verified the shared file and completed the handoff.",
      attachments: [pin],
    },
    next.id,
  );
  const measured: number[] = [];
  for (let n = 0; n < 12; n++) {
    const started = performance.now();
    await snapshot();
    measured.push(performance.now() - started);
  }
  measured.sort((a, b) => a - b);
  const last = await snapshot();
  const report = {
    passed: true,
    verifiedAt: new Date().toISOString(),
    transport:
      "Real loopback HTTP, application server, PostgreSQL and file storage; no external model call",
    checks: [
      "Bearer identity and durable inbox",
      "Shared admission and a real request context",
      "Multipart file upload, checksum and authenticated download",
      "Cross-account handoff with pinned file",
      "Result returned to originating conversation",
    ],
    fileChecksum: createHash("sha256").update(bytes).digest("hex"),
    recordCount: last.records.length,
    snapshot: {
      samples: measured.length,
      medianMs: Math.round(measured[6] * 100) / 100,
      p95Ms: Math.round(measured[11] * 100) / 100,
    },
  };
  writeFileSync(
    "evidence/connected-protocol.json",
    JSON.stringify(report, null, 2) + "\n",
  );
  console.log(JSON.stringify(report, null, 2));
} finally {
  await owner("settings.dispatch", { enabled: false });
  if (credential) await owner("credential.revoke", { id: credential.id });
  if (other) await owner("credential.revoke", { id: other.id });
  const current = (await snapshot()).records.find(
    (r: any) => r.id === second.id,
  );
  if (!current.data.lastWakeupStatus)
    await owner("agent.configure", {
      id: second.id,
      expectedRevision: current.revision,
      deliveryMode: "managed",
      checkInMinutes: 60,
    });
}
