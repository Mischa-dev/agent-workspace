import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { StructuredComposer } from './StructuredComposer'

const human = {
  id: '019fb8b0-0000-7000-8000-000000000001',
  kind: 'human' as const,
  displayName: 'Mischa Nelson',
  handle: 'mischa',
  avatarUrl: null,
}

const worker = {
  id: '019fb8b0-0000-7000-8000-000000000002',
  kind: 'execution_worker' as const,
  displayName: 'Agent',
  handle: 'agent',
  avatarUrl: null,
}

afterEach(cleanup)

describe('StructuredComposer', () => {
  it('retains the same idempotency key and draft across a retry', async () => {
    const onSubmit = vi
      .fn()
      .mockRejectedValueOnce(new Error('Temporary failure'))
      .mockResolvedValueOnce(undefined)
    render(
      <StructuredComposer onSubmit={onSubmit} searchActors={vi.fn()} />,
    )

    fireEvent.change(screen.getByLabelText('Message'), {
      target: { value: 'Hello Project' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send Message' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Temporary failure')
    expect(screen.getByLabelText('Message')).toHaveValue('Hello Project')

    fireEvent.click(screen.getByRole('button', { name: 'Send Message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2))
    expect(onSubmit.mock.calls[0][0].idempotencyKey).toBe(
      onSubmit.mock.calls[1][0].idempotencyKey,
    )
    await waitFor(() => expect(screen.getByLabelText('Message')).toHaveValue(''))
  })

  it('selects a Worker mention by keyboard and submits an Actor ID node', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    const searchActors = vi.fn().mockResolvedValue([human, worker])
    render(
      <StructuredComposer onSubmit={onSubmit} searchActors={searchActors} />,
    )

    fireEvent.change(screen.getByLabelText('Message'), {
      target: { value: 'Please review' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Mention Actor' }))
    const search = await screen.findByRole('combobox')
    await screen.findByRole('option', { name: /Mischa Nelson/ })
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    fireEvent.keyDown(search, { key: 'Enter' })

    expect(screen.getByLabelText('Selected mentions')).toHaveTextContent('@agent')
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Mention Actor' })).toHaveFocus(),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Send Message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].content.nodes).toEqual([
      { type: 'text', text: 'Please review' },
      { type: 'text', text: ' ' },
      { type: 'mention', actorId: worker.id },
    ])
  })

  it('keeps literal @agent text as text when no semantic mention is selected', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    render(
      <StructuredComposer onSubmit={onSubmit} searchActors={vi.fn()} />,
    )

    fireEvent.change(screen.getByLabelText('Message'), {
      target: { value: 'literal @agent text' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send Message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].content.nodes).toEqual([
      { type: 'text', text: 'literal @agent text' },
    ])
  })

  it('counts Unicode scalars explicitly and preserves an over-limit draft', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    render(
      <StructuredComposer onSubmit={onSubmit} searchActors={vi.fn()} />,
    )

    const message = screen.getByLabelText('Message')
    expect(message).not.toHaveAttribute('maxlength')
    const overLimit = '🙂'.repeat(20_001)
    fireEvent.change(message, { target: { value: overLimit } })

    expect(message).toHaveValue(overLimit)
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Shorten this Message by 1 Unicode character.',
    )
    expect(screen.getByRole('button', { name: 'Send Message' })).toBeDisabled()
    expect(onSubmit).not.toHaveBeenCalled()

    const exactLimit = '🙂'.repeat(20_000)
    fireEvent.change(message, { target: { value: exactLimit } })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.getByText('20,000 / 20,000 Unicode characters')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Send Message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].content.nodes).toEqual([
      { type: 'text', text: exactLimit },
    ])
  })

  it('dismisses the picker with Escape and restores trigger focus', async () => {
    render(
      <StructuredComposer
        onSubmit={vi.fn()}
        searchActors={vi.fn().mockResolvedValue([])}
      />,
    )
    const trigger = screen.getByRole('button', { name: 'Mention Actor' })
    fireEvent.click(trigger)
    const search = await screen.findByRole('combobox')
    fireEvent.keyDown(search, { key: 'Escape' })

    await waitFor(() => expect(screen.queryByRole('combobox')).toBeNull())
    expect(trigger).toHaveFocus()
  })
})
