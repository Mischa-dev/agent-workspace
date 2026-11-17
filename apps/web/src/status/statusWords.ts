import type { AccountStatus } from './api'

/** A status as one quiet line: its emoji and words, or "Away until …". */
export function statusLine(status: AccountStatus | null): string {
  if (!status) return ''
  const away =
    status.awayUntil && new Date(status.awayUntil).getTime() > Date.now()
      ? `Away until ${new Date(status.awayUntil).toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' })}`
      : ''
  return [status.emoji, status.text || away].filter(Boolean).join(' ')
}

/** When a status set now clears, from the choice in the dialog. */
export function clearAtFor(choice: string, now: Date = new Date()): string | undefined {
  if (!choice) return undefined
  if (choice === 'today') {
    const end = new Date(now)
    end.setHours(23, 59, 0, 0)
    return end.toISOString()
  }
  return new Date(now.getTime() + Number(choice) * 60_000).toISOString()
}
