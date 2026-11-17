// Collections' query keys, apart from their API so the live connection can name them
// without loading it (W1.2).

export const collectionKeys = {
  all: (projectId: string) => ['project', projectId, 'collections'] as const,
  list: (projectId: string, includeArchived: boolean) => ['project', projectId, 'collections', 'list', includeArchived] as const,
  one: (projectId: string, collectionId: string) => ['project', projectId, 'collections', collectionId] as const,
  /** Every cached list of a project's collections, archived or not. */
  lists: (projectId: string) => ['project', projectId, 'collections', 'list'] as const,
  /** Every cached page of a collection's records, whatever the view. */
  allRecords: (projectId: string, collectionId: string) => ['project', projectId, 'collections', collectionId, 'records'] as const,
  records: (projectId: string, collectionId: string, query: string) => ['project', projectId, 'collections', collectionId, 'records', query] as const,
  /** Records offered by a reference field's picker. */
  recordPick: (projectId: string, collectionId: string, query: string) =>
    ['project', projectId, 'collections', collectionId, 'records', 'pick', query] as const,
  record: (projectId: string, collectionId: string, recordId: string) => ['project', projectId, 'collections', collectionId, 'record', recordId] as const,
}
