// Stand-ins for the OpenClaw plugin SDK pieces the plugin imports, with the
// shapes of OpenClaw 2026.11.5. They record what the plugin hands them.

export const calls: { name: string; args: unknown }[] = []

export function createChatChannelPlugin<T>(options: T): T {
  return options
}

export function createChannelPluginBase<T>(options: T): T {
  return options
}

export function defineChannelPluginEntry<T>(options: T): T {
  return options
}

export function defineSetupPluginEntry<T>(plugin: T): { plugin: T } {
  return { plugin }
}

export function buildChannelOutboundSessionRoute(params: Record<string, unknown>) {
  calls.push({ name: 'buildChannelOutboundSessionRoute', args: params })
  return { sessionKey: `agent:main:${String(params.channel)}:${String(params.to)}` }
}

export function resolveAgentRoute(params: { channel: string; accountId: string; peer: { kind: string; id: string } }) {
  calls.push({ name: 'resolveAgentRoute', args: params })
  return {
    agentId: 'main',
    accountId: params.accountId,
    dmScope: 'per-channel-peer',
    sessionKey: `agent:main:${params.channel}:${params.peer.kind}:${params.peer.id}`,
  }
}

export async function waitUntilAbort(signal: AbortSignal): Promise<void> {
  if (signal.aborted) return
  await new Promise<void>((resolve) => signal.addEventListener('abort', () => resolve(), { once: true }))
}

export function channelReadyPatch(params: { accountId: string }) {
  return { ...params, lifecycle: 'ready', running: true }
}

export function channelStoppedPatch(params: { accountId: string }) {
  return { ...params, lifecycle: 'stopped', running: false }
}
