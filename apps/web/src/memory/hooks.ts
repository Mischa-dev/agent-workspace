import { useQuery } from '@tanstack/react-query'

import { listPins, memoryKeys } from './api'

export function usePins(projectId: string, conversationId: string) {
  return useQuery({
    queryKey: memoryKeys.pins(projectId, conversationId),
    queryFn: () => listPins(conversationId),
    enabled: Boolean(projectId && conversationId),
    staleTime: 15_000,
  })
}
