import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../api/errors'
import { resetSaveQueue, undoLast } from '../views/saveQueue'
import { getWorkItem, updateWorkItem, writeWorkItem } from './api'
import type { PickResult } from './FieldPicker'
import { useApplyPick } from './taskEdits'
import type { WorkItemCard } from './types'

vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./api')>()),
  getWorkItem: vi.fn(),
  updateWorkItem: vi.fn(),
  writeWorkItem: vi.fn(),
}))

const card = {
  id: 'task-a',
  projectId: 'project-a',
  key: 'AW-7',
  version: 1,
  priority: 'medium',
  status: { id: 'todo', name: 'Todo', semanticCategory: 'unstarted' },
  labels: [],
} as unknown as WorkItemCard

const wrapper = (client: QueryClient) =>
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={client}>{children}</QueryClientProvider>
  }

afterEach(() => {
  resetSaveQueue()
  vi.mocked(updateWorkItem).mockReset()
  vi.mocked(writeWorkItem).mockReset()
  vi.mocked(getWorkItem).mockReset()
})

describe('undoing a picked change', () => {
  it('can be undone before its save answers, and never undoes a save that failed', async () => {
    const client = new QueryClient()
    let refuse!: (error: unknown) => void
    vi.mocked(writeWorkItem).mockReturnValueOnce(new Promise((_, reject) => (refuse = reject)))
    // Another editor already set High, so the task reads as if this change saved.
    vi.mocked(getWorkItem).mockResolvedValue({ ...card, priority: 'high', version: 2 } as never)
    const { result } = renderHook(() => useApplyPick(), { wrapper: wrapper(client) })
    const pick = { field: 'priority', priority: 'high' } as unknown as PickResult
    let saving!: Promise<unknown>
    act(() => {
      saving = result.current.mutateAsync({ projectId: 'project-a', result: pick, targets: [card] }).catch(() => undefined)
    })
    await vi.waitFor(() => expect(writeWorkItem).toHaveBeenCalledTimes(1))
    // ⌘Z before the answer: kept already.
    const undoing = undoLast(client)
    refuse(new ApiError({ status: 409, code: 'conflict', message: 'Changed since.' }))
    await expect(undoing).rejects.toThrow('didn’t save')
    await saving
    // Only the change itself was sent: no inverse over the other editor's High.
    expect(writeWorkItem).toHaveBeenCalledTimes(1)
    expect(updateWorkItem).not.toHaveBeenCalled()
  })
})
