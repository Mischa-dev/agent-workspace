import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it } from 'vitest'

import { ThingChip } from './ThingChip'

afterEach(cleanup)

describe('ThingChip', () => {
  it('links to the thing and describes it on focus, until Escape', () => {
    render(
      <MemoryRouter>
        <ThingChip details="AWD-12" href="/tasks/12" kind="task" label="Write the parser" />
      </MemoryRouter>,
    )
    const link = screen.getByRole('link', { name: 'Write the parser' })
    expect(link).toHaveAttribute('href', '/tasks/12')
    fireEvent.focus(link)
    expect(screen.getByRole('tooltip')).toHaveTextContent('TaskWrite the parserAWD-12')
    expect(link).toHaveAccessibleDescription('TaskWrite the parserAWD-12')
    fireEvent.keyDown(link, { key: 'Escape' })
    expect(screen.queryByRole('tooltip')).toBeNull()
  })

  it('takes no focus without an address, so it can sit in a table cell’s button', () => {
    render(<ThingChip kind="record" label="Brief" />)
    expect(screen.queryByRole('link')).toBeNull()
    expect(screen.getByText('Brief').closest('[tabindex]')).toBeNull()
  })
})
