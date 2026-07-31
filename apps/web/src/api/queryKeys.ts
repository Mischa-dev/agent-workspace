export const queryKeys = {
  session: ['session'] as const,
  projects: ['projects', 'rail'] as const,
  project: (projectId: string) => ['project', projectId] as const,
  navigation: (projectId: string) =>
    ['project', projectId, 'navigation'] as const,
  executionProfiles: (projectId: string) =>
    ['project', projectId, 'execution-profiles'] as const,
  executionProfileAccount: (projectId: string, profileId: string) =>
    ['project', projectId, 'execution-profile', profileId, 'account'] as const,
  repositories: (projectId: string) =>
    ['project', projectId, 'repositories'] as const,
  timeline: (
    projectId: string,
    conversationId: string,
    includeArchived = false,
  ) =>
    [
      'project',
      projectId,
      'conversation',
      conversationId,
      'timeline',
      includeArchived ? 'archived' : 'active',
    ] as const,
  conversationSidebar: (
    projectId: string,
    selectedChannelId = '',
    mode = 'compact',
  ) =>
    [
      'project',
      projectId,
      'conversation-sidebar',
      selectedChannelId,
      mode,
    ] as const,
  conversationSidebarRoot: (projectId: string) =>
    ['project', projectId, 'conversation-sidebar'] as const,
  mentionableActors: (projectId: string, query: string) =>
    ['project', projectId, 'mentionable-actors', query] as const,
}
