// Later's query keys, apart from its API so the live connection can name
// them without loading it (W1.3).

import type { LaterView } from './api'

export const laterKeys = {
  root: ['later'] as const,
  list: (view: LaterView) => ['later', 'list', view] as const,
  scheduled: (conversationId: string | null) => ['later', 'scheduled', conversationId ?? 'all'] as const,
  level: (conversationId: string) => ['later', 'level', conversationId] as const,
}
