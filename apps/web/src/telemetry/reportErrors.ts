// Sends errors the app didn't handle to the server (O2), which counts and
// logs them. Only the message, where it came from and the route path go;
// query strings and hashes are stripped, and reports are capped per page
// load so a loop can't flood the server.

const MAX_REPORTS = 10
const ENDPOINT = '/api/v1/client-errors'

let sent = 0

function clip(value: string | undefined, limit: number): string | undefined {
  if (!value) {
    return undefined
  }
  return value.split(/[?#]/)[0]?.slice(0, limit)
}

export function reportError(message: string, source?: string): void {
  if (sent >= MAX_REPORTS || !message) {
    return
  }
  sent += 1
  const body = JSON.stringify({
    message: message.slice(0, 300),
    source: clip(source, 200),
    route: clip(window.location.pathname, 200),
    release: import.meta.env.MODE,
  })
  try {
    void fetch(ENDPOINT, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body,
      credentials: 'same-origin',
      keepalive: true,
    }).catch(() => undefined)
  } catch {
    // Reporting must never cause an error of its own.
  }
}

export function installErrorReporting(): void {
  window.addEventListener('error', (event) => {
    reportError(event.message, event.filename)
  })
  window.addEventListener('unhandledrejection', (event) => {
    const reason: unknown = event.reason
    const message =
      reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : 'Unhandled rejection'
    reportError(message, reason instanceof Error ? reason.name : undefined)
  })
}
