import { useEffect, useState } from 'react'

import type { UsageSchedule, UsageWindow } from './usageApi'

/** The current time, ticking every `intervalMs` while the page is open. */
export function useNow(intervalMs = 1000) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs)
    return () => window.clearInterval(timer)
  }, [intervalMs])
  return now
}

/** "3 d 4 h", "2 h 14 min", "4 min 05 s", or "now". */
export function formatCountdown(milliseconds: number) {
  if (milliseconds <= 0) return 'now'
  const seconds = Math.floor(milliseconds / 1000)
  const days = Math.floor(seconds / 86_400)
  const hours = Math.floor((seconds % 86_400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const rest = seconds % 60
  if (days > 0) return hours > 0 ? `${days} d ${hours} h` : `${days} d`
  if (hours > 0) return `${hours} h ${minutes} min`
  if (minutes > 0) return `${minutes} min ${String(rest).padStart(2, '0')} s`
  return `${rest} s`
}

/** When a window resets, as seen at `now`. */
export function resetText(window: UsageWindow, now: number) {
  if (!window.resetsAt) return window.limitReached ? 'Limit reached' : 'Reset time not reported'
  const at = Date.parse(window.resetsAt)
  if (Number.isNaN(at)) return 'Reset time not reported'
  const left = at - now
  return left <= 0 ? 'Resetting now' : `Resets in ${formatCountdown(left)}`
}

/** Tone for a window's meter: calm, getting full, or used up. */
export function windowTone(window: UsageWindow): 'ok' | 'warning' | 'danger' | 'unknown' {
  if (window.limitReached) return 'danger'
  if (window.usedPercent == null) return 'unknown'
  if (window.usedPercent >= 90) return 'danger'
  if (window.usedPercent >= 70) return 'warning'
  return 'ok'
}

const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday']

export function weekdayName(weekday: number) {
  return WEEKDAYS[weekday - 1] ?? 'Monday'
}

export function weekdays() {
  return WEEKDAYS.map((label, index) => ({ value: index + 1, label }))
}

export function minuteToTime(minute: number) {
  const clamped = Math.max(0, Math.min(1439, Math.round(minute)))
  return `${String(Math.floor(clamped / 60)).padStart(2, '0')}:${String(clamped % 60).padStart(2, '0')}`
}

export function timeToMinute(value: string) {
  const match = /^(\d{1,2}):(\d{2})$/.exec(value)
  if (!match) return null
  const hours = Number(match[1])
  const minutes = Number(match[2])
  if (hours > 23 || minutes > 59) return null
  return hours * 60 + minutes
}

/** "Every Monday at 09:00 (Europe/Berlin)". */
export function describeSchedule(schedule: UsageSchedule) {
  const at = `at ${minuteToTime(schedule.minuteOfDay)} (${schedule.timeZone})`
  if (schedule.cadence === 'daily') return `Every day ${at}`
  if (schedule.cadence === 'weekly') return `Every ${weekdayName(schedule.weekday ?? 1)} ${at}`
  return `Day ${schedule.monthDay ?? 1} of every month ${at}`
}

export function browserTimeZone() {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

/** A fresh key for one spend attempt; a retried click reuses it. */
export function newAttemptKey(prefix = 'spend') {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return `${prefix}-${crypto.randomUUID()}`
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}

/** "Launch 3 tasks", "Launch 1 task". */
export function launchLabel(count: number) {
  return count === 1 ? 'Launch 1 task' : `Launch ${count} tasks`
}

/** A batch result in words, for a toast. */
export function batchOutcome(result: { launched: number; failed: { tasks: number; projectName: string | null; reason: string }[] }) {
  const queued = result.launched === 1 ? '1 task queued' : `${result.launched} tasks queued`
  if (result.failed.length === 0) return { tone: 'success' as const, title: queued, description: 'Each starts in its own project’s environment as a slot frees up.' }
  const first = result.failed[0]
  const where = first.projectName ?? 'One project'
  return {
    tone: result.launched > 0 ? ('info' as const) : ('danger' as const),
    title: result.launched > 0 ? queued : 'Nothing was launched',
    description: `${where}: ${first.reason}${result.failed.length > 1 ? ` (and ${result.failed.length - 1} more)` : ''}`,
  }
}
