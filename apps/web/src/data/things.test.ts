import { describe, expect, it } from 'vitest'

import { ThingStore, type ListRule, type Thing } from './things'

const task = (id: string, version: number, fields: Record<string, unknown> = {}): Thing => ({
  kind: 'task',
  id,
  version,
  title: `Task ${id}`,
  status: 'open',
  rank: Number(id.replace(/\D/g, '')),
  ...fields,
})

const openByRank: ListRule = {
  kind: 'task',
  matches: (thing) => (thing.status === undefined ? 'unknown' : thing.status === 'open'),
  compare: (left, right) => Number(left.rank) - Number(right.rank),
}

describe('the thing store', () => {
  it('ignores an older version', () => {
    const store = new ThingStore()
    store.put(task('t1', 3, { title: 'Newer' }))
    expect(store.put(task('t1', 2, { title: 'Older' }))).toBe(false)
    expect(store.get('task', 't1')?.title).toBe('Newer')
  })

  it('keeps an overlay over a base update, and settle drops it', () => {
    const store = new ThingStore()
    store.put(task('t1', 1))
    const overlay = store.overlay({ kind: 'task', id: 't1' }, { title: 'Mine' })
    store.put(task('t1', 2, { status: 'done' }))
    expect(store.get('task', 't1')).toMatchObject({ title: 'Mine', status: 'done', version: 2 })
    overlay.settle()
    expect(store.get('task', 't1')).toMatchObject({ title: 'Task t1', status: 'done' })
  })

  it('restores exactly what the server said on rollback', () => {
    const store = new ThingStore()
    const base = task('t1', 1)
    store.put(base)
    const overlay = store.overlay({ kind: 'task', id: 't1' }, { title: 'Mine', status: 'done' })
    overlay.rollback()
    expect(store.get('task', 't1')).toEqual(base)
  })

  it('places a matching thing in sort order and drops one that stops matching, without fetching', () => {
    const store = new ThingStore()
    store.setList('open', [task('t1', 1), task('t5', 1)], openByRank)
    store.put(task('t3', 1))
    expect(store.list('open').map((thing) => thing.id)).toEqual(['t1', 't3', 't5'])
    store.put(task('t1', 2, { status: 'done' }))
    expect(store.list('open').map((thing) => thing.id)).toEqual(['t3', 't5'])
    store.put(task('t9', 1, { status: 'done' }))
    expect(store.list('open').map((thing) => thing.id)).toEqual(['t3', 't5'])
    expect(store.isStale('open')).toBe(false)
  })

  it('marks only the list whose rule cannot tell as stale', () => {
    const store = new ThingStore()
    store.setList('open', [task('t1', 1)], openByRank)
    store.setList('labels', [], { kind: 'label', matches: () => true })
    store.put({ kind: 'task', id: 't2', version: 1 })
    expect(store.isStale('open')).toBe(true)
    expect(store.isStale('labels')).toBe(false)
  })

  it('takes every versioned thing in an answer', () => {
    const store = new ThingStore()
    store.normalise({ items: [task('t1', 1), { nested: { thing: task('t2', 4) } }], note: { kind: 'x', id: 'no-version' } })
    expect(store.get('task', 't2')?.version).toBe(4)
    expect(store.get('task', 't1')).toBeDefined()
    expect(store.get('x', 'no-version')).toBeUndefined()
  })
})
