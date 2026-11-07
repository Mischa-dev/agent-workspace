import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { FieldDefinition } from '../fields/fieldModel'
import { FieldFilterEditor } from './FieldFilters'

vi.mock('./hooks', () => ({ useAssignableActors: () => ({ data: [] }) }))

const risk: FieldDefinition = {
  id: 'risk',
  name: 'Risk',
  description: '',
  type: 'single_choice',
  options: [
    { id: 'low', name: 'Low', color: '#33aa66', archived: false },
    { id: 'high', name: 'High', color: '#cc3333', archived: false },
  ],
  target: null,
  position: 1,
  archived: false,
  version: 1,
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('FieldFilterEditor', () => {
  it('says so and leaves the list alone when the filters would be too long to keep', () => {
    const onApply = vi.fn()
    const onClose = vi.fn()
    const anchor = document.createElement('button')
    document.body.append(anchor)
    render(<FieldFilterEditor anchor={anchor} clause={null} field={risk} fits={() => false} onApply={onApply} onClose={onClose} projectId="project-a" />)

    fireEvent.click(screen.getByRole('checkbox', { name: 'High' }))
    fireEvent.click(screen.getByRole('button', { name: 'Apply' }))
    expect(onApply).not.toHaveBeenCalled()
    expect(onClose).not.toHaveBeenCalled()
    expect(screen.getByText(/Too many field filters or values/)).toBeTruthy()
  })

  it('applies a filter that fits', () => {
    const onApply = vi.fn()
    const anchor = document.createElement('button')
    document.body.append(anchor)
    render(<FieldFilterEditor anchor={anchor} clause={null} field={risk} fits={() => true} onApply={onApply} onClose={vi.fn()} projectId="project-a" />)

    fireEvent.click(screen.getByRole('checkbox', { name: 'High' }))
    fireEvent.click(screen.getByRole('button', { name: 'Apply' }))
    expect(onApply).toHaveBeenCalledWith({ field: 'risk', op: 'is', values: ['high'] })
  })
})
