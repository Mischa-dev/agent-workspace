export interface RuntimeInput {
  runId: string;
  actorId: string;
  prompt: string;
  instructions: string;
  mode: 'chat' | 'execution';
  cwd: string;
  timeoutMs: number;
  onEvent: (type: string, data: Record<string, unknown>) => Promise<void>;
  tool: (name: string, input: Record<string, unknown>) => Promise<unknown>;
  signal: AbortSignal;
}
export interface RuntimeResult {
  status: 'completed' | 'cancelled' | 'failed' | 'uncertain';
  text: string;
  threadId?: string;
  turnId?: string;
  error?: string;
}
export interface RuntimeConnector {
  capabilities(): Promise<Record<string, unknown>>;
  run(input: RuntimeInput): Promise<RuntimeResult>;
}
// Tools are bounded application commands. Identity and scope are injected by
// the worker, never accepted as model supplied tool arguments.
export const WORKSPACE_TOOL_SCHEMA = {
  type: 'object', properties: {
    operation: { type: 'string', enum: ['conversation.read','resource.list','resource.read','resource.create','resource.revise','plan.propose','task.list'] },
    input: { type: 'object', additionalProperties: true }
  }, required: ['operation','input'], additionalProperties: false
} as const;
