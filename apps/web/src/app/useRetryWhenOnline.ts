import { useEffect, useRef } from 'react'

import { onReconnect } from './localDrafts'

/** While `waiting`, call `retry` when the connection comes back (and now and then until it does). */
export function useRetryWhenOnline(waiting: boolean, retry: () => void) {
  const latest = useRef(retry)
  useEffect(() => {
    latest.current = retry
  })
  useEffect(() => (waiting ? onReconnect(() => latest.current()) : undefined), [waiting])
}
