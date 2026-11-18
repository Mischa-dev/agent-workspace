import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'

import { SLASH_COMMANDS } from './slashCommands'
import { useConversationTools } from './useConversationTools'

const callOp = vi.fn()
vi.mock('../api/ops', async (original) => ({
  ...(await original<typeof import('../api/ops')>()),
  callOp: (name: string, input: unknown, options: unknown) => callOp(name, input, options),
}))
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
  it('runs its manifest’s operation, with a new request id for each task and the same one to retry a failure', async () => {
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

    callOp.mockRejectedValueOnce(new TypeError('Failed to fetch'))
    await expect(run('First')).rejects.toThrow()
    callOp.mockResolvedValue({ workItemId: 'w1', key: 'AW-1' })
    await run('First')
    await run('Second')
    await expect(run('')).rejects.toThrow('Add the title after /task.')

    expect(callOp.mock.calls.map(([name, input]) => [name, input])).toEqual([
      ['task_create', { title: 'First', projectId: 'p1' }],
      ['task_create', { title: 'First', projectId: 'p1' }],
      ['task_create', { title: 'Second', projectId: 'p1' }],
    ])
    const keys = callOp.mock.calls.map(([, , options]) => (options as { requestId: string }).requestId)
    expect(keys[0]).toBe(keys[1])
    expect(keys[2]).not.toBe(keys[1])
  })
})
