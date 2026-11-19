import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { FormPiece } from './FormPiece'
import { firstMissing, type FormField } from './formFields'
import { axeViolations, renderPiece, type Width } from './testing'

const fields: FormField[] = [
  { id: 'title', label: 'Summary', type: 'text', required: true, maxLength: 500 },
  { id: 'severity', label: 'Severity', type: 'single_choice', required: true, hint: 'How bad it is', options: [{ value: 'hi', label: 'High' }, { value: 'lo', label: 'Low' }] },
  { id: 'areas', label: 'Areas', type: 'multiple_choice', hint: 'Pick any', options: [{ value: 'ui', label: 'Screens' }, { value: 'api', label: 'Server' }] },
  { id: 'build', label: 'Build', type: 'number' },
  { id: 'notes', label: 'Notes', type: 'long_text' },
  { id: 'urgent', label: 'Urgent', type: 'checkbox', required: true },
]

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('FormPiece', () => {
  it.each<Width>(['wide', 'narrow'])('asks for each field, with hints, and passes axe (%s)', async (width) => {
    const { container } = renderPiece(<FormPiece fields={fields} label="Report a bug" onSubmit={vi.fn()} submitLabel="Send" />, { width })
    expect(screen.getByRole('form', { name: 'Report a bug' })).toBeVisible()
    expect(screen.getByLabelText('Summary (required)')).toHaveAttribute('maxlength', '500')
    expect(screen.getByLabelText('Severity (required)')).toHaveAccessibleDescription('How bad it is')
    expect(screen.getByRole('group', { name: 'Areas' })).toHaveAccessibleDescription('Pick any')
    expect(screen.getByLabelText('Build')).toHaveAttribute('type', 'number')
    expect(await axeViolations(container)).toEqual([])
  })

  it('names the first required field that is still empty, and sends nothing', () => {
    const onSubmit = vi.fn()
    renderPiece(<FormPiece fields={fields} label="Report a bug" onSubmit={onSubmit} submitLabel="Send" />)
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(screen.getByText('Fill in Summary.')).toBeVisible()
    fireEvent.change(screen.getByLabelText('Summary (required)'), { target: { value: 'Crash on save' } })
    expect(screen.queryByText('Fill in Summary.')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    // An unticked checkbox is an answer, so Urgent never blocks.
    expect(screen.getByText('Fill in Severity.')).toBeVisible()
    expect(onSubmit).not.toHaveBeenCalled()
  })

  it('sends the values through its operation and starts over once it lands', async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    renderPiece(<FormPiece fields={fields} label="Report a bug" onSubmit={onSubmit} submitLabel="Send" />)
    fireEvent.change(screen.getByLabelText('Summary (required)'), { target: { value: 'Crash on save' } })
    fireEvent.change(screen.getByLabelText('Severity (required)'), { target: { value: 'hi' } })
    fireEvent.click(screen.getByRole('checkbox', { name: 'Server' }))
    fireEvent.change(screen.getByLabelText('Build'), { target: { value: '43' } })
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })))
    expect(onSubmit).toHaveBeenCalledWith({ title: 'Crash on save', severity: 'hi', areas: ['api'], build: '43' })
    await waitFor(() => expect(screen.getByLabelText('Summary (required)')).toHaveValue(''))
  })

  it('shows why a send failed and keeps what was typed', async () => {
    const onSubmit = vi.fn().mockRejectedValue(new Error('The form is closed.'))
    renderPiece(<FormPiece fields={fields.slice(0, 1)} label="Report a bug" onSubmit={onSubmit} submitLabel="Send" />)
    fireEvent.change(screen.getByLabelText('Summary (required)'), { target: { value: 'Crash on save' } })
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })))
    expect(await screen.findByText('The form is closed.')).toBeVisible()
    expect(screen.getByLabelText('Summary (required)')).toHaveValue('Crash on save')
  })
})

describe('firstMissing', () => {
  it('reads blanks, spaces and empty picks as missing', () => {
    expect(firstMissing(fields, { title: '  ' })).toBe('Fill in Summary.')
    expect(firstMissing(fields, { title: 'x', severity: 'hi' })).toBeNull()
    expect(firstMissing([{ id: 'a', label: 'Areas', type: 'multiple_choice', required: true }], { a: [] })).toBe('Fill in Areas.')
  })
})
