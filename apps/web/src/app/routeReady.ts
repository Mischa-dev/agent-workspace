// Route timing (W1.33, plan 3.10): each page marks `aw:route-start` when it
// starts and `aw:route-ready` once it shows its content, and measures
// `aw:route` between them, so the speed test (e2e/perf.spec.ts) and any
// telemetry read one number. "Shows its content" means: nothing is fetching,
// nothing in the page area is a skeleton (`aria-busy`), and the page area
// holds something. A first load starts at the navigation's start (0).

import { useEffect, useRef } from 'react'
import { useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useLocation } from 'react-router-dom'

/** The longest a page is watched; one still loading by then isn't marked. */
const WATCH_MS = 30_000

function mark(name: string, detail: Record<string, unknown>) {
  try {
    performance.mark(name, { detail })
  } catch {
    // Old browsers take no detail; the timing is a nicety.
  }
}

function contentShown(): boolean {
  const main = document.getElementById('main-workspace')
  if (!main) return false
  return !main.querySelector('[aria-busy="true"]') && (main.textContent ?? '').trim().length > 0
}

/** Watch one page from `start` (ms since the navigation began) until it shows its content. */
export function watchRouteReady(queryClient: QueryClient, path: string, start: number): () => void {
  if (typeof requestAnimationFrame !== 'function' || typeof performance === 'undefined') return () => {}
  if (start > 0) mark('aw:route-start', { path })
  let frame = 0
  const check = () => {
    if (queryClient.isFetching() === 0 && contentShown()) {
      const end = performance.now()
      mark('aw:route-ready', { path, ms: Math.round(end - start) })
      try {
        performance.measure('aw:route', { start, end, detail: { path } })
      } catch {
        // As above.
      }
      return
    }
    if (performance.now() - start < WATCH_MS) frame = requestAnimationFrame(check)
  }
  frame = requestAnimationFrame(check)
  return () => cancelAnimationFrame(frame)
}

/** Marks every page of the shell as it opens; a first load counts from 0. */
export default function RouteReadyMarks() {
  const queryClient = useQueryClient()
  const { pathname } = useLocation()
  const first = useRef(true)
  useEffect(() => {
    const start = first.current ? 0 : performance.now()
    first.current = false
    return watchRouteReady(queryClient, pathname, start)
  }, [pathname, queryClient])
  return null
}
