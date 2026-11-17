// Pages' query keys, apart from their API so the live connection can name them
// without loading it (W1.2).

export const memoryKeys = {
  pages: (projectId: string) => ['project', projectId, 'memory', 'pages'] as const,
  page: (projectId: string, pageId: string) => ['project', projectId, 'memory', 'page', pageId] as const,
  decisions: (projectId: string) => ['project', projectId, 'memory', 'decisions'] as const,
  decisionList: (projectId: string, showSuperseded: boolean) =>
    ['project', projectId, 'memory', 'decisions', showSuperseded] as const,
  /** A page's discussion (its comments). */
  discussion: (projectId: string, pageId: string) => ['project', projectId, 'memory', 'page', pageId, 'discussion'] as const,
  /** The hidden space that holds a workspace's own pages. */
  workspacePagesSpace: (organizationId: string | null | undefined) => ['organization', organizationId, 'pages-space'] as const,
  pins: (projectId: string, conversationId: string) =>
    ['project', projectId, 'memory', 'pins', conversationId] as const,
}

/** A project's page list or one of its pages (with what hangs off it). */
export function isPageKey(key: readonly unknown[], projectId: string) {
  const page = memoryKeys.page(projectId, '')
  return key[0] === page[0] && key[1] === projectId && key[2] === page[2] && (key[3] === page[3] || key[3] === memoryKeys.pages(projectId)[3])
}
