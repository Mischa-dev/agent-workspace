import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ReplyStatusList } from './ReplyStatus'
import type { ReplyRequest } from './types'

const agent = { id: 'agent-1', kind: 'external_agent', displayName: 'Codex', handle: 'codex', avatarUrl: null }

function request(overrides: Partial<ReplyRequest>): ReplyRequest {
  return {
    id: 'request-1',
    messageId: 'message-1',
    agent,
    required: true,
    status: 'claimed',
    reason: null,
    progress: 'Running the server tests',
    progressAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    agentLastSeenAt: null,
    ...overrides,
  } as ReplyRequest
}

describe('ReplyStatusList', () => {
  afterEach(cleanup)

  it('stops a working agent, and offers to ask again once stopped', async () => {
    const onStop = vi.fn().mockResolvedValue(undefined)
    const onRetry = vi.fn()
    const { rerender } = render(<ReplyStatusList onRetry={onRetry} onStop={onStop} requests={[request({})]} />)
    expect(screen.getByText(/Codex is working · Running the server tests/)).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Stop Codex' }))
    await waitFor(() => expect(onStop).toHaveBeenCalledWith('request-1'))

    rerender(
      <ReplyStatusList
        onRetry={onRetry}
        onStop={onStop}
        requests={[request({ status: 'cancelled', reason: 'Stopped by Mischa.', progress: null })]}
      />,
    )
    expect(screen.getByText('Codex was stopped')).toBeVisible()
    expect(screen.getByText('Stopped by Mischa.')).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Stop Codex' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Ask again' }))
    expect(onRetry).toHaveBeenCalledWith('request-1')
  })

  it('has no Stop for people who can’t post, and says when a follow-up waits', () => {
    render(
      <ReplyStatusList
        onRetry={vi.fn()}
        requests={[request({ id: 'request-2', status: 'pending', required: false, followsRequestId: 'request-1' })]}
      />,
    )
    expect(screen.getByText('Codex will pick this up after what it’s working on')).toBeVisible()
    expect(screen.queryByRole('button', { name: /Stop/ })).not.toBeInTheDocument()
  })
})
