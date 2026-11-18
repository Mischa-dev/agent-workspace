import { describe, expect, it } from 'vitest'

import { queuedTimings, recordTiming } from './timings'

describe('speed timings', () => {
  it('queues sensible timings by name only', () => {
    recordTiming('conversation_open', 180.4)
    recordTiming('message_send', -5)
    recordTiming('inbox_open', Number.NaN)
    recordTiming('search_results', 500_000)
    expect(queuedTimings()).toEqual([{ name: 'conversation_open', ms: 180 }])
  })
})
