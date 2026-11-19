import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { HistoryPanel } from './HistoryPanel'

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

/** Answers each operation as `answers` says, and keeps what was sent. */
function serve(answers: (op: string) => unknown) {
  const sent: { op: string; body: unknown }[] = []
  vi.stubGlobal(
    'fetch',
    vi.fn((url: string, init?: RequestInit) => {
      const op = String(url).split('/').pop() ?? ''
      sent.push({ op, body: init?.body ? JSON.parse(String(init.body)) : null })
      return Promise.resolve(
        new Response(JSON.stringify({ outcome: 'ok', value: answers(op) }), { status: 200, headers: { 'content-type': 'application/json' } }),
      )
    }),
  )
  return sent
}

function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>
}

const LIST = {
  kind: 'whiteboard',
  caps: { list: true, showVersion: true, restore: true, diff: false },
  entries: [
    { version: 3, at: '2026-11-17T10:00:00.000Z', actor: 'Ana', actorId: 'a', via: 'Planner', summary: 'Saved 12 elements', changed: ['scene'] },
    { version: 2, at: '2026-11-16T10:00:00.000Z', actor: null, actorId: null, via: null, summary: 'Before a big delete', changed: ['scene'] },
  ],
}

describe('HistoryPanel', () => {
  it('says who and through which app, and restores an older version held to the current one', async () => {
    const onRestored = vi.fn()
    const sent = serve((op) => (op === 'history_list' ? LIST : { kind: 'whiteboard', restored: 2, version: 4 }))
    render(wrap(<HistoryPanel restore={{ expectedVersion: 41, onRestored }} thing="board-1" />))
    expect(await screen.findByText('Saved 12 elements')).toBeVisible()
    expect(screen.getByText('Ana through Planner')).toBeVisible()
    expect(screen.getByText('Someone')).toBeVisible()
    // The newest is what's there now: only older ones can be put back.
    const restore = screen.getAllByRole('button', { name: 'Restore' })
    expect(restore).toHaveLength(1)
    fireEvent.click(restore[0])
    await waitFor(() => expect(onRestored).toHaveBeenCalledWith(2))
    expect(sent.find((call) => call.op === 'history_restore')?.body).toEqual({ thing: 'board-1', version: 2, expectedVersion: 41 })
  })

  it('only reads without restore, or where the kind can’t restore', async () => {
    serve(() => ({ ...LIST, caps: { ...LIST.caps, restore: false } }))
    render(wrap(<HistoryPanel restore={{ expectedVersion: 1 }} thing="board-1" />))
    expect(await screen.findByText('Saved 12 elements')).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Restore' })).toBeNull()
  })
})
