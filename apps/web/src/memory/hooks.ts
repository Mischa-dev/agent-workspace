import { useQuery } from '@tanstack/react-query'

import { listPins, memoryKeys } from './api'

export function usePins(conversationId: string) {
  return useQuery({
    queryKey: memoryKeys.pins(conversationId),
    queryFn: () => listPins(conversationId),
    enabled: Boolean(conversationId),
    staleTime: 15_000,
  })
}
