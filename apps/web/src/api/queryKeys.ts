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
  harnesses: (projectId: string) => ['project', projectId, 'harnesses'] as const,
  providerAccounts: (projectId: string) =>
    ['project', projectId, 'provider-accounts'] as const,
  providerAccountStatus: (projectId: string, accountId: string) =>
    ['project', projectId, 'provider-accounts', accountId, 'status'] as const,
  repositories: (projectId: string) =>
    ['project', projectId, 'repositories'] as const,
  members: (projectId: string) => ['project', projectId, 'members'] as const,
  invitations: (projectId: string) =>
    ['project', projectId, 'invitations'] as const,
  externalAgents: (projectId: string) =>
    ['project', projectId, 'external-agents'] as const,
  sponsoredExecutions: (projectId: string) =>
    ['project', projectId, 'sponsored-executions'] as const,
  githubIntegration: (projectId: string) =>
    ['project', projectId, 'github-integration'] as const,
  githubAutomation: (projectId: string) =>
    ['project', projectId, 'github-automation'] as const,
  workItemsRoot: (projectId: string) =>
    ['project', projectId, 'work-items'] as const,
  workItems: (projectId: string, filters: Record<string, unknown>) =>
    ['project', projectId, 'work-items', 'list', filters] as const,
  workItem: (projectId: string, workItemId: string) =>
    ['project', projectId, 'work-items', 'detail', workItemId] as const,
  workItemWorkflow: (projectId: string) =>
    ['project', projectId, 'work-items', 'workflow'] as const,
  workItemAssignees: (projectId: string, query: string) =>
    ['project', projectId, 'work-items', 'assignees', query] as const,
  workItemTimeline: (projectId: string, workItemId: string) =>
    ['project', projectId, 'work-items', 'timeline', workItemId] as const,
  workItemFiles: (projectId: string, workItemId: string) =>
    ['project', projectId, 'work-items', 'files', workItemId] as const,
  workItemPages: (projectId: string, filters: Record<string, unknown>) =>
    ['project', projectId, 'work-items', 'pages', filters] as const,
  workItemGroups: (projectId: string, filters: Record<string, unknown>) =>
    ['project', projectId, 'work-items', 'groups', filters] as const,
  workItemBoard: (projectId: string, filters: Record<string, unknown>) =>
    ['project', projectId, 'work-items', 'board', filters] as const,
  workItemLabels: (projectId: string, includeArchived = false) =>
    ['project', projectId, 'work-items', 'labels', includeArchived ? 'all' : 'active'] as const,
  workItemMilestones: (projectId: string, includeArchived = false) =>
    ['project', projectId, 'work-items', 'milestones', includeArchived ? 'all' : 'active'] as const,
  workItemReferences: (projectId: string, query: string) =>
    ['project', projectId, 'work-items', 'references', query] as const,
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
  messageRevisions: (projectId: string, conversationId: string, messageId: string) =>
    [
      'project',
      projectId,
      'conversation',
      conversationId,
      'message',
      messageId,
      'revisions',
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
