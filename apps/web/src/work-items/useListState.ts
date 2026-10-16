import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'

import { readListState, withFilter, writeListState, type ListState } from './listModel'

export type UpdateListState = (next: (current: ListState) => ListState, options?: { replace?: boolean }) => void

/** The list's state, read from and written to the URL. Updates build on the latest URL, not a stale copy. */
export function useListState() {
  const [parameters, setParameters] = useSearchParams()
  const state = useMemo(() => readListState(parameters), [parameters])
  const update = useCallback<UpdateListState>(
    (next, options) => setParameters((previous) => writeListState(next(readListState(previous))), { replace: options?.replace }),
    [setParameters],
  )
  return { state, update, parameters, setParameters }
}

export const SEARCH_DELAY = 250

/**
 * The search box's text. Typing updates the box at once and the URL once the
 * typing pauses, replacing the current history entry so Back doesn't step
 * through every keystroke.
 */
export function useDebouncedSearch(urlValue: string, update: UpdateListState, delay = SEARCH_DELAY) {
  const [text, setText] = useState(urlValue)
  const [synced, setSynced] = useState(urlValue)
  const [typing, setTyping] = useState(false)
  const pending = useRef<number | undefined>(undefined)
  // The URL changed from elsewhere (Back, Clear filters): show that.
  if (synced !== urlValue) {
    setSynced(urlValue)
    if (!typing) setText(urlValue)
  }
  useEffect(() => () => window.clearTimeout(pending.current), [])
  const change = useCallback(
    (value: string) => {
      setText(value)
      setTyping(true)
      window.clearTimeout(pending.current)
      pending.current = window.setTimeout(() => {
        pending.current = undefined
        setTyping(false)
        const query = value.trim() ? value.slice(0, 120) : undefined
        update((current) => withFilter(current, 'q', query), { replace: true })
      }, delay)
    },
    [delay, update],
  )
  return [text, change] as const
}
