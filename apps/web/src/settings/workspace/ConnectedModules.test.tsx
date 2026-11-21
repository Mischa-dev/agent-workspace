import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { renderPiece } from '../../pieces/testing'
import { ConnectedModules } from './ConnectedModules'

const callOp = vi.fn()
vi.mock('../../api/ops', async (original) => ({
  ...(await original<typeof import('../../api/ops')>()),
  callOp: (name: string, input: unknown, options: unknown) => callOp(name, input, options),
}))
vi.mock('../../experience/api', () => ({ refreshLayouts: () => Promise.resolve() }))

const manifest = {
  name: 'Helpdesk',
  description: 'Tickets from the support inbox.',
  publisher: 'Acme',
  version: '1.2.0',
  digest: 'd1',
  key: 'helpdesk',
  accountName: 'Helpdesk',
  kinds: ['ticket'],
  operations: [{ name: 'ticket_open', verb: 'open' }],
  events: ['task.created'],
  panels: [],
  frameOrigins: ['https://helpdesk.example'],
  scopes: [
    { scope: 'tasks.view', label: 'See tasks', why: 'To link tickets.', required: true, known: true, approved: true },
    { scope: 'tasks.edit', label: 'Change tasks', why: 'To close them.', required: false, known: true, approved: true },
    { scope: 'mystery', label: 'Not something this server has', why: 'Later.', required: false, known: false, approved: false },
  ],
}

const installed = {
  ...manifest,
  moduleId: 'cm1',
  state: 'active',
  accountId: 'a1',
  manifestUrl: 'https://helpdesk.example/manifest.json',
  baseUrl: 'https://helpdesk.example',
  approvedBy: 'u1',
  approvedAt: '2026-11-21T00:00:00Z',
  pending: null,
  fileCheck: null,
}

afterEach(() => {
  cleanup()
  callOp.mockReset()
})

describe('ConnectedModules', () => {
  it('shows what an app asks for before connecting it, and connects it with the scopes chosen', async () => {
    let connected = false
    callOp.mockImplementation(async (name: string, _input: unknown, options: { preview?: boolean }) => {
      switch (name) {
        case 'connected_module_list':
          return { modules: connected ? [installed] : [] }
        case 'connected_module_install':
          if (!options.preview) connected = true
          return installed
        default:
          return null
      }
    })
    renderPiece(<ConnectedModules workspaceId="w1" />)
    fireEvent.click(await screen.findByRole('button', { name: 'Connect a module' }))
    fireEvent.change(screen.getByLabelText('Manifest address'), { target: { value: ' https://helpdesk.example/manifest.json ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Look it up' }))

    await screen.findByText('Connect Helpdesk?')
    expect(callOp).toHaveBeenCalledWith('connected_module_install', { url: 'https://helpdesk.example/manifest.json' }, { workspaceId: 'w1', preview: true })
    // Required: on and fixed. Not on this server: off and fixed.
    expect(screen.getByLabelText('See tasks')).toBeDisabled()
    expect(screen.getByLabelText('See tasks')).toBeChecked()
    expect(screen.getByLabelText('mystery')).toBeDisabled()
    expect(screen.getByLabelText('mystery')).not.toBeChecked()
    fireEvent.click(screen.getByLabelText('Change tasks'))
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }))

    await waitFor(() =>
      expect(callOp).toHaveBeenCalledWith(
        'connected_module_install',
        { url: 'https://helpdesk.example/manifest.json', digest: 'd1', scopes: ['tasks.view'] },
        { workspaceId: 'w1' },
      ),
    )
    expect(await screen.findByText('Helpdesk')).toBeInTheDocument()
  })

  it('offers a review where an app’s new manifest asks for more', async () => {
    callOp.mockImplementation(async (name: string) =>
      name === 'connected_module_list'
        ? {
            modules: [
              {
                ...installed,
                pending: { ...manifest, version: '2.0.0', digest: 'd2', scopes: [...manifest.scopes, { scope: 'pages.edit', label: 'Change pages', why: 'Notes.', required: false, known: true, approved: false }] },
              },
            ],
          }
        : null,
    )
    renderPiece(<ConnectedModules workspaceId="w1" />)
    fireEvent.click(await screen.findByRole('button', { name: 'Review changes' }))
    expect(await screen.findByText(/New: Change pages/)).toBeInTheDocument()
    expect(screen.getByLabelText('Change pages')).toBeChecked()
  })
})
