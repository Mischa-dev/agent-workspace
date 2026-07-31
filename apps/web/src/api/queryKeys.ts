export const queryKeys = {
  session: ['session'] as const,
  projects: ['projects', 'rail'] as const,
  project: (projectId: string) => ['project', projectId] as const,
  navigation: (projectId: string) =>
    ['project', projectId, 'navigation'] as const,
  timeline: (projectId: string, threadId: string) =>
    ['project', projectId, 'thread', threadId, 'timeline'] as const,
}
