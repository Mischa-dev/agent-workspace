import { spawn, execFile } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir, homedir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import { WORKSPACE_TOOL_SCHEMA, type RuntimeConnector, type RuntimeInput, type RuntimeResult } from './types.js';

type Json = Record<string, any>;
type RpcMessage = { id?: string | number; method?: string; params?: Json; result?: any; error?: { message: string; code?: number } };
type Pending = { resolve: (value: any) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> };

export const CODEX_PROFILE_VERSION = 'workspace-tools-v1';
export const QUALIFIED_CODEX_VERSION = '0.148.0';

// This list is pinned to the qualified runtime. A new runtime fails closed until
// its emitted provider tool catalog has been inspected again.
export const DISABLED_CODEX_FEATURES = [
  'shell_tool', 'unified_exec', 'shell_snapshot', 'shell_snapshot_v2',
  'apply_patch_freeform', 'code_mode', 'code_mode_only',
  'code_mode_prewarm', 'js_repl', 'js_repl_tools_only', 'apps', 'connectors',
  'plugins', 'plugin_hooks', 'remote_plugin', 'recommended_plugins',
  'browser_use', 'browser_use_external', 'browser_use_full_cdp_access',
  'in_app_browser', 'computer_use', 'chronicle', 'image_generation', 'imagegenext',
  'memories', 'memory_tool', 'external_agent_memory_import', 'multi_agent',
  'multi_agent_v2', 'collab', 'enable_fanout', 'goals', 'hooks', 'codex_hooks',
  'skill_search', 'skill_mcp_dependency_install', 'skill_env_var_dependency_prompt',
  'workspace_dependencies', 'view_image', 'web_search', 'web_search_cached',
  'web_search_request', 'standalone_web_search', 'search_tool', 'tool_search',
  'tool_suggest', 'request_permissions', 'request_permissions_tool', 'request_rule',
  'unbounded_connection_retries', 'deferred_executor', 'deferred_tool_world_state',
  'current_time_reminder', 'sleep_tool', 'send_async_message', 'remote_control',
  'in_app_local_automation', 'undo', 'worktrees', 'codex_git_commit',
] as const;

export function restrictedCodexConfig(stateDirectory: string): Json {
  const config: Json = {
    approval_policy: 'never', sandbox_mode: 'read-only', web_search: 'disabled',
    project_doc_max_bytes: 0, project_doc_fallback_filenames: [],
    include_apps_instructions: false, include_environment_context: false,
    include_collaboration_mode_instructions: false,
    developer_instructions: '', instructions: '',
    'skills.include_instructions': false, 'skills.bundled.enabled': false,
    'orchestrator.skills.enabled': false, 'orchestrator.mcp.enabled': false,
    'agents.enabled': false,
    'features.skip_host_skill_discovery': true,
    // Dynamic tools need this host. It exposes an isolated V8 wrapper with no
    // filesystem, process or network APIs. agents.enabled=false also disables
    // the host's separate delegation tools.
    'features.code_mode_host': true,
    'tools.update_plan.enabled': false,
    'tools.experimental_request_user_input.enabled': false,
    notify: [],
    'history.persistence': 'none', sqlite_home: stateDirectory,
    'shell_environment_policy.inherit': 'none',
    'shell_environment_policy.experimental_use_profile': false,
  };
  for (const feature of DISABLED_CODEX_FEATURES) config[`features.${feature}`] = false;
  return config;
}

