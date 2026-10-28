import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useRef } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { useListKeys } from './listKeys'

function Results({ onEscape, onMove }: { onEscape?: () => boolean; onMove?: (element: HTMLElement) => void }) {
  const listRef = useRef<HTMLUListElement>(null)
  useListKeys(listRef, 'a', { onEscape, onMove })
  return (
    <>
      <input aria-label="Search" />
      <button type="button">Elsewhere</button>
      <ul ref={listRef}>
        {['One', 'Two', 'Three'].map((name) => (
          <li key={name}>
            <a href={`#${name}`}>{name}</a>
          </li>
        ))}
      </ul>
    </>
  )
}

afterEach(cleanup)

describe('arrow keys through a list', () => {
  it('moves focus down, up and to the ends, starting from the page', () => {
    const onMove = vi.fn()
    render(<Results onMove={onMove} />)
    fireEvent.keyDown(document.body, { key: 'ArrowDown' })
    expect(screen.getByRole('link', { name: 'One' })).toHaveFocus()
    fireEvent.keyDown(document.activeElement!, { key: 'j' })
    expect(screen.getByRole('link', { name: 'Two' })).toHaveFocus()
    fireEvent.keyDown(document.activeElement!, { key: 'End' })
    expect(screen.getByRole('link', { name: 'Three' })).toHaveFocus()
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
    expect(screen.getByRole('link', { name: 'Three' })).toHaveFocus()
    fireEvent.keyDown(document.activeElement!, { key: 'Home' })
    expect(screen.getByRole('link', { name: 'One' })).toHaveFocus()
    expect(onMove.mock.calls.map(([element]) => (element as HTMLElement).textContent)).toEqual(['One', 'Two', 'Three', 'Three', 'One'])
  })

  it('never takes keys typed into a field, and leaves the arrows to other controls', () => {
    const onMove = vi.fn()
    render(<Results onMove={onMove} />)
    const field = screen.getByRole('textbox', { name: 'Search' })
    field.focus()
    for (const key of ['j', 'k', 'ArrowDown', 'Home', 'End']) fireEvent.keyDown(field, { key })
    expect(field).toHaveFocus()
    const button = screen.getByRole('button', { name: 'Elsewhere' })
    button.focus()
    fireEvent.keyDown(button, { key: 'ArrowDown' })
    expect(button).toHaveFocus()
    // J still works from a control that isn't a field.
    fireEvent.keyDown(button, { key: 'j' })
    expect(screen.getByRole('link', { name: 'One' })).toHaveFocus()
    expect(onMove).toHaveBeenCalledTimes(1)
  })

  it('hands Escape to the page, and keeps it when nothing closed', () => {
    const onEscape = vi.fn().mockReturnValueOnce(true).mockReturnValueOnce(false)
    render(<Results onEscape={onEscape} />)
    expect(fireEvent.keyDown(document.body, { key: 'Escape' })).toBe(false)
    expect(fireEvent.keyDown(document.body, { key: 'Escape' })).toBe(true)
    expect(onEscape).toHaveBeenCalledTimes(2)
  })
})
