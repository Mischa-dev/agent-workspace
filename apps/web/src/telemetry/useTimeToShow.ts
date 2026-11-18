import { useEffect, useRef } from 'react'

import { recordTiming, type TimingName } from './timings'

/**
 * Times from `key` first appearing (a conversation opened, a query typed)
 * until `shown` is true for it, once per key (C2.29). A key that was
 * already shown when it appeared, from the cache, counts as instant.
 */
export function useTimeToShow(name: TimingName, key: string, shown: boolean) {
  const started = useRef<{ key: string; at: number; done: boolean } | null>(null)
  useEffect(() => {
    if (!key) return
    if (started.current?.key !== key) started.current = { key, at: performance.now(), done: false }
    const current = started.current
    if (shown && !current.done) {
      current.done = true
      recordTiming(name, performance.now() - current.at)
    }
  }, [key, name, shown])
}