function tomlLiteral(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(tomlLiteral).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.entries(value).map(([k,v]) => `${JSON.stringify(k)}=${tomlLiteral(v)}`).join(',')}}`;
  return JSON.stringify(value);
}

export function codexArguments(config: Json): string[] {
  return ['app-server', '--listen', 'stdio://', ...Object.entries(config).flatMap(([key, value]) => ['-c', `${key}=${tomlLiteral(value)}`])];
}

function runtimeEnvironment(codexHome?: string): NodeJS.ProcessEnv {
  // Do not forward DATABASE_URL, GitHub tokens, owner credentials or arbitrary
  // process environment into the runtime. Authentication stays in Codex's store.
  return {
    HOME: homedir(), PATH: process.env.PATH ?? '/usr/bin:/bin',
    TMPDIR: tmpdir(), LANG: 'en_US.UTF-8',
    ...(codexHome ? { CODEX_HOME: codexHome } : {}),
  };
}

export class CodexRpc {
  private child: ReturnType<typeof spawn>;
  private pending = new Map<string | number, Pending>();
  private nextId = 1;
  private closed = false;
  private stderr = '';
  onNotification: (message: RpcMessage) => void | Promise<void> = () => {};
  onRequest: (message: RpcMessage) => Promise<any> = async () => { throw new Error('Runtime operation is not permitted'); };
  onExit: (error: Error) => void = () => {};

  constructor(binary: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv; requestTimeoutMs?: number }) {
    this.timeout = options.requestTimeoutMs ?? 20_000;
    this.child = spawn(binary, args, { cwd: options.cwd, env: options.env, stdio: ['pipe', 'pipe', 'pipe'] });
    this.child.stderr!.on('data', chunk => { this.stderr = (this.stderr + chunk.toString()).slice(-4000); });
    const lines = createInterface({ input: this.child.stdout! });
    lines.on('line', line => {
      if (line.length > 4_000_000) { this.fail(new Error('Oversized runtime protocol line')); return; }
      let message: RpcMessage;
      try { message = JSON.parse(line); } catch { this.fail(new Error('Invalid runtime protocol JSON')); return; }
      if (message.id !== undefined && message.method) {
        Promise.resolve(this.onRequest(message)).then(
          result => this.write({ id: message.id, result }),
          () => this.write({ id: message.id, error: { code: -32000, message: 'Workspace denied this operation' } }),
        ).catch(() => {});
      } else if (message.id !== undefined) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        clearTimeout(pending.timer); this.pending.delete(message.id);
        if (message.error) pending.reject(new Error(message.error.message));
        else pending.resolve(message.result);
      } else if (message.method) {
        Promise.resolve(this.onNotification(message)).catch(error => this.fail(error));
      }
    });
    this.child.on('error', error => this.fail(error));
    this.child.on('exit', (code, signal) => this.fail(new Error(`Codex process exited (${code ?? signal ?? 'unknown'})`)));
  }
  private timeout: number;
  private write(message: unknown) {
    if (this.closed || !this.child.stdin?.writable) throw new Error('Runtime transport is closed');
    this.child.stdin.write(JSON.stringify(message) + '\n');
  }
  private fail(error: Error) {
    if (this.closed) return;
    this.closed = true;
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(error); }
    this.pending.clear(); this.onExit(error);
  }
  request(method: string, params: Json = {}): Promise<any> {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Runtime request timed out: ${method}`)); }, this.timeout);
      this.pending.set(id, { resolve, reject, timer });
      try { this.write({ id, method, params }); }
      catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error); }
    });
  }
  notify(method: string, params: Json = {}) { this.write({ method, params }); }
  async close() {
    this.onExit = () => {};
    this.fail(new Error('Runtime transport closed'));
    this.child.stdin?.end(); this.child.kill('SIGTERM');
    await new Promise<void>(resolve => {
      if (this.child.exitCode !== null || this.child.signalCode !== null) return resolve();
      const timer = setTimeout(() => { this.child.kill('SIGKILL'); resolve(); }, 1500);
      this.child.once('exit', () => { clearTimeout(timer); resolve(); });
    });
  }
}

export interface CodexConnectorOptions {
  binary?: string;
  model?: string;
  codexHome?: string;
  requestTimeoutMs?: number;
  /** Local protocol fixtures only. Never accepted through HTTP or model input. */
  fixtureConfig?: Json;
}

const TOOL_NAME = 'workspace';
const ALLOWED_OPERATIONS = new Set(WORKSPACE_TOOL_SCHEMA.properties.operation.enum);
const REPOSITORY_OPERATIONS = ['repo.list', 'repo.read', 'repo.write', 'repo.check'] as const;
const EXECUTION_TOOL_SCHEMA = {
  ...WORKSPACE_TOOL_SCHEMA,
  properties: {
    ...WORKSPACE_TOOL_SCHEMA.properties,
    operation: { type: 'string', enum: [...ALLOWED_OPERATIONS, ...REPOSITORY_OPERATIONS] },
  },
};

