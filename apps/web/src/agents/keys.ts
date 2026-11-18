// Agents' query keys, apart from their API so the live connection can name
// them without loading it (W1.3).

export const agentKeys = {
  profiles: (projectId: string) => ['project', projectId, 'agent-profiles'] as const,
  presence: (projectId: string) => ['project', projectId, 'agents'] as const,
}
