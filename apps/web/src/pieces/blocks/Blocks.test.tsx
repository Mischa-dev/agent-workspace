import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { axeViolations, renderPiece, type Width } from '../testing'
import { formFields, formValues, textRuns, type FormBlock, type PanelBlock } from './format'
import { PanelBlocks } from './PanelBlocks'

afterEach(cleanup)

const form: FormBlock = {
  type: 'form',
  id: 'deploy',
  schema: {
    type: 'object',
    properties: {
      target: { type: 'string', title: 'Target', enum: ['staging', 'production'] },
      replicas: { type: 'integer', title: 'Replicas', default: 2 },
      note: { type: 'string', title: 'Note' },
      notify: { type: 'boolean', title: 'Tell the channel' },
    },
    required: ['target'],
  },
  ui: { order: ['target', 'replicas'], widgets: { note: 'textarea' } },
  submit: { label: 'Deploy', action: { id: 'deploy' } },
}

/** One of every block, as the plan's sample panel has them. */
const EVERY: PanelBlock[] = [
  { type: 'text', text: 'Builds for **main**, see [the docs](https://example.com/docs) or [here](/home).', style: 'body' },
  { type: 'text', text: 'Recent', style: 'heading' },
  { type: 'text', text: 'The runner is slow today.', style: 'callout', tone: 'warning' },
  { type: 'status', state: 'working', label: 'Build 214 is running', detail: 'Step 3 of 5', progress: 0.6 },
  {
    type: 'fields',
    items: [
      { label: 'Branch', value: 'main' },
      { label: 'Started', value: new Date().toISOString(), kind: 'time' },
      { label: 'Result', value: { text: 'running', tone: 'agent' }, kind: 'badge' },
      { label: 'Task', value: { kind: 'task', id: 't1', label: 'AWD-12' }, kind: 'thing' },
    ],
  },
  {
    type: 'table',
    columns: [
      { id: 'build', label: 'Build' },
      { id: 'status', label: 'Status', kind: 'badge' },
    ],
    rows: [
      { id: '214', cells: { build: '214', status: { text: 'running', tone: 'agent' } } },
      { id: '213', cells: { build: '213', status: 'passed' } },
    ],
    rowAction: { id: 'open_build' },
  },
  {
    type: 'list',
    items: [
      { id: 'a', title: 'Deploy web', subtitle: 'Yesterday', status: { text: 'Done', tone: 'success' }, action: { id: 'open' } },
      { id: 'b', title: 'Deploy server', meta: 'Waiting on review' },
    ],
    more: { action: { id: 'more' } },
  },
  { type: 'chart', kind: 'number', value: 1280, unit: 'builds' },
  { type: 'buttons', items: [{ label: 'Rerun', action: { id: 'rerun' }, style: 'primary', confirm: { title: 'Rerun build 214?' } }] },
  { type: 'section', title: 'Settings', blocks: [form] },
  { type: 'map', center: [0, 0] } as PanelBlock,
]

describe('PanelBlocks', () => {
  it.each<Width>(['wide', 'narrow'])('draws every block type at %s width, with no accessibility problems', async (width) => {
    const { container } = renderPiece(<PanelBlocks blocks={EVERY} label="Builds" onAction={() => undefined} />, { width })
    const panel = screen.getByRole('region', { name: 'Builds' })
    for (const type of ['text', 'status', 'fields', 'table', 'list', 'chart', 'buttons', 'section', 'form']) {
      expect(panel.querySelector(`[data-part="panel-${type}"]`), type).not.toBeNull()
    }
    expect(screen.getByText('Build 214 is running')).toBeInTheDocument()
    expect(screen.getByText('1,280')).toBeInTheDocument()
    // A block from a newer format is a note, not a failure.
    expect(screen.getByText('Part of this panel needs a newer Workspace.')).toBeInTheDocument()
    expect(await axeViolations(container)).toEqual([])
  })

  it('sends what was asked: a confirmed button, a row, a list item and a form with typed values', async () => {
    const onAction = vi.fn()
    renderPiece(<PanelBlocks blocks={EVERY} onAction={onAction} />)
    fireEvent.click(screen.getByRole('button', { name: 'Rerun' }))
    const dialog = await screen.findByRole('dialog', { name: 'Rerun build 214?' })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Rerun' }))
    await waitFor(() => expect(onAction).toHaveBeenCalledWith({ id: 'rerun' }))
    fireEvent.click(screen.getByRole('button', { name: '214' }))
    expect(onAction).toHaveBeenCalledWith({ id: 'open_build', values: { row: '214' } })
    fireEvent.click(screen.getByRole('button', { name: 'Deploy web' }))
    expect(onAction).toHaveBeenCalledWith({ id: 'open', values: { row: 'a' } })
    fireEvent.change(screen.getByLabelText(/Target/), { target: { value: 'staging' } })
    fireEvent.click(screen.getByRole('button', { name: 'Deploy' }))
    await waitFor(() => expect(onAction).toHaveBeenCalledWith({ id: 'deploy', values: expect.objectContaining({ target: 'staging', replicas: 2 }) }))
  })

  it('shows the module’s field errors and keeps links to https and the app only', () => {
    renderPiece(
      <PanelBlocks
        blocks={[form, { type: 'text', text: 'Bad [link](javascript:alert(1)) and [good](https://example.com)' }]}
        errors={{ target: 'Production is frozen' }}
        onAction={() => undefined}
      />,
    )
    expect(screen.getByText(/Production is frozen/)).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'link' })).toBeNull()
    expect(screen.getByRole('link', { name: 'good' })).toHaveAttribute('href', 'https://example.com')
  })
})

describe('the block format', () => {
  it('reads inline text without HTML', () => {
    expect(textRuns('a **b** *c* `d` [e](/f) <b>g</b>')).toEqual([
      { kind: 'text', text: 'a ' },
      { kind: 'bold', text: 'b' },
      { kind: 'text', text: ' ' },
      { kind: 'italic', text: 'c' },
      { kind: 'text', text: ' ' },
      { kind: 'code', text: 'd' },
      { kind: 'text', text: ' ' },
      { kind: 'link', text: 'e', href: '/f' },
      { kind: 'text', text: ' <b>g</b>' },
    ])
  })

  it('turns a form schema into fields in the asked order, and numbers back into numbers', () => {
    expect(formFields(form).map((field) => [field.id, field.type, field.required])).toEqual([
      ['target', 'single_choice', true],
      ['replicas', 'number', false],
      ['note', 'long_text', false],
      ['notify', 'checkbox', false],
    ])
    expect(formValues(form, { replicas: '3', note: 'hi', notify: true })).toEqual({ replicas: 3, note: 'hi', notify: true })
  })
})