export class CodexConnector implements RuntimeConnector {
  constructor(private options: CodexConnectorOptions = {}) {}
  async capabilities(): Promise<Record<string, unknown>> {
    try {
      const { stdout } = await promisify(execFile)(this.options.binary ?? 'codex', ['--version'], { timeout: 5000, env: runtimeEnvironment(this.options.codexHome) });
      const version = stdout.trim().replace(/^codex-cli\s+/, '');
      const compatible = version === QUALIFIED_CODEX_VERSION;
      return {
        runtime: 'codex-app-server', version, profile: CODEX_PROFILE_VERSION,
        chat: compatible, execution: compatible, freshSessions: true,
        scopedDynamicTools: compatible, environmentAccess: false,
        toolExecution: 'isolated V8 wrapper; only checked workspace commands; no Node, filesystem or network API',
        executionLimits: 'repo.list/read/write and fixed repo.check only; no shell or arbitrary test scripts',
        steering: false, interruption: compatible, quotaTelemetry: compatible,
        reconciliation: 'durable receipt required; no automatic redispatch',
        qualification: compatible ? 'pinned-runtime-profile' : 'unqualified-version',
        ...(!compatible ? { reason: `Runtime ${version} differs from qualified ${QUALIFIED_CODEX_VERSION}` } : {}),
      };
    } catch { return { runtime: 'codex-app-server', chat: false, execution: false, qualification: 'unavailable', reason: 'Codex CLI could not be started' }; }
  }

