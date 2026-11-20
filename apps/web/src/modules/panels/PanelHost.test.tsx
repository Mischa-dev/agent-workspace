import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import sample from '../../../../../modules/panels/sample.json'
import { axeViolations, renderPiece, type Width } from '../../pieces/testing'
import { PanelHost } from './PanelHost'

const callOp = vi.fn()
vi.mock('../../api/ops', async (original) => ({
  ...(await original<typeof import('../../api/ops')>()),
  callOp: (name: string, input: unknown, options: unknown) => callOp(name, input, options),
}))

const place = { moduleId: 'm1', panel: 'builds', spaceId: 's1' }
const opened = (panel: unknown) => ({ moduleId: 'm1', panel: 'builds', answer: { kind: 'panel', panel } })

afterEach(() => {
  cleanup()
  callOp.mockReset()
  vi.unstubAllGlobals()
})

describe('PanelHost', () => {
  it.each<Width>(['wide', 'narrow'])('draws every block of the shared sample, and passes axe (%s)', async (width) => {
    callOp.mockResolvedValue(opened(sample))
    const { container } = renderPiece(<PanelHost label="Builds" place={place} />, { width })
    expect(await screen.findByRole('group', { name: 'Builds' })).toBeVisible()
    expect(screen.getByText('Build 214 is running')).toBeVisible()
    expect(screen.getAllByRole('table')).toHaveLength(2)
    expect(screen.getByText('Two tests failed twice this week')).toBeVisible()
    expect(screen.getByLabelText(/Branch/)).toHaveValue('main')
    expect(screen.getByRole('button', { name: 'Start build' })).toBeVisible()
    expect(screen.getByRole('link', { name: 'Build 214' })).toHaveAttribute('href', 'https://builds.example.com/214')
    expect(callOp).toHaveBeenCalledWith('module_panel_open', expect.objectContaining(place), expect.anything())
    expect(await axeViolations(container)).toEqual([])
  })

  it('sends a form with the panel’s state, shows the module’s errors, then its done', async () => {
    const onDone = vi.fn()
    callOp.mockImplementation(async (name: string) => (name === 'module_panel_open' ? opened(sample) : null))
    renderPiece(<PanelHost label="Builds" onDone={onDone} place={place} />)
    await screen.findByRole('button', { name: 'Start build' })

    callOp.mockResolvedValueOnce({ moduleId: 'm1', panel: 'builds', answer: { kind: 'errors', errors: { branch: 'No such branch.' } } })
    fireEvent.click(screen.getByRole('button', { name: 'Start build' }))
    expect(await screen.findByText('No such branch.')).toBeVisible()
    expect(callOp).toHaveBeenLastCalledWith(
      'module_panel_press',
      expect.objectContaining({ state: 'page=1', action: { id: 'start', values: expect.objectContaining({ branch: 'main', target: 'release' }) } }),
      expect.objectContaining({ requestId: expect.any(String) }),
    )

    callOp.mockResolvedValueOnce({ moduleId: 'm1', panel: 'builds', answer: { kind: 'done', message: 'Build started.', result: { build: '215' } } })
    fireEvent.click(screen.getByRole('button', { name: 'Start build' }))
    expect(await screen.findByText('Build started.')).toBeVisible()
    expect(onDone).toHaveBeenCalledWith({ message: 'Build started.', result: { build: '215' } })
  })

  it('sends a choice as the schema’s own value, and asks for a required one first', async () => {
    const form = {
      type: 'form',
      schema: {
        type: 'object',
        required: ['size'],
        properties: { size: { type: 'integer', title: 'Size', enum: [1, 2, 3] } },
      },
      submit: { label: 'Save', action: { id: 'save' } },
    }
    callOp.mockImplementation(async (name: string) => (name === 'module_panel_open' ? opened({ blocks: [form] }) : null))
    renderPiece(<PanelHost label="Sizes" place={place} />)
    const size = await screen.findByRole('combobox', { name: /Size/ })
    expect(size).toHaveValue('')
    expect(screen.getByRole('option', { name: 'Choose…' })).toBeInTheDocument()

    fireEvent.change(size, { target: { value: '1' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(callOp).toHaveBeenLastCalledWith(
        'module_panel_press',
        expect.objectContaining({ action: { id: 'save', values: { size: 2 } } }),
        expect.anything(),
      ),
    )
  })

  it('asks before a button with a confirmation, and draws the next panel it answers', async () => {
    callOp.mockImplementation(async (name: string) => (name === 'module_panel_open' ? opened(sample) : null))
    renderPiece(<PanelHost label="Builds" place={place} />)
    fireEvent.click(await screen.findByRole('button', { name: 'Rerun' }))
    expect(callOp).toHaveBeenCalledTimes(1)
    callOp.mockResolvedValueOnce(
      opened({ format: 'aw-panel/1', blocks: [{ type: 'status', state: 'working', label: 'Build 215 is queued' }, { type: 'map' }] }),
    )
    const dialog = await screen.findByRole('dialog', { name: 'Rerun build 214?' })
    fireEvent.click(dialog.querySelector('.aw-btn--primary') as HTMLElement)
    expect(await screen.findByText('Build 215 is queued')).toBeVisible()
    expect(screen.getByText('Part of this panel needs a newer Workspace.')).toBeVisible()
    expect(callOp).toHaveBeenLastCalledWith(
      'module_panel_press',
      expect.objectContaining({ action: { id: 'rerun', values: { build: '214' } } }),
      expect.anything(),
    )
  })

  it('says so when the app doesn’t answer, and tries again', async () => {
    callOp.mockRejectedValueOnce(new TypeError('network'))
    renderPiece(<PanelHost label="Builds" place={place} />)
    expect(await screen.findByText('The app didn’t answer')).toBeVisible()
    callOp.mockResolvedValueOnce(opened(sample))
    fireEvent.click(screen.getByRole('button', { name: /try again/i }))
    await waitFor(() => expect(screen.getByRole('group', { name: 'Builds' })).toBeVisible())
  })
})
