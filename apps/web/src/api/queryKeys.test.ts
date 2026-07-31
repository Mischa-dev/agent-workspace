import { describe, expect, it } from 'vitest'

import { queryKeys } from './queryKeys'

describe('query-key isolation', () => {
  it('scopes the same Thread identifier to its route Project', () => {
    expect(queryKeys.timeline('project-a', 'thread-shared')).not.toEqual(
      queryKeys.timeline('project-b', 'thread-shared'),
    )
    expect(queryKeys.timeline('project-a', 'thread-shared')).toEqual([
      'project',
      'project-a',
      'conversation',
      'thread-shared',
      'timeline',
      'active',
    ])
  })
})
