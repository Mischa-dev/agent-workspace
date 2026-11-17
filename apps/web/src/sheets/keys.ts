// Sheets' query keys, apart from their API so the live connection can name them
// without loading it (W1.2).

export const sheetKeys = {
  all: (projectId: string) => ['project', projectId, 'sheets'] as const,
  list: (projectId: string) => ['project', projectId, 'sheets', 'list'] as const,
  one: (projectId: string, sheetId: string) => ['project', projectId, 'sheets', sheetId] as const,
  tab: (projectId: string, sheetId: string, tab: string) => ['project', projectId, 'sheets', sheetId, tab] as const,
}
