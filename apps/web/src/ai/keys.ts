// Planning's query keys, apart from its API so the live connection can name
// them without loading it (W1.3).

export const aiKeys = {
  root: (projectId: string) => ['project', projectId, 'ai'] as const,
  status: (projectId: string) => ['project', projectId, 'ai', 'status'] as const,
  suggestions: (projectId: string, filters: Record<string, unknown>) => ['project', projectId, 'ai', 'suggestions', filters] as const,
  ranking: (projectId: string, filters: Record<string, unknown>) => ['project', projectId, 'ai', 'ranking', filters] as const,
  graph: (projectId: string, filters: Record<string, unknown>) => ['project', projectId, 'ai', 'graph', filters] as const,
  plans: (projectId: string) => ['project', projectId, 'ai', 'plans'] as const,
  plan: (projectId: string, importId: string) => ['project', projectId, 'ai', 'plans', importId] as const,
  readiness: (projectId: string, workItemId: string) => ['project', projectId, 'ai', 'readiness', workItemId] as const,
  /** A task's readiness at one version of the task, so an edit rechecks it. */
  readinessAt: (projectId: string, workItemId: string, version: number) =>
    ['project', projectId, 'ai', 'readiness', workItemId, version] as const,
  /** A conversation's catch-up summary over one range. */
  catchUp: (conversationId: string, range: string) => ['conversation', conversationId, 'ai-summary', range] as const,
  workspace: (organizationId: string) => ['organization', organizationId, 'ai'] as const,
}
