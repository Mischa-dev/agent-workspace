import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  CodexConnector,
  CodexRpc,
  codexArguments,
  restrictedCodexConfig,
} from "../apps/server/src/connectors/codex.js";

async function withRuntime(
  scenario: string,
  action: (binary: string) => Promise<void>,
) {
  const dir = await mkdtemp(join(tmpdir(), "workspace-protocol-test-"));
  const binary = join(dir, "fake-codex");
  const source = `#!${process.execPath}
const { createInterface } = require('node:readline');
if (process.argv.includes('--version')) { console.log('codex-cli ${scenario === "version" ? "99.0.0" : "0.148.0"}'); process.exit(0); }
const send = data => process.stdout.write(JSON.stringify(data)+'\\n');
const reply = (id,result) => send({id,result});
const scenario = ${JSON.stringify(scenario)};
let thread = 'fixture-thread';
const turn = 'fixture-turn';
let toolResponses = 0;
createInterface({input:process.stdin}).on('line',line=>{
  const m = JSON.parse(line);
  if (m.method === 'initialize') {
    if (scenario === 'never-initialize') return;
    if (scenario === 'overlap') return setTimeout(()=>reply(m.id,{}),70);
    return reply(m.id,{});
  }
  if (m.method === 'config/read') return reply(m.id,{config:{mcp_servers:{fixture_external:{command:'never-run-this'}}}});
  if (m.method === 'account/rateLimits/read') return reply(m.id,{rateLimits:{primary:{usedPercent:0,windowDurationMins:300,resetsAt:2000000000},credits:{balance:'PRIVATE_BALANCE'}},rateLimitResetCredits:{id:'PRIVATE_RESET_ID'}});
  if (m.method === 'model/list') return reply(m.id,{data:[{model:'fixture',isDefault:true}]});
  if (m.method === 'thread/start') {
    if (m.params.environments.length || m.params.runtimeWorkspaceRoots.length || m.params.config['mcp_servers.fixture_external.enabled'] !== false) throw new Error('Unsafe profile');
    return reply(m.id,{thread:{id:thread},model:'fixture',instructionSources:scenario==='instructions'?['private/AGENTS.md']:[]});
  }
  if (m.method === 'turn/start') {
    if (scenario === 'crash') return process.exit(2);
    send({method:'turn/started',params:{threadId:thread,turn:{id:turn}}});
    reply(m.id,{turn:{id:turn}});
    send({method:'account/rateLimits/updated',params:{rateLimits:{primary:{usedPercent:1},credits:{balance:'PRIVATE_BALANCE'}},rateLimitResetCredits:{id:'PRIVATE_RESET_ID'}}});
    if (scenario === 'overlap') return setTimeout(()=>send({method:'turn/completed',params:{threadId:thread,turn:{id:turn,status:'completed'}}}),700);
    if (scenario === 'cancel') return;
    const op = scenario === 'repo' ? 'repo.read' : 'resource.read';
    const boundThread = scenario === 'wrong-thread' ? 'other-thread' : thread;
    send({id:701,method:'item/tool/call',params:{threadId:boundThread,turnId:turn,callId:'call1',tool:'workspace',arguments:{operation:op,input:{id:'fixture-resource',path:'file.txt'}}}});
    if (scenario === 'duplicate') send({id:702,method:'item/tool/call',params:{threadId:boundThread,turnId:turn,callId:'call1',tool:'workspace',arguments:{operation:op,input:{id:'fixture-resource',path:'file.txt'}}}});
    return;
  }
  if (m.id === 701 || m.id === 702) {
    if (scenario === 'wrong-thread' || scenario === 'repo') {
      if (!m.error) throw new Error('Unsafe callback was accepted');
    } else if (!m.result || m.result.success !== true) throw new Error('Expected successful callback');
    if (scenario === 'duplicate' && ++toolResponses < 2) return;
    send({method:'item/completed',params:{threadId:thread,item:{type:'agentMessage',id:'answer',text:'Fixture completed.'}}});
    send({method:'turn/completed',params:{threadId:thread,turn:{id:turn,status:'completed'}}});
  }
  if (m.method === 'turn/interrupt') {
    reply(m.id,{});
    send({method:'turn/completed',params:{threadId:thread,turn:{id:turn,status:'interrupted'}}});
  }
});`;
  await writeFile(binary, source, { mode: 0o700 });
  try {
    await action(binary);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

test("restricted profile disables ambient tools and does not expose application environment", () => {
  const config = restrictedCodexConfig("/private/test-state");
  for (const feature of [
    "shell_tool",
    "memories",
    "plugins",
    "apps",
    "code_mode",
    "browser_use",
    "computer_use",
  ])
    assert.equal(config[`features.${feature}`], false);
  assert.equal(config["orchestrator.skills.enabled"], false);
  assert.equal(config["orchestrator.mcp.enabled"], false);
  assert.equal(config.project_doc_max_bytes, 0);
  assert.deepEqual(config.notify, []);
  assert.deepEqual(codexArguments({ web_search: "disabled", notify: [] }), [
    "app-server",
    "--listen",
    "stdio://",
    "-c",
    'web_search="disabled"',
    "-c",
    "notify=[]",
  ]);
});

test("fresh runtime records durable dispatch before turn and dispatches bound tools", async () =>
  withRuntime("normal", async (binary) => {
    const events: string[] = [];
    const tools: string[] = [];
    const result = await new CodexConnector({ binary }).run({
      runId: "run1",
      actorId: "actor1",
      mode: "chat",
      cwd: "/ignored",
      prompt: "Hello",
      instructions: "Test",
      timeoutMs: 2000,
      signal: new AbortController().signal,
      onEvent: async (type) => {
        events.push(type);
      },
      tool: async (name, _input, metadata) => {
        tools.push(name);
        assert.deepEqual(metadata, {
          callId: "call1",
          threadId: "fixture-thread",
          turnId: "fixture-turn",
        });
        return { text: "permitted" };
      },
    });
    assert.equal(result.status, "completed");
    assert.equal(result.text, "Fixture completed.");
    assert.equal(result.threadId, "fixture-thread");
    assert.equal(result.turnId, "fixture-turn");
    assert.deepEqual(tools, ["resource.read"]);
    assert.ok(
      events.indexOf("runtime.dispatch_intent") <
        events.indexOf("runtime.accepted"),
    );
    assert.ok(events.includes("runtime.terminal"));
  }));

test("duplicate provider call id invokes one application command", async () =>
  withRuntime("duplicate", async (binary) => {
    let calls = 0;
    const result = await new CodexConnector({ binary }).run({
      runId: "run1",
      actorId: "actor1",
      mode: "chat",
      cwd: "/ignored",
      prompt: "Hello",
      instructions: "Test",
      timeoutMs: 2000,
      signal: new AbortController().signal,
      onEvent: async () => {},
      tool: async () => {
        calls++;
        return {};
      },
    });
    assert.equal(result.status, "completed");
    assert.equal(calls, 1);
  }));

for (const scenario of ["wrong-thread", "repo"])
  test(`runtime rejects ${scenario} command before callback`, async () =>
    withRuntime(scenario, async (binary) => {
      let calls = 0;
      const result = await new CodexConnector({ binary }).run({
        runId: "run1",
        actorId: "actor1",
        mode: "chat",
        cwd: "/ignored",
        prompt: "Hello",
        instructions: "Test",
        timeoutMs: 2000,
        signal: new AbortController().signal,
        onEvent: async () => {},
        tool: async () => {
          calls++;
          return {};
        },
      });
      assert.equal(result.status, "completed");
      assert.equal(calls, 0);
    }));

test("operator stop waits for observed interrupted turn", async () =>
  withRuntime("cancel", async (binary) => {
    const abort = new AbortController();
    const events: string[] = [];
    const result = await new CodexConnector({ binary }).run({
      runId: "run1",
      actorId: "actor1",
      mode: "chat",
      cwd: "/ignored",
      prompt: "Hello",
      instructions: "Test",
      timeoutMs: 2000,
      signal: abort.signal,
      onEvent: async (type) => {
        events.push(type);
        if (type === "runtime.accepted") abort.abort();
      },
      tool: async () => ({}),
    });
    assert.equal(result.status, "cancelled");
    assert.ok(events.includes("runtime.interrupt_requested"));
    assert.ok(events.includes("runtime.terminal"));
  }));

test("transport death after dispatch is uncertain and is not retried", async () =>
  withRuntime("crash", async (binary) => {
    const events: string[] = [];
    const result = await new CodexConnector({ binary }).run({
      runId: "run1",
      actorId: "actor1",
      mode: "chat",
      cwd: "/ignored",
      prompt: "Hello",
      instructions: "Test",
      timeoutMs: 2000,
      signal: new AbortController().signal,
      onEvent: async (type) => {
        events.push(type);
      },
      tool: async () => ({}),
    });
    assert.equal(result.status, "uncertain");
    assert.equal(
      events.filter((type) => type === "runtime.dispatch_intent").length,
      1,
    );
  }));

for (const scenario of ["version", "instructions"])
  test(`unqualified ${scenario} fails before model dispatch`, async () =>
    withRuntime(scenario, async (binary) => {
      const events: string[] = [];
      const result = await new CodexConnector({ binary }).run({
        runId: "run1",
        actorId: "actor1",
        mode: "chat",
        cwd: "/ignored",
        prompt: "Hello",
        instructions: "Test",
        timeoutMs: 2000,
        signal: new AbortController().signal,
        onEvent: async (type) => {
          events.push(type);
        },
        tool: async () => ({}),
      });
      assert.equal(result.status, "failed");
      assert.ok(!events.includes("runtime.dispatch_intent"));
    }));

test("protocol request timeout terminates waiting without an unbounded retry", async () => {
  const rpc = new CodexRpc(process.execPath, ["-e", "process.stdin.resume()"], {
    cwd: tmpdir(),
    env: {},
    requestTimeoutMs: 40,
  });
  try {
    await assert.rejects(rpc.request("ignored"), /timed out/);
  } finally {
    await rpc.close();
  }
});

function fixtureInput(
  runId: string,
  onEvent: (type: string, data: Record<string, unknown>) => Promise<void>,
  signal = new AbortController().signal,
) {
  return {
    runId,
    actorId: runId,
    mode: "chat" as const,
    cwd: "/ignored",
    prompt: "Hello",
    instructions: "Test",
    timeoutMs: 2000,
    signal,
    onEvent,
    tool: async () => ({}),
  };
}

test("startup is serialized but accepted model turns remain concurrent", async () =>
  withRuntime("overlap", async (binary) => {
    const events: { runId: string; type: string; phase?: unknown }[] = [];
    const connector = new CodexConnector({ binary });
    const run = (runId: string) =>
      connector.run(
        fixtureInput(runId, async (type, data) => {
          events.push({ runId, type, phase: data.phase });
        }),
      );
    const results = await Promise.all([run("one"), run("two")]);
    assert.ok(results.every((result) => result.status === "completed"));
    const starts = events.filter(
      (event) =>
        event.type === "runtime.bootstrap" && event.phase === "initialize",
    );
    assert.equal(starts.length, 2);
    const first = starts[0].runId,
      second = starts[1].runId;
    const index = (runId: string, type: string, phase?: string) =>
      events.findIndex(
        (event) =>
          event.runId === runId &&
          event.type === type &&
          (!phase || event.phase === phase),
      );
    assert.ok(
      index(first, "runtime.bootstrap", "ready") <
        index(second, "runtime.bootstrap", "initialize"),
    );
    assert.ok(
      index(second, "runtime.accepted") < index(first, "runtime.terminal"),
    );
  }));

test("queued startup cancellation never enters initialize and active startup cancellation is prompt", async () =>
  withRuntime("never-initialize", async (binary) => {
    const firstAbort = new AbortController(),
      secondAbort = new AbortController();
    const connector = new CodexConnector({ binary, bootstrapTimeoutMs: 2000 });
    let signalStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      signalStarted = resolve;
    });
    const first = connector.run(
      fixtureInput(
        "first",
        async (type, data) => {
          if (type === "runtime.bootstrap" && data.phase === "initialize")
            signalStarted();
        },
        firstAbort.signal,
      ),
    );
    await started;
    const secondEvents: Record<string, unknown>[] = [];
    const second = await connector.run(
      fixtureInput(
        "second",
        async (type, data) => {
          if (type === "runtime.bootstrap") {
            secondEvents.push(data);
            if (data.phase === "queue" && data.status === "waiting")
              setTimeout(() => secondAbort.abort(), 20);
          }
        },
        secondAbort.signal,
      ),
    );
    assert.equal(second.status, "cancelled");
    assert.ok(!secondEvents.some((event) => event.phase === "initialize"));
    assert.ok(
      secondEvents.some(
        (event) =>
          event.phase === "queue" && event.errorCategory === "cancelled",
      ),
    );
    const cancelledAt = Date.now();
    firstAbort.abort();
    assert.equal((await first).status, "cancelled");
    assert.ok(Date.now() - cancelledAt < 500);
  }));

