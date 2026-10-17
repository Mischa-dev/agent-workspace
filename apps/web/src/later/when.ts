/**
 * Times people type: "in 20 minutes", "tomorrow at 3pm", "friday", "next
 * week", "at 17:30", "2026-11-02 9am". Used by the /remind and /schedule
 * commands and the reminder menus. Everything is in the browser's own time
 * zone and comes out as a Date; the server stores the instant.
 */

import { formatShortDate, formatTime } from '../ui/time'

export type WhenChoice = { id: string; label: string; at: Date }

const MINUTE = 60_000

function atHour(day: Date, hours: number, minutes = 0) {
  const next = new Date(day)
  next.setHours(hours, minutes, 0, 0)
  return next
}

function addDays(day: Date, days: number) {
  const next = new Date(day)
  next.setDate(next.getDate() + days)
  return next
}

/** Next Monday at 9:00 (a week on when it's Monday already). */
function nextWeek(now: Date) {
  const days = ((8 - now.getDay()) % 7) || 7
  return atHour(addDays(now, days), 9)
}

/** The quick choices for "remind me" and "schedule". */
export function whenChoices(now = new Date()): WhenChoice[] {
  return [
    { id: '20m', label: 'In 20 minutes', at: new Date(now.getTime() + 20 * MINUTE) },
    { id: '1h', label: 'In 1 hour', at: new Date(now.getTime() + 60 * MINUTE) },
    { id: 'tomorrow', label: 'Tomorrow at 9:00', at: atHour(addDays(now, 1), 9) },
    { id: 'next-week', label: 'Next week', at: nextWeek(now) },
  ]
}

const UNITS: Record<string, number> = {
  m: MINUTE,
  min: MINUTE,
  mins: MINUTE,
  minute: MINUTE,
  minutes: MINUTE,
  h: 60 * MINUTE,
  hr: 60 * MINUTE,
  hrs: 60 * MINUTE,
  hour: 60 * MINUTE,
  hours: 60 * MINUTE,
  d: 24 * 60 * MINUTE,
  day: 24 * 60 * MINUTE,
  days: 24 * 60 * MINUTE,
  w: 7 * 24 * 60 * MINUTE,
  week: 7 * 24 * 60 * MINUTE,
  weeks: 7 * 24 * 60 * MINUTE,
}

const WEEKDAYS = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday']

// "3pm", "3:30 pm", "15:30"; a bare "3" only counts after "at".
const TIME = /^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?(?=\s|$|[,.!?])/i

type Clock = { hours: number; minutes: number; length: number }

function readClock(text: string, afterAt: boolean): Clock | null {
  const match = TIME.exec(text)
  if (!match) return null
  const [whole, hourText, minuteText, meridiem] = match
  if (!afterAt && !minuteText && !meridiem) return null
  let hours = Number(hourText)
  const minutes = minuteText ? Number(minuteText) : 0
  if (minutes > 59) return null
  if (meridiem) {
    if (hours < 1 || hours > 12) return null
    if (meridiem.toLowerCase() === 'pm' && hours !== 12) hours += 12
    if (meridiem.toLowerCase() === 'am' && hours === 12) hours = 0
  } else if (hours > 23) {
    return null
  }
  return { hours, minutes, length: whole.length }
}

/** An optional "at 3pm" or "3pm" after a day word; `rest` starts after the day word. */
function optionalClock(rest: string): Clock | null {
  const spaced = /^\s+(at\s+)?/i.exec(rest)
  if (!spaced) return null
  const clock = readClock(rest.slice(spaced[0].length), Boolean(spaced[1]))
  return clock ? { ...clock, length: spaced[0].length + clock.length } : null
}

/**
 * A time at the very start of `text`, and how many characters it took.
 * Null when the text doesn't start with one.
 */
