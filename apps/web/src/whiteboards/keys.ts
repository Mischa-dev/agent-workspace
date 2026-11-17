// Whiteboards' query keys, apart from their API so the live connection can name them
// without loading it (W1.2).

import type { LinkTargetKind } from './api'

export const whiteboardKeys = {
  all: ['whiteboards'] as const,
  list: (filters: Record<string, unknown>) => ['whiteboards', 'list', filters] as const,
  detail: (id: string) => ['whiteboards', 'detail', id] as const,
  /** The scene the canvas loads once. */
  scene: (id: string) => ['whiteboard-scene', id] as const,
  linked: (kind: LinkTargetKind, targetId: string) => ['whiteboards', 'linked', kind, targetId] as const,
}
