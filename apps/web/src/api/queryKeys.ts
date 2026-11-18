export const queryKeys = {
  session: ['session'] as const,
  projects: ['projects', 'rail'] as const,
  /** Every project list: the rail, search and the archived list. */
  projectLists: ['projects'] as const,
  projectSearch: (query: string) => ['projects', 'search', query] as const,
  projectsArchived: ['projects', 'archived'] as const,
  project: (projectId: string) => ['project', projectId] as const,
  /** A workspace's own emoji (`customEmojiKeys`); without a workspace, every workspace's. */
  customEmoji: ['custom-emoji'] as const,
  customEmojiOf: (workspaceId: string | null) => ['custom-emoji', workspaceId] as const,
  /** Everyone's statuses (`statusKeys`). */
  statuses: ['statuses'] as const,
  /** One thing and its links (`GET /api/v1/things/{kind}/{id}`), shared by embeds and chips. */
  things: ['things'] as const,
  thing: (kind: string, id: string) => ['things', kind, id] as const,
  /** Where a thing is referenced from ("Referenced in"). */
  thingReferences: (kind: string, id: string) => ['things', kind, id, 'references'] as const,
  /** DMs and groups (the Messages overview). */
  direct: ['direct'] as const,
  inbox: ['inbox'] as const,
  myWork: ['my-work'] as const,
  /** Search results, by query and the project they're narrowed to. */
  searchAll: ['search'] as const,
  search: (query: string, projectId: string | undefined) => ['search', query, projectId] as const,
  /** One workspace as Settings › General reads it (its lifecycle too). */
  workspace: (organizationId: string) => ['organization', organizationId] as const,
  /** A workspace's module switches (Settings › Modules). */
  workspaceModules: (organizationId: string) => ['organization', organizationId, 'modules'] as const,
  /** Which project a task key opens. */
  taskKey: (projectId: string, key: string) => ['project', projectId, 'task-key', key] as const,
  /** The command menu's task results for one project. */
  commandMenuTasks: (projectId: string, query: string) =>
    ['project', projectId, 'command-menu', 'tasks', query] as const,
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
  agentRecipes: () => ['agent-recipes'] as const,
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
  workItemAssigneesByIds: (projectId: string, ids: readonly string[]) =>
    ['project', projectId, 'work-items', 'assignees', '', 'ids', ids.join(',')] as const,
  workItemEmbed: (projectId: string, filters: Record<string, unknown>) =>
    ['project', projectId, 'work-items', 'embed', filters] as const,
  taskMoveDestinations: (actorId: string, projectId: string, taskId: string) =>
    ['task-move-destinations', actorId, projectId, taskId] as const,
  /** Repositories a task can link to, for the task's picker. */
  repositoryPicker: (projectId: string) => ['projects', projectId, 'repositories', 'work-item-picker'] as const,
  workItemTimeline: (projectId: string, workItemId: string) =>
    ['project', projectId, 'work-items', 'timeline', workItemId] as const,
  workItemCommentRevisions: (projectId: string, workItemId: string, commentId: string) =>
    ['project', projectId, 'work-items', 'timeline', workItemId, 'comment', commentId] as const,
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
  /** Everything cached about one conversation in a project. */
  conversation: (projectId: string, conversationId: string) =>
    ['project', projectId, 'conversation', conversationId] as const,
  /** A thread's origin message, shown above its replies. */
  originMessage: (projectId: string, parentId: string, originId: string) =>
    ['project', projectId, 'conversation', parentId, 'timeline', 'origin', originId] as const,
  conversationRestorable: (projectId: string, conversationId: string) =>
    ['project', projectId, 'conversation', conversationId, 'restorable'] as const,
  conversationSidebarRoot: (projectId: string) =>
    ['project', projectId, 'conversation-sidebar'] as const,
  mentionableActors: (projectId: string, query: string) =>
    ['project', projectId, 'mentionable-actors', query] as const,
}

/** Whether a cached key is one thing's (`queryKeys.thing`), or any "Referenced in" list. */
export function isThingKey(key: readonly unknown[], kind: string, id: string) {
  return key[0] === queryKeys.things[0] && key[1] === kind && key[2] === id
}

export function isThingReferencesKey(key: readonly unknown[]) {
  return key[0] === queryKeys.things[0] && key[3] === 'references'
}