test("bootstrap deadline fails once with sanitized phase diagnostics", async () =>
  withRuntime("never-initialize", async (binary) => {
    const events: Record<string, unknown>[] = [];
    const result = await new CodexConnector({
      binary,
      bootstrapTimeoutMs: 100,
    }).run(
      fixtureInput("timeout", async (type, data) => {
        if (type === "runtime.bootstrap") events.push(data);
      }),
    );
    assert.equal(result.status, "failed");
    assert.equal(
      events.filter(
        (event) => event.phase === "initialize" && event.status === "started",
      ).length,
      1,
    );
    assert.ok(
      events.some(
        (event) =>
          event.phase === "initialize" &&
          event.status === "failed" &&
          event.errorCategory === "timeout" &&
          typeof event.elapsedMs === "number",
      ),
    );
    for (const event of events)
      assert.ok(
        Object.keys(event).every((key) =>
          ["phase", "status", "elapsedMs", "errorCategory"].includes(key),
        ),
      );
  }));

test("quota read and update events contain only normalized capacity", async () =>
  withRuntime("normal", async (binary) => {
    const quotas: Record<string, unknown>[] = [];
    const result = await new CodexConnector({ binary }).run(
      fixtureInput("quota", async (type, data) => {
        if (type === "runtime.quota") quotas.push(data);
      }),
    );
    assert.equal(result.status, "completed");
    assert.ok(quotas.length >= 2);
    for (const quota of quotas)
      assert.deepEqual(Object.keys(quota), ["capacity"]);
    const encoded = JSON.stringify(quotas);
    for (const forbidden of [
      "PRIVATE_BALANCE",
      "PRIVATE_RESET_ID",
      "credits",
      "rateLimitResetCredits",
    ])
      assert.ok(!encoded.includes(forbidden));
  }));