  async run(input: RuntimeInput): Promise<RuntimeResult> {
    if (input.signal.aborted) return { status: 'cancelled', text: '' };
    const capabilities = await this.capabilities();
    if (!capabilities.chat) return { status: 'failed', text: '', error: String(capabilities.reason) };
    const directory = await mkdtemp(join(tmpdir(), 'workspace-codex-'));
    await mkdir(join(directory, 'state'), { mode: 0o700 });
    const config = { ...restrictedCodexConfig(join(directory, 'state')), ...this.options.fixtureConfig };
    const rpc = new CodexRpc(this.options.binary ?? 'codex', codexArguments(config), {
      cwd: directory, env: runtimeEnvironment(this.options.codexHome), requestTimeoutMs: this.options.requestTimeoutMs,
    });
    let threadId: string | undefined;
    let turnId: string | undefined;
    let dispatched = false;
    let stopping = false;
    let completed = false;
    const toolCalls = new Map<string, { digest: string; response: Promise<unknown> }>();
    const messages = new Map<string, string>();
    let resolveTerminal!: (result: RuntimeResult) => void;
    const terminal = new Promise<RuntimeResult>(resolve => { resolveTerminal = resolve; });
    let stopTimer: ReturnType<typeof setTimeout> | undefined;
    let runtimeTimer: ReturnType<typeof setTimeout> | undefined;
    let eventChain = Promise.resolve();
    const event = (type: string, data: Json) => {
      eventChain = eventChain.then(() => input.onEvent(type, data));
      return eventChain;
    };
    const finish = (result: RuntimeResult) => {
      if (completed) return;
      completed = true;
      resolveTerminal({ ...result, threadId, turnId });
    };
    const currentText = () => [...messages.values()].join('\n\n');
    const stop = async (reason: string) => {
      if (stopping || completed) return;
      stopping = true;
      try { await event('runtime.interrupt_requested', { threadId, turnId, reason }); } catch {}
      if (!dispatched) { finish({ status: 'cancelled', text: '' }); return; }
      // A timeout or a transport close cannot prove the provider stopped.
      stopTimer = setTimeout(() => finish({ status: 'uncertain', text: currentText(), error: 'Interrupt was not followed by an observed terminal event' }), 8000);
      if (threadId && turnId) {
        try { await rpc.request('turn/interrupt', { threadId, turnId }); await event('runtime.interrupt_acknowledged', { threadId, turnId }); }
        catch { /* The terminal deadline retains uncertainty. */ }
      }
    };
    const onAbort = () => { void stop('owner-stop'); };
    input.signal.addEventListener('abort', onAbort, { once: true });
    rpc.onExit = error => finish({ status: dispatched ? 'uncertain' : 'failed', text: currentText(), error: error.message });
    rpc.onRequest = async message => {
      const params = message.params ?? {};
      if (message.method !== 'item/tool/call') throw new Error('Unsupported runtime operation');
      if (stopping || completed || input.signal.aborted) throw new Error('Run no longer accepts tools');
      if (params.threadId !== threadId || !turnId || params.turnId !== turnId || params.tool !== TOOL_NAME || params.namespace) throw new Error('Tool run binding mismatch');
      const args = params.arguments;
      const allowed = ALLOWED_OPERATIONS.has(args?.operation) || (input.mode === 'execution' && REPOSITORY_OPERATIONS.includes(args?.operation));
      if (!args || typeof args !== 'object' || !allowed || !args.input || typeof args.input !== 'object' || Array.isArray(args.input)) throw new Error('Invalid workspace command');
      if (typeof params.callId !== 'string' || !params.callId || params.callId.length > 256) throw new Error('Invalid tool call identity');
      const digest = createHash('sha256').update(JSON.stringify(args)).digest('hex');
      const previous = toolCalls.get(params.callId);
      if (previous) {
        if (previous.digest !== digest) throw new Error('Tool call identity reused with different arguments');
        return previous.response;
      }
      if (toolCalls.size >= 24) return { success: false, contentItems: [{ type: 'inputText', text: 'The bounded tool allowance for this response has been reached. Finish with the available evidence.' }] };
      const perform = async () => {
        await event('runtime.tool_requested', { threadId, turnId, callId: params.callId, operation: args.operation });
        try {
          const result = await input.tool(args.operation, args.input, { callId: params.callId, threadId: threadId!, turnId: turnId! });
          const text = JSON.stringify(result ?? null);
          if (Buffer.byteLength(text) > 120_000) throw new Error('Tool result exceeds the bounded context limit');
          await event('runtime.tool_completed', { threadId, turnId, callId: params.callId, operation: args.operation });
          return { success: true, contentItems: [{ type: 'inputText', text }] };
        } catch (error) {
          const message = error instanceof Error ? error.message : 'Workspace command failed';
          await event('runtime.tool_failed', { threadId, turnId, callId: params.callId, operation: args.operation });
          return { success: false, contentItems: [{ type: 'inputText', text: JSON.stringify({ error: message }) }] };
        }
      };
      const response = perform();
      toolCalls.set(params.callId, { digest, response });
      return response;
    };
    rpc.onNotification = async message => {
      const params = message.params ?? {};
      if (params.threadId && threadId && params.threadId !== threadId) return;
      if (message.method === 'turn/started') {
        turnId = params.turn?.id ?? turnId;
        await event('runtime.accepted', { threadId, turnId });
        if (stopping && threadId && turnId) {
          try { await rpc.request('turn/interrupt', { threadId, turnId }); } catch {}
        }
      }
      if (message.method === 'item/agentMessage/delta' && typeof params.delta === 'string') {
        const key = params.itemId ?? 'answer';
        messages.set(key, (messages.get(key) ?? '') + params.delta);
        await event('runtime.text_delta', { threadId, turnId, itemId: key, delta: params.delta });
      }
      if (message.method === 'item/completed' && params.item?.type === 'agentMessage') {
        messages.set(params.item.id ?? 'answer', params.item.text ?? '');
      }
      if (message.method === 'item/completed' && params.item?.type !== 'agentMessage') {
        const item = params.item ?? {};
        await event('runtime.item_completed', { threadId, turnId, itemId: item.id, itemType: item.type, tool: item.tool, status: item.status });
      }
      if (message.method === 'account/rateLimits/updated') await event('runtime.quota', { rateLimits: params.rateLimits, rateLimitsByLimitId: params.rateLimitsByLimitId, observedAt: new Date().toISOString() });
      if (message.method === 'turn/completed') {
        turnId = params.turn?.id ?? turnId;
        const status = params.turn?.status;
        await event('runtime.terminal', { threadId, turnId, status });
        finish({ status: status === 'completed' ? 'completed' : status === 'interrupted' ? 'cancelled' : 'failed', text: currentText(), error: params.turn?.error?.message });
      }
    };
    try {
      await rpc.request('initialize', { clientInfo: { name: 'private_workspace', title: 'Private workspace', version: '0.0.1' }, capabilities: { experimentalApi: true } });
      rpc.notify('initialized');
      // Read only configuration metadata. Never persist or return the contents,
      // which may contain connector environment variables or authorization data.
      const effective = await rpc.request('config/read', { includeLayers: false, cwd: directory });
      for (const name of Object.keys(effective?.config?.mcp_servers ?? {})) {
        if (!/^[a-zA-Z0-9_-]+$/.test(name)) throw new Error('Cannot safely disable a configured MCP server with an unsupported name');
        config[`mcp_servers.${name}.enabled`] = false;
      }
      try { const quota = await rpc.request('account/rateLimits/read', {}); await event('runtime.quota', { ...quota, observedAt: new Date().toISOString() }); } catch { await event('runtime.quota_unavailable', { observedAt: new Date().toISOString() }); }
      const models = await rpc.request('model/list', {});
      const model = this.options.model ?? models.data?.find((entry: Json) => entry.isDefault)?.model;
      if (!model) throw new Error('Runtime did not advertise a default supported model');
      // A per-thread override complements CLI flags against project/config merging.
      if (input.signal.aborted || stopping) return { status: 'cancelled', text: '' };
      const thread = await rpc.request('thread/start', {
        ephemeral: true, cwd: directory, environments: [], runtimeWorkspaceRoots: [],
        approvalPolicy: 'never', sandbox: 'read-only',
        model,
        config,
        baseInstructions: 'You are an AI participant in a private workspace. Answer the current request using the supplied context. Only the workspace tool is available. Be concise and truthful about actions and missing context.',
        developerInstructions: `${input.instructions}\nWorkspace actor: ${input.actorId}. Workspace run: ${input.runId}. Tools are permission checked for this run. You have no host files, shell, browser, other conversations or private memory. Never claim a Resource was saved until a resource.create or resource.revise call succeeds.`,
        dynamicTools: [{ type: 'function', name: TOOL_NAME, description: 'Read the current conversation, list/read/create/revise permitted Resources, list tasks, or propose a reviewable plan. resource.create input is {title, text, type: "note"|"paste"|"output", projectIds?: string[], spaceIds?: string[]}; it returns {resource, revision}. resource.read input is {id, revisionId?}; read text at revision.data.text. resource.revise input is {id, expectedRevision: number, text, title?}; expectedRevision is the current resource.revision. plan.propose input is {title, projectId?, tasks: [{title, ...planningFields}]}; it creates a proposal for owner review and grants no execution permission. conversation.read, resource.list and task.list accept {}.' + (input.mode === 'execution' ? ' The approved checkout also supports repo.list with {path?: string}, repo.read with {path: string}, repo.write with {path: string, text: string}, and repo.check with {}. All paths are relative to the fixed approved checkout. These fixed operations do not allow shell commands, arbitrary host files or credentials.' : ''), inputSchema: input.mode === 'execution' ? EXECUTION_TOOL_SCHEMA : WORKSPACE_TOOL_SCHEMA }],
      });
      threadId = thread.thread?.id;
      if (!threadId || thread.instructionSources?.length) throw new Error('Runtime loaded unexpected instruction sources');
      await event('runtime.thread_created', { threadId, model: thread.model, profile: CODEX_PROFILE_VERSION, runtimeVersion: QUALIFIED_CODEX_VERSION, environmentAccess: false });
      if (input.signal.aborted || stopping) return { status: 'cancelled', text: '', threadId };
      await event('runtime.dispatch_intent', { threadId, clientUserMessageId: input.runId });
      dispatched = true;
      runtimeTimer = setTimeout(() => { void stop('run-deadline'); }, Math.max(100, Math.min(input.timeoutMs, 15 * 60_000)));
      const turn = await rpc.request('turn/start', {
        threadId, clientUserMessageId: input.runId,
        input: [{ type: 'text', text: input.prompt }],
        environments: [], runtimeWorkspaceRoots: [],
        approvalPolicy: 'never', sandboxPolicy: { type: 'readOnly', networkAccess: false },
        effort: 'low',
      });
      turnId = turn.turn?.id ?? turnId;
      await event('runtime.start_acknowledged', { threadId, turnId });
      const result = await terminal;
      await eventChain;
      return result;
    } catch (error) {
      return { status: dispatched ? 'uncertain' : 'failed', text: currentText(), threadId, turnId, error: error instanceof Error ? error.message : 'Runtime failed' };
    } finally {
      if (runtimeTimer) clearTimeout(runtimeTimer);
      if (stopTimer) clearTimeout(stopTimer);
      input.signal.removeEventListener('abort', onAbort);
      await rpc.close();
      await rm(directory, { recursive: true, force: true });
    }
  }
}
