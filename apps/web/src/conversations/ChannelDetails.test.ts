import { describe, expect, it } from 'vitest'

import { channelDetailsTabs } from './channelDetailsTabs'

describe('channel details tabs (C2.8)', () => {
  it('shows Members only for a private channel, and Tasks and Files only where they are on', () => {
    expect(channelDetailsTabs({ isPrivate: false, tasks: true, files: true }).map((tab) => tab.value)).toEqual([
      'about',
      'pins',
      'tasks',
      'files',
    ])
    expect(channelDetailsTabs({ isPrivate: true, tasks: false, files: false }).map((tab) => tab.value)).toEqual([
      'about',
      'members',
      'pins',
    ])
  })
})
