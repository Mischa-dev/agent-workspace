import { fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it } from 'vitest'

import { MarkdownEditor } from './MarkdownEditor'

function Harness({ initial }: { initial: string }) {
  const [value, setValue] = useState(initial)
  return (
    <MemoryRouter>
      <MarkdownEditor label="Description" onChange={setValue} value={value} />
    </MemoryRouter>
  )
}

describe('MarkdownEditor', () => {
  it('formats the selection from the toolbar and shortcuts, and previews', () => {
    render(<Harness initial="ship it" />)
    const field = screen.getByRole('textbox', { name: 'Description' }) as HTMLTextAreaElement

    field.setSelectionRange(0, 4)
    fireEvent.click(screen.getByRole('button', { name: 'Bold' }))
    expect(field.value).toBe('**ship** it')
    expect(field.value.slice(field.selectionStart, field.selectionEnd)).toBe('ship')

    fireEvent.keyDown(field, { key: 'b', ctrlKey: true })
    expect(field.value).toBe('ship it')

    fireEvent.click(screen.getByRole('button', { name: 'Checklist' }))
    expect(field.value).toBe('- [ ] ship it')

    fireEvent.click(screen.getByRole('radio', { name: 'Preview' }))
    expect(screen.getByRole('checkbox', { name: 'Not done' })).toBeDisabled()
    expect(screen.queryByRole('toolbar')).toBeNull()
  })
})
