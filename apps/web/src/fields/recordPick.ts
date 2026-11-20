import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { collectionKeys } from '../collections/keys'
import { useDebouncedValue } from '../work-items/pickerModel'

/** A record as a picker lists it. */
export type RecordHit = { id: string; title: string; number: number }

/** A record as a picker names it. */
export const recordLabel = (record: RecordHit) => record.title || `Untitled #${record.number}`

/** Up to 20 records of `collection` matching `query` (debounced), for a picker. */
export function useRecordPick(projectId: string, collection: string, query: string) {
  const debounced = useDebouncedValue(query, 200)
  return useQuery({
    queryKey: collectionKeys.recordPick(projectId, collection, debounced.trim()),
    enabled: Boolean(collection),
    queryFn: async () => {
      const params = new URLSearchParams({ limit: '20' })
      if (debounced.trim()) params.set('search', debounced.trim())
      const payload = await requestJson<{ records?: Array<{ id?: unknown; title?: unknown; number?: unknown }> }>(
        `/api/v1/projects/${encodeURIComponent(projectId)}/collections/${encodeURIComponent(collection)}/records?${params.toString()}`,
      )
      return (payload.records ?? []).flatMap((record): RecordHit[] =>
        typeof record.id === 'string'
          ? [{ id: record.id, title: typeof record.title === 'string' ? record.title : '', number: typeof record.number === 'number' ? record.number : 0 }]
          : [],
      )
    },
  })
}
