import { useCallback, useEffect, useRef, useState } from 'react'

import type { Notice } from './notice'

/** A short note about the last change. It fades after a while unless it's an error. */
export function useNotice() {
  const [notice, setNotice] = useState<Notice | null>(null)
  const timer = useRef<number | undefined>(undefined)
  const notify = useCallback((next: Notice | null) => {
    window.clearTimeout(timer.current)
    setNotice(next)
    if (next && next.tone !== 'danger' && next.tone !== 'warning') {
      timer.current = window.setTimeout(() => setNotice(null), next.action ? 10_000 : 5_000)
    }
  }, [])
  useEffect(() => () => window.clearTimeout(timer.current), [])
  return { notice, notify }
}

