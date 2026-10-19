import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, useLocation, useNavigate } from 'react-router-dom'

import { withFilter } from './listModel'
import { SEARCH_DELAY, useDebouncedSearch, useListState } from './useListState'

function Search() {
  const { state, update } = useListState()
  const [text, change] = useDebouncedSearch(state.filters.q ?? '', update)
  const location = useLocation()
  const navigate = useNavigate()
  return (
    <>
      <input aria-label="Search" onChange={(event) => change(event.target.value)} value={text} />
      <output aria-label="Location">{location.search}</output>
      <button onClick={() => navigate(-1)} type="button">
        Back
      </button>
    </>
  )
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('search box', () => {
  it('writes the URL once typing pauses, replacing the entry so Back skips the keystrokes', () => {
    render(
      <MemoryRouter initialEntries={['/list', '/list?priority=high']} initialIndex={1}>
        <Search />
      </MemoryRouter>,
    )
    const input = screen.getByRole('textbox', { name: 'Search' })
    for (const value of ['l', 'lo', 'log', 'login']) fireEvent.change(input, { target: { value } })
    expect(input).toHaveValue('login')
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent('?priority=high')

    act(() => vi.advanceTimersByTime(SEARCH_DELAY))
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent('?q=login&priority=high')

    fireEvent.change(input, { target: { value: 'logout' } })
    act(() => vi.advanceTimersByTime(SEARCH_DELAY))
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent('?q=logout&priority=high')

    // One Back leaves the search page entirely: no entry per keystroke or per pause.
    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    expect(screen.getByRole('status', { name: 'Location' })).toBeEmptyDOMElement()
    expect(input).toHaveValue('')
  })

  it('drops unsent typing when the URL changes from elsewhere', () => {
    render(
      <MemoryRouter initialEntries={['/list', '/list?q=old']} initialIndex={1}>
        <Search />
      </MemoryRouter>,
    )
    const input = screen.getByRole('textbox', { name: 'Search' })
    expect(input).toHaveValue('old')
    fireEvent.change(input, { target: { value: 'older' } })
    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    expect(input).toHaveValue('')
    act(() => vi.advanceTimersByTime(SEARCH_DELAY))
    expect(screen.getByRole('status', { name: 'Location' })).toBeEmptyDOMElement()
  })
})

describe('list state', () => {
  it('keeps both of two updates made before the page catches up', () => {
    function TwoPicks() {
      const { update } = useListState()
      const location = useLocation()
      return (
        <>
          <button
            onClick={() => {
              update((current) => withFilter(current, 'priority', 'high'))
              update((current) => withFilter(current, 'priority', [current.filters.priority, 'urgent'].join(',')))
            }}
            type="button"
          >
            Pick two
          </button>
          <output aria-label="Location">{location.search}</output>
        </>
      )
    }
    render(
      <MemoryRouter initialEntries={['/list?blocked=true']}>
        <TwoPicks />
      </MemoryRouter>,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Pick two' }))
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent('?priority=high%2Curgent&blocked=true')
  })
})
