import { beforeEach, expect, it, vi } from 'vitest'

import { getProjectConversationSidebar, markConversationSeen } from '../conversations/sidebarApi'
import type { SidebarChannel } from '../conversations/types'
import { getDirectOverview } from '../direct/api'
import { markAllRead } from './markAllRead'

vi.mock('../conversations/sidebarApi', () => ({
  getProjectConversationSidebar: vi.fn(),
  markConversationSeen: vi.fn(),
}))
vi.mock('../direct/api', () => ({ getDirectOverview: vi.fn() }))

beforeEach(() => vi.resetAllMocks())

it('reports an expanded channel failure while marking the unread conversations it did fetch', async () => {
  vi.mocked(getDirectOverview).mockResolvedValue({ conversations: [] } as unknown as Awaited<ReturnType<typeof getDirectOverview>>)
  vi.mocked(getProjectConversationSidebar).mockResolvedValueOnce({
    channels: [{ id: 'channel', highestSequence: 4, highestSeenSequence: 0, hasMoreThreads: true, threads: [] } as unknown as SidebarChannel],
  } as Awaited<ReturnType<typeof getProjectConversationSidebar>>).mockRejectedValueOnce(new Error('Offline'))
  vi.mocked(markConversationSeen).mockResolvedValue(undefined as unknown as Awaited<ReturnType<typeof markConversationSeen>>)

  expect(await markAllRead(['project'])).toEqual({ marked: 1, failed: 1 })
  expect(markConversationSeen).toHaveBeenCalledWith({ conversationId: 'channel', highestSeenSequence: 4 })
})
