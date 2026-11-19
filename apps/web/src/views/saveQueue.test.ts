import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it } from 'vitest'

import { forget, inOrder, lastChange, remember, resetSaveQueue, saving, undoLast } from './saveQueue'

/** A save that finishes when the test says so. */
function deferred(log: string[], name: string) {
  let finish: (ok: boolean) => void = () => undefined
  const done = new Promise<string>((resolve, reject) => {
    finish = (ok) => (ok ? resolve(name) : reject(new Error(`${name} failed`)))
  })
  return {
    save: () => {
      log.push(`start ${name}`)
      return done.finally(() => log.push(`end ${name}`))
    },
    finish,
  }
}

const client = new QueryClient()
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

afterEach(() => resetSaveQueue())

describe('the save queue', () => {
  it('saves one thing’s changes one at a time, in the order they were made', async () => {
    const log: string[] = []
    const first = deferred(log, 'first')
    const second = deferred(log, 'second')
    const third = deferred(log, 'third')
    const results = [inOrder('task:a', first.save), inOrder('task:a', second.save), inOrder('task:a', third.save)]
    await settle()
    expect(log).toEqual(['start first'])
    expect(saving('task:a')).toBe(true)
    // The second finishing early changes nothing: it hasn't started.
    second.finish(true)
    third.finish(true)
    await settle()
    expect(log).toEqual(['start first'])
    first.finish(true)
    expect(await Promise.all(results)).toEqual(['first', 'second', 'third'])
    expect(log).toEqual(['start first', 'end first', 'start second', 'end second', 'start third', 'end third'])
    await settle()
    expect(saving('task:a')).toBe(false)
  })

  it('goes on after a save fails, and the failure reaches only its own caller', async () => {
    const log: string[] = []
    const failing = deferred(log, 'failing')
    const next = deferred(log, 'next')
    const failed = inOrder('record:r', failing.save)
    const after = inOrder('record:r', next.save)
    failing.finish(false)
    await expect(failed).rejects.toThrow('failing failed')
    await settle()
    expect(log).toContain('start next')
    next.finish(true)
    await expect(after).resolves.toBe('next')
  })

  it('never makes different things wait for each other', async () => {
    const log: string[] = []
    const slow = deferred(log, 'slow')
    const quick = deferred(log, 'quick')
    void inOrder('task:a', slow.save)
    const other = inOrder('task:b', quick.save)
    await settle()
    expect(log).toEqual(['start slow', 'start quick'])
    quick.finish(true)
    await expect(other).resolves.toBe('quick')
    slow.finish(true)
  })

  it('undoes the last change once, and forgets one that didn’t save', async () => {
    const undone: string[] = []
    remember({ label: 'Changed the status of AWD-1', undo: async () => void undone.push('AWD-1') })
    const second = { label: 'Changed the title of AWD-2', undo: async () => void undone.push('AWD-2') }
    remember(second)
    expect(lastChange()?.label).toBe('Changed the title of AWD-2')
    expect((await undoLast(client))?.label).toBe('Changed the title of AWD-2')
    expect(await undoLast(client)).toBeNull()
    expect(undone).toEqual(['AWD-2'])

    const unsaved = { label: 'Changed a record', undo: async () => undefined }
    remember(unsaved)
    forget({ label: 'someone else’s', undo: async () => undefined })
    expect(lastChange()).toBe(unsaved)
    forget(unsaved)
    expect(lastChange()).toBeNull()
  })

  it('says why an undo failed, and doesn’t try it again', async () => {
    remember({
      label: 'Changed the priority of AWD-3',
      undo: async () => {
        throw new Error('The priority changed since.')
      },
    })
    await expect(undoLast(client)).rejects.toThrow('The priority changed since.')
    expect(lastChange()).toBeNull()
  })
})
