import { describe, expect, it } from 'vitest'

import { parseTools, presenceLabel, profileSwatch } from './presenceModel'

const base = { taskKey: null, taskTitle: null, threadName: null, active: 0 }

describe('presenceLabel', () => {
  it('names the task an agent is working on', () => {
    expect(presenceLabel({ ...base, state: 'working', taskKey: 'KEY-12', taskTitle: 'Fix login', active: 1 })).toBe('Working on KEY-12')
    expect(presenceLabel({ ...base, state: 'working', taskKey: 'KEY-12', active: 3 })).toBe('Working on KEY-12 and 2 more')
    expect(presenceLabel({ ...base, state: 'working', threadName: 'triage', active: 1 })).toBe('Working on triage')
    expect(presenceLabel({ ...base, state: 'working', active: 1 })).toBe('Working')
  })

  it('says when it waits for a person, and idle otherwise', () => {
    expect(presenceLabel({ ...base, state: 'waiting', taskKey: 'KEY-3', active: 1 })).toBe('Waiting for you')
    expect(presenceLabel({ ...base, state: 'idle' })).toBe('Idle')
  })
})

describe('parseTools', () => {
  it('splits on commas and lines, dropping blanks and repeats', () => {
    expect(parseTools('Read, Edit\n\nBash(git:*)\nRead ,')).toEqual(['Read', 'Edit', 'Bash(git:*)'])
    expect(parseTools('  ')).toEqual([])
  })
})

describe('profileSwatch', () => {
  it('keeps kit swatches and maps other color words onto one', () => {
    expect(profileSwatch('teal', 'x')).toBe('teal')
    expect(profileSwatch('violet', 'x')).toBe('orchid')
    expect(typeof profileSwatch('chartreuse', 'x')).toBe('string')
  })
})
