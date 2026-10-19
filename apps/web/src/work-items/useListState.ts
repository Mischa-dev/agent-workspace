import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'

import { readListState, withFilter, writeListState, type ListState } from './listModel'

export type UpdateListState = (next: (current: ListState) => ListState, options?: { replace?: boolean }) => void

/** The list's state, read from and written to the URL. Updates build on the latest URL, not a stale copy. */
export function useListState() {
  const [parameters, setParameters] = useSearchParams()
  const state = useMemo(() => readListState(parameters), [parameters])
  // The router gives each update the URL as of the last render, so a second
  // update before the next render (two quick picks in a filter) would drop
  // the first. Until the URL catches up, updates build on the last write.
  const pending = useRef<URLSearchParams | null>(null)
  useEffect(() => {
    pending.current = null
  }, [parameters])
  const update = useCallback<UpdateListState>(
    (next, options) => {
      const written = writeListState(next(readListState(pending.current ?? parameters)))
      pending.current = written
      setParameters(written, { replace: options?.replace })
    },
    [parameters, setParameters],
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
  // What our own last write put in the URL, until the URL shows it.
  const [sent, setSent] = useState<string | null>(null)
  // Counts URL changes from elsewhere; each one cancels unsent typing.
  const [outside, setOutside] = useState(0)
  const pending = useRef<number | undefined>(undefined)
  if (synced !== urlValue) {
    setSynced(urlValue)
    setSent(null)
    // The URL changed from elsewhere (Back, Clear filters): it wins over
    // anything typed but not yet sent.
    if (sent === null || urlValue !== sent) {
      setText(urlValue)
      setOutside((count) => count + 1)
    }
  }
  useEffect(() => {
    window.clearTimeout(pending.current)
    pending.current = undefined
  }, [outside])
  useEffect(() => () => window.clearTimeout(pending.current), [])
  const change = useCallback(
    (value: string) => {
      setText(value)
      window.clearTimeout(pending.current)
      pending.current = window.setTimeout(() => {
        pending.current = undefined
        const query = value.trim() ? value.slice(0, 120) : undefined
        setSent(query ?? '')
        update((current) => withFilter(current, 'q', query), { replace: true })
      }, delay)
    },
    [delay, update],
  )
  return [text, change] as const
}
