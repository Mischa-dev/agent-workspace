import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { HostActions } from './HostActions'

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } }),
  )
}

function renderActions() {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <HostActions actions={[{ id: 'tune', label: 'Tune', needs: 'panel' }]} hostId="h1" workspaceId="w1" />
    </QueryClientProvider>,
  )
}

describe('HostActions', () => {
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('opens the module panel a host’s answer names instead of its message', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input)
        if (path.includes('/ops/agent_action_run')) {
          return jsonResponse({ outcome: 'ok', value: { commandId: 'c1', state: 'pending' } })
        }
        if (path.includes('/ops/agent_action_get')) {
          return jsonResponse({
            outcome: 'ok',
            value: { commandId: 'c1', state: 'acked', reply: { message: 'Opened.' }, panel: { moduleId: 'm1', panel: 'tune' } },
          })
        }
        if (path.includes('/ops/module_panel_open')) {
          const panel = { format: 'aw-panel/1', blocks: [{ type: 'text', text: 'Pick a model.' }] }
          return jsonResponse({ outcome: 'ok', value: { moduleId: 'm1', panel: 'tune', answer: { kind: 'panel', panel } } })
        }
        return jsonResponse({ outcome: 'ok', value: {} })
      }),
    )
    renderActions()
    fireEvent.click(screen.getByRole('button', { name: 'Tune' }))
    expect(await screen.findByRole('dialog', { name: 'Tune' })).toBeVisible()
    expect(await screen.findByText('Pick a model.')).toBeVisible()
    expect(screen.queryByText('Opened.')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /close/i }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })
})
