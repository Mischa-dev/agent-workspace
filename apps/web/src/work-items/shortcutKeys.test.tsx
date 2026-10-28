import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useOpenAsYouGo } from './shortcutKeys'

beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('the side panel following the keyboard', () => {
  it('opens only where the selection comes to rest', () => {
    const open = vi.fn()
    const { result } = renderHook(() => useOpenAsYouGo(open))
    act(() => {
      result.current.follow('first')
      vi.advanceTimersByTime(50)
      result.current.follow('second')
      vi.advanceTimersByTime(50)
      result.current.follow('third')
    })
    expect(open).not.toHaveBeenCalled()
    act(() => vi.advanceTimersByTime(200))
    expect(open).toHaveBeenCalledTimes(1)
    expect(open).toHaveBeenCalledWith('third')
  })

  it('drops a pending open when Escape or a click comes first', () => {
    const open = vi.fn()
    const { result, unmount } = renderHook(() => useOpenAsYouGo(open))
    act(() => {
      result.current.follow('first')
      result.current.cancel()
      vi.advanceTimersByTime(500)
    })
    expect(open).not.toHaveBeenCalled()
    act(() => result.current.follow('second'))
    unmount()
    act(() => vi.advanceTimersByTime(500))
    expect(open).not.toHaveBeenCalled()
  })
})
