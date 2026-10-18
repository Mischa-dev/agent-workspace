import { describe, expect, it } from 'vitest'

import { matchSlashCommand, suggestSlashCommands, commandsFor } from './slashCommands'
import { parseLeadingWhen, parseReminder, parseSchedule, whenChoices } from './when'

// Wednesday 14 October 2026, 10:00 local time.
const now = new Date(2026, 9, 14, 10, 0, 0)

function local(at: Date | null | undefined) {
  if (!at) return null
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`
}

describe('typed times', () => {
  it.each([
    ['in 20 minutes', '2026-10-14 10:20'],
    ['in 2h', '2026-10-14 12:00'],
    ['in an hour', '2026-10-14 11:00'],
    ['in half an hour', '2026-10-14 10:30'],
    ['in 3 days', '2026-10-17 10:00'],
    ['tomorrow', '2026-10-15 09:00'],
    ['tomorrow at 3pm', '2026-10-15 15:00'],
    ['tomorrow 15:30', '2026-10-15 15:30'],
    ['tonight', '2026-10-14 20:00'],
    ['today at 4pm', '2026-10-14 16:00'],
    ['next week', '2026-10-19 09:00'],
    ['friday', '2026-10-16 09:00'],
    ['wed at 8am', '2026-10-21 08:00'],
    ['at 3pm', '2026-10-14 15:00'],
    ['at 9', '2026-10-15 09:00'],
    ['9:30am', '2026-10-15 09:30'],
    ['12pm', '2026-10-14 12:00'],
    ['2026-11-02 9am', '2026-11-02 09:00'],
  ])('reads “%s”', (text, expected) => {
    const parsed = parseLeadingWhen(text, now)
    expect(local(parsed?.at)).toBe(expected)
    expect(parsed?.length).toBe(text.length)
  })

  it.each(['soon', '9', 'in 5 parsecs', '25:00', 'at 13pm', 'frisbee', '2026-02-30'])('refuses “%s”', (text) => {
    expect(parseLeadingWhen(text, now)).toBeNull()
  })

  it('leaves “tonight” and “today 3pm” in the past once they have gone, for callers to refuse', () => {
    const late = new Date(2026, 9, 14, 21, 0, 0)
    expect(parseLeadingWhen('tonight', late)!.at.getTime()).toBeLessThan(late.getTime())
    expect(parseLeadingWhen('today 3pm', late)!.at.getTime()).toBeLessThan(late.getTime())
    // A bare time that has gone means tomorrow.
    expect(local(parseLeadingWhen('at 3pm', late)?.at)).toBe('2026-10-15 15:00')
  })

  it('counts days on the calendar across a clock change', () => {
    // Whatever the zone, the clock time is kept, which adding milliseconds wouldn't do over DST.
    const before = new Date(2026, 2, 27, 10, 0, 0)
    expect(local(parseLeadingWhen('in 3 days', before)?.at)).toBe('2026-03-30 10:00')
    expect(local(parseLeadingWhen('in 2 weeks', before)?.at)).toBe('2026-04-10 10:00')
    const autumn = new Date(2026, 9, 31, 10, 0, 0)
    expect(local(parseLeadingWhen('in a week', autumn)?.at)).toBe('2026-11-07 10:00')
  })

  it('means this morning by “tomorrow morning” in the small hours', () => {
    const night = new Date(2026, 9, 14, 2, 0, 0)
    const morning = whenChoices(night).find((choice) => choice.id === 'tomorrow')!
    expect(local(morning.at)).toBe('2026-10-14 09:00')
    expect(morning.label).toBe('This morning')
  })

  it('offers quick choices in order', () => {
    expect(whenChoices(now).map((choice) => local(choice.at))).toEqual([
      '2026-10-14 10:20',
      '2026-10-14 11:00',
      '2026-10-15 09:00',
      '2026-10-19 09:00',
    ])
  })
})

describe('/remind', () => {
  it.each([
    ['me to call the printer at 3pm', 'call the printer', '2026-10-14 15:00'],
    ['me to check the deploy tomorrow', 'check the deploy', '2026-10-15 09:00'],
    ['tomorrow to water the plants', 'water the plants', '2026-10-15 09:00'],
    ['in 20 min stretch', 'stretch', '2026-10-14 10:20'],
    ['send the invoice on friday', 'send the invoice', '2026-10-16 09:00'],
    ['review notes next week.', 'review notes', '2026-10-19 09:00'],
  ])('reads “%s”', (args, note, at) => {
    const parsed = parseReminder(args, now)
    expect(parsed.note).toBe(note)
    expect(local(parsed.at)).toBe(at)
  })

  it('keeps a note with no time as one', () => {
    expect(parseReminder('me to buy milk', now)).toEqual({ note: 'buy milk', at: null })
  })
})

describe('/schedule', () => {
  it('takes the time first and the message after', () => {
    const parsed = parseSchedule('tomorrow 9am Morning everyone, standup moved', now)
    expect(local(parsed?.at)).toBe('2026-10-15 09:00')
    expect(parsed?.text).toBe('Morning everyone, standup moved')
  })

  it('needs a time', () => {
    expect(parseSchedule('Morning everyone', now)).toBeNull()
  })
})

describe('slash commands', () => {
  it('matches a whole command name only', () => {
    expect(matchSlashCommand('/remind me to stretch at 3pm')).toMatchObject({
      command: { name: 'remind' },
      args: 'me to stretch at 3pm',
    })
    expect(matchSlashCommand('/MUTE')).toMatchObject({ command: { name: 'mute' }, args: '' })
    // Anything else is a message.
    expect(matchSlashCommand('/shrug')).toBeNull()
    expect(matchSlashCommand('/remindme')).toBeNull()
    expect(matchSlashCommand('and/or')).toBeNull()
    expect(matchSlashCommand('/usr/bin/env')).toBeNull()
  })

  it('suggests while the name is typed', () => {
    expect(suggestSlashCommands('/re').map((command) => command.name)).toEqual(['remind'])
    expect(suggestSlashCommands('/').length).toBeGreaterThan(3)
    expect(suggestSlashCommands('/remind ')).toEqual([])
  })

  it('leaves out project commands in a DM', () => {
    const names = commandsFor({ inProject: false }).map((command) => command.name)
    expect(names).not.toContain('task')
    expect(matchSlashCommand('/task Buy milk', commandsFor({ inProject: false }))).toBeNull()
  })
})
