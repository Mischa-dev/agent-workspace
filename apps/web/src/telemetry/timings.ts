// Speed (C2.29): how long the things people do most take, measured in the
// browser against the speed targets and sent to the server, which counts
// them per name (never stored, never with ids). Timings are batched and
// sent every 15 seconds, or when the page is hidden.

import { apiUrl } from '../api/client'

/** What is timed; the server drops any other name. */
export type TimingName = 'conversation_open' | 'message_send' | 'inbox_open' | 'search_results'

const ENDPOINT = '/api/v1/client-timings'
const FLUSH_MS = 15_000
const MAX_QUEUED = 20
/** Longer than this is a stalled or backgrounded tab, not speed. */
const MAX_MS = 120_000

let queue: { name: TimingName; ms: number }[] = []
let timer: ReturnType<typeof setTimeout> | null = null
let listening = false

function flush() {
  if (timer) clearTimeout(timer)
  timer = null
  if (queue.length === 0) return
  const body = JSON.stringify({ timings: queue })
  queue = []
  try {
    void fetch(apiUrl(ENDPOINT), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body,
      credentials: 'same-origin',
      keepalive: true,
    }).catch(() => undefined)
  } catch {
    // Timing must never cause an error of its own.
  }
}

/** Counts one timing; `ms` outside 0 to two minutes is dropped. */
export function recordTiming(name: TimingName, ms: number): void {
  if (!Number.isFinite(ms) || ms < 0 || ms > MAX_MS) return
  if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
  if (queue.length >= MAX_QUEUED) return
  queue.push({ name, ms: Math.round(ms) })
  if (!listening && typeof document !== 'undefined') {
    listening = true
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden') flush()
    })
  }
  timer ??= setTimeout(flush, FLUSH_MS)
}

/** Times `work` (a request) as `name` when it succeeds. */
export function timed<T>(name: TimingName, work: Promise<T>): Promise<T> {
  const started = performance.now()
  return work.then((value) => {
    recordTiming(name, performance.now() - started)
    return value
  })
}

/** What has been queued and not sent, for tests. */
export function queuedTimings() {
  return [...queue]
}
