/** Ready-made setups for the runners in docs/agent-runner.md. */
export type RunnerPresetId = 'claude_code' | 'codex' | 'custom' | 'agent'

export type RunnerPreset = {
  id: RunnerPresetId
  label: string
  displayName: string
  handle: string
  connectionName: string
  /** A runner wakes on a webhook; a plain agent checks in over MCP. */
  runner: boolean
}

export const RUNNER_PRESETS: RunnerPreset[] = [
  { id: 'claude_code', label: 'Claude Code', displayName: 'Claude', handle: 'claude', connectionName: 'Claude Code runner', runner: true },
  { id: 'codex', label: 'Codex', displayName: 'Codex', handle: 'codex', connectionName: 'Codex runner', runner: true },
  { id: 'custom', label: 'Custom runner', displayName: '', handle: '', connectionName: '', runner: true },
  { id: 'agent', label: 'Other agent', displayName: '', handle: '', connectionName: '', runner: false },
]

/** The environment a runner reads, in the names docs/agent-runner.md uses. */
export function runnerSettings({
  origin,
  token,
  webhookSecret,
  handle,
  runner,
}: {
  origin: string
  token: string
  webhookSecret: string | null
  handle: string
  runner: RunnerPresetId
}) {
  return [
    `AGENT_WORKSPACE_MCP_URL=${origin}/api/v1/mcp`,
    `AGENT_WORKSPACE_TOKEN=${token}`,
    ...(webhookSecret ? [`AGENT_WORKSPACE_WEBHOOK_SECRET=${webhookSecret}`] : []),
    `AGENT_WORKSPACE_HANDLE=${handle}`,
    `AGENT_WORKSPACE_RUNNER=${runner === 'agent' ? 'custom' : runner}`,
  ].join('\n')
}
