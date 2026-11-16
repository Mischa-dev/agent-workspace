// Runs' query keys, apart from their API so the shell can name them without
// loading it (W1.2).

import type { RunListFilter, UsageFilter } from './api'

export const runKeys = {
  /** Every cached run list, whatever its filter. */
  lists: ['runs', 'list'] as const,
  list: (filter: RunListFilter) => ['runs', 'list', filter] as const,
  /** Every cached usage summary, whatever its filter. */
  usages: ['runs', 'usage'] as const,
  usage: (filter: UsageFilter) => ['runs', 'usage', filter] as const,
  detail: (runId: string) => ['runs', 'detail', runId] as const,
  diff: (runId: string) => ['runs', 'detail', runId, 'diff'] as const,
}
