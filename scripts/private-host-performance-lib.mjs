import { isIP } from 'node:net'

export const WARMUP_SAMPLES = 5
export const MEASURED_SAMPLES = 25

export function validateLoopbackBaseUrl(value) {
  const url = new URL(value)
  if (url.protocol !== 'http:' || url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
    throw new Error('base URL must be an origin-only loopback HTTP URL')
  }
  const hostname = url.hostname.replace(/^\[|\]$/g, '')
  if (!(hostname === 'localhost' || hostname === '::1' || (isIP(hostname) === 4 && hostname.startsWith('127.')))) {
    throw new Error('base URL must resolve explicitly to loopback')
  }
  return url.origin
}

export function summarizeDurations(durations) {
  if (durations.length !== MEASURED_SAMPLES || durations.some((value) => !Number.isFinite(value) || value < 0)) {
    throw new Error(`exactly ${MEASURED_SAMPLES} finite non-negative samples are required`)
  }
  const sorted = [...durations].sort((left, right) => left - right)
  return {
    samples: sorted.length,
    minMs: round(sorted[0]),
    medianMs: round(sorted[Math.floor(sorted.length / 2)]),
    p95Ms: round(sorted[Math.ceil(sorted.length * 0.95) - 1]),
    maxMs: round(sorted.at(-1)),
  }
}

export function parseSetCookieHeaders(headers) {
  const values = typeof headers.getSetCookie === 'function'
    ? headers.getSetCookie()
    : splitCombinedSetCookie(headers.get('set-cookie') ?? '')
  const cookies = new Map()
  for (const value of values) {
    const pair = value.split(';', 1)[0]
    const separator = pair.indexOf('=')
    if (separator > 0) cookies.set(pair.slice(0, separator), pair.slice(separator + 1))
  }
  return cookies
}

export function selectTimelineThread(channels) {
  const threads = Array.isArray(channels)
    ? channels.flatMap((channel) => Array.isArray(channel?.threads) ? channel.threads : [])
    : []
  const valid = threads.filter((thread) => thread && typeof thread.id === 'string')
  return valid.find((thread) => Number.isSafeInteger(Number(thread.highestSequence)) && Number(thread.highestSequence) > 0)
    ?? valid[0]
    ?? null
}

function splitCombinedSetCookie(value) {
  if (!value) return []
  return value.split(/,(?=\s*[^;,=]+=[^;,]*)/).map((part) => part.trim())
}

function round(value) {
  return Math.round(value * 100) / 100
}