export function parseLeadingWhen(text: string, now = new Date()): { at: Date; length: number } | null {
  const lower = text.toLowerCase()

  const relative = /^in\s+(\d{1,4}|an?|one)\s*([a-z]+)(?=\s|$|[,.!?])/.exec(lower)
  if (relative) {
    const unit = UNITS[relative[2]]
    const count = /^\d/.test(relative[1]) ? Number(relative[1]) : 1
    if (unit && count > 0) return { at: new Date(now.getTime() + count * unit), length: relative[0].length }
  }

  const half = /^in\s+half\s+an?\s+hour(?=\s|$|[,.!?])/.exec(lower)
  if (half) return { at: new Date(now.getTime() + 30 * MINUTE), length: half[0].length }

  const day = (word: string, base: Date, defaultHour: number) => {
    const clock = optionalClock(text.slice(word.length))
    const at = clock ? atHour(base, clock.hours, clock.minutes) : atHour(base, defaultHour)
    return { at, length: word.length + (clock?.length ?? 0) }
  }

  if (/^tomorrow(?=\s|$|[,.!?])/.test(lower)) return day('tomorrow', addDays(now, 1), 9)
  if (/^tonight(?=\s|$|[,.!?])/.test(lower)) return { at: atHour(now, 20), length: 'tonight'.length }
  if (/^today(?=\s|$|[,.!?])/.test(lower)) {
    const clock = optionalClock(text.slice(5))
    if (clock) return { at: atHour(now, clock.hours, clock.minutes), length: 5 + clock.length }
  }
  if (/^next week(?=\s|$|[,.!?])/.test(lower)) {
    const clock = optionalClock(text.slice(9))
    const base = nextWeek(now)
    return { at: clock ? atHour(base, clock.hours, clock.minutes) : base, length: 9 + (clock?.length ?? 0) }
  }

  const weekday = /^(?:next\s+)?(sun|mon|tue|tues|wed|thu|thur|thurs|fri|sat)[a-z]*(?=\s|$|[,.!?])/.exec(lower)
  if (weekday) {
    const index = WEEKDAYS.findIndex((name) => name.startsWith(weekday[1]))
    const word = weekday[0]
    if (index >= 0 && WEEKDAYS[index].startsWith(word.replace(/^next\s+/, ''))) {
      const days = ((index - now.getDay() + 7) % 7) || 7
      return day(text.slice(0, word.length), addDays(now, days), 9)
    }
  }

  const date = /^(\d{4})-(\d{2})-(\d{2})(?=\s|$|[,.!?])/.exec(lower)
  if (date) {
    const base = new Date(Number(date[1]), Number(date[2]) - 1, Number(date[3]))
    if (!Number.isNaN(base.getTime()) && base.getDate() === Number(date[3])) {
      return day(date[0], base, 9)
    }
  }

  const at = /^at\s+/.exec(lower)
  const clock = readClock(at ? text.slice(at[0].length) : text, Boolean(at))
  if (clock) {
    let result = atHour(now, clock.hours, clock.minutes)
    // A time that has passed today means tomorrow.
    if (result.getTime() <= now.getTime()) result = addDays(result, 1)
    return { at: result, length: (at?.[0].length ?? 0) + clock.length }
  }
  return null
}

/** A time that ends `text` ("call the printer at 3pm"), and the words before it. */
export function parseTrailingWhen(text: string, now = new Date()): { at: Date; rest: string } | null {
  const trimmed = text.trimEnd()
  const starts = [...trimmed.matchAll(/(^|\s)(?=\S)/g)].map((match) => (match.index ?? 0) + match[1].length)
  for (const start of starts) {
    let suffix = trimmed.slice(start)
    const on = /^on\s+/i.exec(suffix)
    if (on) suffix = suffix.slice(on[0].length)
    const parsed = parseLeadingWhen(suffix, now)
    if (parsed && suffix.slice(parsed.length).replace(/[.!?]+$/, '').trim() === '') {
      return { at: parsed.at, rest: trimmed.slice(0, start).trim() }
    }
  }
  return null
}

export type ParsedReminder = { note: string; at: Date | null }

/**
 * "/remind me to call the printer at 3pm", "/remind tomorrow check the
 * deploy", "/remind in 20 min stretch". A reminder without a time is an
 * error for /remind; /later keeps it with none.
 */
export function parseReminder(args: string, now = new Date()): ParsedReminder {
  let text = args.trim().replace(/^me\s+/i, '').replace(/^to\s+/i, '')
  const leading = parseLeadingWhen(text, now)
  if (leading) {
    const note = text.slice(leading.length).trim().replace(/^to\s+/i, '').replace(/^[,:-]\s*/, '')
    return { note, at: leading.at }
  }
  const trailing = parseTrailingWhen(text, now)
  if (trailing) return { note: trailing.rest.replace(/[,:-]$/, '').trim(), at: trailing.at }
  text = text.trim()
  return { note: text, at: null }
}

/** "/schedule tomorrow 9am Morning everyone": the time first, then the message. */
export function parseSchedule(args: string, now = new Date()): { at: Date; text: string } | null {
  const trimmed = args.trim()
  const leading = parseLeadingWhen(trimmed, now)
  if (!leading) return null
  const text = trimmed.slice(leading.length).replace(/^[\s,:-]+/, '')
  return { at: leading.at, text }
}

/** Input value for a datetime-local field. */
export function toLocalInput(at: Date) {
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`
}

/** "Today at 3:00 PM", "Tomorrow at 9:00 AM", "Mon, Nov 2 at 9:00 AM". */
export function describeWhen(at: Date | string, now = new Date()) {
  const date = typeof at === 'string' ? new Date(at) : at
  if (Number.isNaN(date.getTime())) return ''
  const iso = date.toISOString()
  const startOf = (day: Date) => atHour(day, 0).getTime()
  const days = Math.round((startOf(date) - startOf(now)) / (24 * 60 * MINUTE))
  const time = formatTime(iso)
  if (days === 0) return `Today at ${time}`
  if (days === 1) return `Tomorrow at ${time}`
  if (days === -1) return `Yesterday at ${time}`
  return `${formatShortDate(iso, now)} at ${time}`
}
