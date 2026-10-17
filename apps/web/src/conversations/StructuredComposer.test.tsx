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

const workItem = {
  id: '019fb8b0-0000-7000-8000-000000000003',
  key: 'AW-42',
  title: 'Structured reference proof',
  statusCategory: 'started',
  archivedAt: null,
  href: '/projects/project-a/work-items/019fb8b0-0000-7000-8000-000000000003',
}

afterEach(cleanup)

function type(value: string) {
  const box = screen.getByLabelText('Message') as HTMLTextAreaElement
  fireEvent.change(box, { target: { value } })
  return box
}

describe('StructuredComposer', () => {
  it('sends a message that is only a file once its upload finishes', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    let finish: (value: unknown) => void = () => undefined
    const upload = vi.fn(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { container } = render(<StructuredComposer onSubmit={onSubmit} searchActors={vi.fn()} upload={upload as never} />)
    const input = container.querySelector('input[type="file"]') as HTMLInputElement
    fireEvent.change(input, { target: { files: [new File(['hi'], 'notes.txt', { type: 'text/plain' })] } })
    expect(await screen.findByText('notes.txt')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled()

    const file = {
      id: '019fb8b0-0000-7000-8000-0000000000f1',
      name: 'notes.txt',
      contentType: 'text/plain',
      size: 2,
      width: null,
      height: null,
      commentId: null,
      uploadedBy: human.id,
      createdAt: '',
    }
    finish(file)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].attachments).toEqual([file])
    expect(onSubmit.mock.calls[0][0].content).toEqual({ version: 1, nodes: [{ type: 'text', text: '' }] })
    await waitFor(() => expect(screen.queryByText('notes.txt')).not.toBeInTheDocument())
  })

  it('formats the selection with Slack-style shortcuts', () => {
    render(<StructuredComposer onSubmit={vi.fn()} searchActors={vi.fn()} />)
    const box = type('make this bold')
    box.setSelectionRange(5, 9)
    fireEvent.keyDown(box, { key: 'b', ctrlKey: true })
    expect(box).toHaveValue('make **this** bold')
  })

  it('retains the same idempotency key and draft across a retry', async () => {
    const onSubmit = vi.fn().mockRejectedValueOnce(new Error('Temporary failure')).mockResolvedValueOnce(undefined)
    render(<StructuredComposer onSubmit={onSubmit} searchActors={vi.fn()} />)

    type('Hello Project')
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Temporary failure')
    expect(screen.getByLabelText('Message')).toHaveValue('Hello Project')

    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2))
    expect(onSubmit.mock.calls[0][0].idempotencyKey).toBe(onSubmit.mock.calls[1][0].idempotencyKey)
    await waitFor(() => expect(screen.getByLabelText('Message')).toHaveValue(''))
  })

  it('keeps the draft through a reload and sends what failed offline once the connection is back', async () => {
    const onSubmit = vi.fn().mockRejectedValueOnce(new TypeError('Failed to fetch')).mockResolvedValue(undefined)
    const first = render(<StructuredComposer draftKey="channel-1" onSubmit={onSubmit} searchActors={vi.fn()} />)
    type('Half written')
    first.unmount()

    render(<StructuredComposer draftKey="channel-1" onSubmit={onSubmit} searchActors={vi.fn()} />)
    expect(screen.getByLabelText('Message')).toHaveValue('Half written')
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    expect(await screen.findByText('Half written', { selector: '.aw-composer__queued-text' })).toBeInTheDocument()
    expect(screen.getByLabelText('Message')).toHaveValue('')

    window.dispatchEvent(new Event('online'))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2))
    expect(onSubmit.mock.calls[1][0].idempotencyKey).toBe(onSubmit.mock.calls[0][0].idempotencyKey)
    await waitFor(() => expect(screen.queryByRole('list', { name: 'Waiting to send' })).not.toBeInTheDocument())
    expect(window.localStorage.length).toBe(0)
  })

  it('picks a mention inline with the keyboard and submits an Actor ID node where it was typed', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    const searchActors = vi.fn().mockResolvedValue([human, worker])
    render(<StructuredComposer onSubmit={onSubmit} searchActors={searchActors} />)

    const box = type('Please review @ag')
    await screen.findByRole('option', { name: /Mischa Nelson/ })
    expect(searchActors).toHaveBeenLastCalledWith('ag')
    fireEvent.keyDown(box, { key: 'ArrowDown' })
    fireEvent.keyDown(box, { key: 'Enter' })

    expect(box).toHaveValue('Please review @agent ')
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull())
    fireEvent.keyDown(box, { key: 'Enter' })
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].content.nodes).toEqual([
      { type: 'text', text: 'Please review ' },
      { type: 'mention', actorId: worker.id },
    ])
  })

  it('keeps literal @agent text as text when no mention is picked', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    render(<StructuredComposer onSubmit={onSubmit} searchActors={vi.fn().mockResolvedValue([])} />)

    type('literal @agent text')
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].content.nodes).toEqual([{ type: 'text', text: 'literal @agent text' }])
  })

  it('references a work item with # and submits only its canonical ID', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    render(
      <StructuredComposer onSubmit={onSubmit} searchActors={vi.fn()} searchWorkItems={vi.fn().mockResolvedValue([workItem])} />,
    )
    const box = type('Track #AW')
    await screen.findByRole('option', { name: /AW-42/ })
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(box).toHaveValue('Track AW-42 ')
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].content.nodes).toEqual([
      { type: 'text', text: 'Track ' },
      { type: 'work_item_ref', workItemId: workItem.id },
    ])
  })

  it('counts Unicode scalars explicitly and preserves an over-limit draft', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    render(<StructuredComposer onSubmit={onSubmit} searchActors={vi.fn()} />)

    const message = screen.getByLabelText('Message')
    expect(message).not.toHaveAttribute('maxlength')
    const overLimit = '🙂'.repeat(20_001)
    type(overLimit)

    expect(message).toHaveValue(overLimit)
    expect(screen.getByRole('alert')).toHaveTextContent('Shorten this by 1 character to send it.')
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled()
    expect(onSubmit).not.toHaveBeenCalled()

    const exactLimit = '🙂'.repeat(20_000)
    type(exactLimit)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.getByText('characters left')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1))
    expect(onSubmit.mock.calls[0][0].content.nodes).toEqual([{ type: 'text', text: exactLimit }])
  })

  it('opens the mention list from its button and closes it with Escape, keeping focus in the box', async () => {
    render(<StructuredComposer onSubmit={vi.fn()} searchActors={vi.fn().mockResolvedValue([])} />)
    fireEvent.click(screen.getByRole('button', { name: 'Mention someone' }))
    const list = await screen.findByRole('listbox', { name: 'People and agents' })
    expect(list).toBeInTheDocument()
    const box = screen.getByLabelText('Message')
    expect(box).toHaveValue('@')
    await waitFor(() => expect(box).toHaveFocus())
    fireEvent.keyDown(box, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull())
    expect(box).toHaveFocus()
  })
})
