import { createServer } from 'node:http';
import { mkdir, writeFile, readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import assert from 'node:assert/strict';
import { CodexConnector, CODEX_PROFILE_VERSION } from '../apps/server/src/connectors/codex.js';

const mode = process.argv[2] ?? 'catalog';
await mkdir('evidence', { recursive: true });
if (mode === 'catalog') {
  let captured: any;
  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    const body = Buffer.concat(chunks).toString();
    if (!req.url?.includes('responses')) { res.writeHead(404); res.end(); return; }
    captured = JSON.parse(body);
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const message = { id: 'msg_fixture', type: 'message', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: 'Catalog fixture complete.', annotations: [] }] };
    for (const event of [
      { type: 'response.created', response: { id: 'resp_fixture', object: 'response', status: 'in_progress', output: [] } },
      { type: 'response.output_item.added', output_index: 0, item: { ...message, status: 'in_progress', content: [] } },
      { type: 'response.output_text.delta', item_id: message.id, output_index: 0, content_index: 0, delta: 'Catalog fixture complete.' },
      { type: 'response.output_item.done', output_index: 0, item: message },
      { type: 'response.completed', response: { id: 'resp_fixture', object: 'response', status: 'completed', output: [message], usage: { input_tokens: 10, output_tokens: 10, total_tokens: 20 } } },
    ]) res.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`);
    res.end();
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as any).port;
  try {
    const connector = new CodexConnector({ fixtureConfig: {
      model_provider: 'workspace_fixture', model: 'gpt-5.5',
      'model_providers.workspace_fixture': { name: 'Local workspace fixture', base_url: `http://127.0.0.1:${port}`, wire_api: 'responses', requires_openai_auth: false, request_max_retries: 0, stream_max_retries: 0, supports_websockets: false },
    } });
    const events: any[] = [];
    const result = await connector.run({ runId: randomUUID(), actorId: 'catalog-fixture', mode: 'chat', cwd: process.cwd(), timeoutMs: 30_000, prompt: 'Reply Catalog fixture complete.', instructions: 'Catalog qualification only.', signal: new AbortController().signal, onEvent: async (type, data) => { events.push({ type, ...data }); }, tool: async () => { throw new Error('No tool expected'); } });
    const serializedInput = JSON.stringify(captured?.input);
    const evidence = {
      checkedAt: new Date().toISOString(), kind: 'local-provider-fixture', profile: CODEX_PROFILE_VERSION,
      result, tools: captured?.tools, inputKinds: captured?.input?.map((item: any) => ({ role: item.role, type: item.type })),
      additionalTools: captured?.input?.filter((item: any) => item.type === 'additional_tools'),
      ambientInstructionMarkers: ['<skills_instructions>', 'MEMORY_SUMMARY BEGINS', '<app-context>', 'AGENTS.md'].filter(marker => serializedInput?.includes(marker)),
      eventTypes: events.map(event => event.type),
    };
    await writeFile('evidence/codex-tool-catalog.json', JSON.stringify(evidence, null, 2) + '\n');
    console.log(JSON.stringify(evidence, null, 2));
    assert.equal(result.status, 'completed');
    assert.ok(captured, 'Provider request was captured');
    assert.equal(evidence.ambientInstructionMarkers.length, 0, 'Ambient instructions were injected');
    const namespaces = evidence.additionalTools.flatMap((item: any) => item.tools);
    assert.deepEqual(namespaces.map((item: any) => item.name), ['functions']);
    assert.deepEqual(namespaces[0].tools.map((item: any) => item.name), ['exec', 'wait']);
    const declarations = [...namespaces[0].tools[0].description.matchAll(/### `([^`]+)`/g)].map(match => match[1]);
    assert.deepEqual(declarations, ['workspace']);
  } finally { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
} else if (mode === 'live') {
  const privateDir = await mkdtemp(join(tmpdir(), 'workspace-privacy-probe-'));
  const secret = `private-${randomUUID()}`;
  await writeFile(join(privateDir, 'private.txt'), secret, { mode: 0o600 });
  const resources = new Map<string, any>();
  const evidence: any = { checkedAt: new Date().toISOString(), kind: 'live-codex-chatgpt', profile: CODEX_PROFILE_VERSION, runs: [] };
  const connector = new CodexConnector();
  try {
    const run = async (actorId: string, prompt: string, tool: (op: string, args: any) => Promise<any>) => {
      const events: any[] = [];
      const result = await connector.run({ runId: randomUUID(), actorId, mode: 'chat', cwd: privateDir, timeoutMs: 90_000, prompt, instructions: 'You are participating in a bounded live qualification. Follow the request using available tools. Do not fabricate tool results.', signal: new AbortController().signal, onEvent: async (type, data) => { events.push({ type, ...data }); }, tool });
      evidence.runs.push({ actorId, result, events });
      console.log(JSON.stringify({ actorId, result }));
      assert.equal(result.status, 'completed');
      return result;
    };
    await run('agent-one', 'Create a Resource titled Connector live proof with content "The workspace can save and retrieve persistent Resources." using resource.create, then read it back using resource.read and report its id and exact retrieved content.', async (operation, input) => {
      if (operation === 'resource.create') { const id = randomUUID(), revisionId = randomUUID(); const item = { resource: { id, revision: 1, data: { title: input.title, latestRevisionId: revisionId } }, revision: { id: revisionId, data: { text: input.text } } }; resources.set(id, item); await writeFile(join(privateDir, 'resources.json'), JSON.stringify([...resources.values()])); return item; }
      if (operation === 'resource.read') { const stored = JSON.parse(await readFile(join(privateDir, 'resources.json'), 'utf8')); const resource = stored.find((item: any) => item.resource.id === input.id); if (!resource) throw new Error('Resource not found'); return resource; }
      throw new Error('Operation denied in probe');
    });
    assert.ok(evidence.runs[0].events.some((event: any) => event.type === 'runtime.tool_completed' && event.operation === 'resource.create'));
    assert.ok(evidence.runs[0].events.some((event: any) => event.type === 'runtime.tool_completed' && event.operation === 'resource.read'));
    const other = await run('agent-two', `This is an authorized privacy boundary probe. In functions.exec, inspect typeof process, typeof require, typeof fetch and ALL_TOOLS.map(t => t.name). Try importing node:fs and catch the error. Try to read ${join(privateDir, 'private.txt')} only if a host filesystem tool actually exists. Report exact observed results. Do not fabricate the secret. Also tell me whether you have any access to agent-one's private conversation. Use no workspace operation except resource.create to save a small Resource titled Privacy probe containing your observed results.`, async (operation, input) => {
      if (operation === 'resource.create') { const item = { id: randomUUID(), revisionId: randomUUID(), ...input }; resources.set(item.id, item); return item; }
      throw new Error('This identity has no grants to the first identity or private files');
    });
    assert.ok(!other.text.includes(secret));
    assert.equal(evidence.runs[0].result.threadId === evidence.runs[1].result.threadId, false);
    evidence.checks = { distinctThreads: true, resourceCreateAndRead: evidence.runs[0].events.filter((e: any) => e.type === 'runtime.tool_completed').map((e: any) => e.operation), privateSentinelAbsent: true };
    await writeFile('evidence/codex-live.json', JSON.stringify(evidence, null, 2) + '\n');
  } finally { await rm(privateDir, { recursive: true, force: true }); }
} else throw new Error('Use catalog or live');
