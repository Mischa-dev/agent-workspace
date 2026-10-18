import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'

import { SLASH_COMMANDS } from './slashCommands'
import { useConversationTools } from './useConversationTools'

const createWorkItem = vi.fn()
vi.mock('../work-items/api', () => ({ createWorkItem: (input: unknown) => createWorkItem(input) }))
vi.mock('./api', () => ({
  invalidateLater: vi.fn(),
  laterKeys: { level: (id: string) => ['level', id] },
  saveForLater: vi.fn(),
  scheduleMessage: vi.fn(),
  setLevel: vi.fn(),
  useConversationLevel: () => ({ data: undefined }),
  useScheduled: () => ({ data: [] }),
}))

describe('/task', () => {
  it('uses a new idempotency key for each task, and the same one to retry a failure', async () => {
    const queryClient = new QueryClient()
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter>{children}</MemoryRouter>
      </QueryClientProvider>
    )
    const { result } = renderHook(
      () => useConversationTools({ canPost: true, conversationId: 'c1', kind: 'channel', projectId: 'p1' }),
      { wrapper },
    )
    const task = SLASH_COMMANDS.find((command) => command.name === 'task')!
    const run = (args: string) => result.current.composer.onCommand!(task, args, () => null)

    createWorkItem.mockRejectedValueOnce(new TypeError('Failed to fetch'))
    await expect(run('First')).rejects.toThrow()
    createWorkItem.mockResolvedValue({ id: 'w1', key: 'AW-1', title: 'First' })
    await run('First')
    await run('Second')

    const keys = createWorkItem.mock.calls.map(([input]) => (input as { idempotencyKey: string }).idempotencyKey)
    expect(keys[0]).toBe(keys[1])
    expect(keys[2]).not.toBe(keys[1])
  })
})
