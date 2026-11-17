import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { CalendarPiece } from './CalendarPiece'
import { calendarRange } from './calendarRange'

type Item = { id: string; title: string; day: string | null }

const items: Item[] = [
  { id: 'a', title: 'Kickoff', day: '2026-11-03' },
  { id: 'b', title: 'Review', day: '2026-11-03' },
  { id: 'c', title: 'Launch', day: '2026-11-20' },
  { id: 'd', title: 'Someday', day: null },
]

/** A phone-width screen, as `usePhoneWidth` asks the browser. */
function phone(matches: boolean) {
  vi.stubGlobal('matchMedia', (query: string) => ({ matches, media: query, addEventListener: () => undefined, removeEventListener: () => undefined }))
}

function calendar(overrides: Partial<Parameters<typeof CalendarPiece<Item>>[0]> = {}) {
  const props = {
    dayOf: (item: Item) => item.day,
    href: (item: Item) => `/items/${item.id}`,
    items,
    month: new Date(2026, 10, 1),
    noun: 'record',
    onMonth: vi.fn(),
    title: (item: Item) => item.title,
    ...overrides,
  }
  render(<CalendarPiece {...props} />)
  return props
}

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('CalendarPiece', () => {
  it('puts each item on its day of the month, adds on a day and moves by dragging, when wide', () => {
    phone(false)
    const onAdd = vi.fn()
    const onMoveDay = vi.fn()
    const { onMonth } = calendar({ onAdd, onMoveDay })
    expect(screen.getByRole('heading', { name: /November 2026/ })).toBeVisible()
    const third = screen.getByRole('group', { name: /November 3$/ })
    expect(within(third).getAllByRole('link').map((link) => link.textContent)).toEqual(['Kickoff', 'Review'])
    expect(screen.queryByRole('link', { name: 'Someday' })).toBeNull()

    fireEvent.click(within(third).getByRole('button', { name: 'New record on November 3' }))
    expect(onAdd).toHaveBeenCalledWith('2026-11-03')
    fireEvent.click(screen.getByRole('button', { name: 'Next month' }))
    expect(onMonth).toHaveBeenCalledWith(new Date(2026, 11, 1))

    // Dragging Launch onto the 3rd moves it there.
    const twentieth = screen.getByRole('group', { name: /November 20$/ })
    const launch = within(twentieth).getByRole('link', { name: 'Launch' })
    Object.defineProperty(document, 'elementFromPoint', { configurable: true, value: () => third })
    const pointer = (type: string, x: number) => {
      const event = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: 5 })
      Object.defineProperties(event, { pointerId: { value: 7 }, pointerType: { value: 'mouse' } })
      return event
    }
    act(() => {
      launch.dispatchEvent(pointer('pointerdown', 0))
      window.dispatchEvent(pointer('pointermove', 50))
      window.dispatchEvent(pointer('pointermove', 90))
      window.dispatchEvent(pointer('pointerup', 90))
    })
    expect(onMoveDay).toHaveBeenCalledWith(items[2], '2026-11-03')
  })

  it('lists the days that have items, when on a phone', () => {
    phone(true)
    const onAdd = vi.fn()
    calendar({ onAdd })
    expect(screen.queryByRole('group', { name: /November 3$/ })).toBeNull()
    const days = screen.getAllByRole('listitem')
    expect(days.some((day) => within(day).queryByRole('link', { name: 'Launch' }))).toBe(true)
    fireEvent.click(screen.getAllByRole('button', { name: 'New record on November 20' })[0])
    expect(onAdd).toHaveBeenCalledWith('2026-11-20')
  })

  it('loads from the first day the grid shows to the day after its last', () => {
    expect(calendarRange(new Date(2026, 10, 15))).toEqual({ from: expect.stringMatching(/^2026-1[01]-\d\d$/), to: expect.stringMatching(/^2026-12-\d\d$/) })
  })
})
