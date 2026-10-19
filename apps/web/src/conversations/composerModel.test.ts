import { describe, expect, it, vi } from 'vitest'

import type { ConversationActor, ConversationWorkItemReference } from './types'
import {
  activeTrigger,
  buildMessageContent,
  composerScalarValues,
  createIdempotencyKey,
  draftFromContent,
  renameHandles,
  typedHandles,
} from './composerModel'

const actor = (id: string, handle: string): ConversationActor => ({
  id,
  handle,
  kind: 'human',
  displayName: handle,
  avatarUrl: null,
})

const item = (id: string, key: string): ConversationWorkItemReference => ({
  id,
  key,
  title: key,
  statusCategory: 'started',
  archivedAt: null,
  href: `/work-items/${id}`,
})

describe('buildMessageContent', () => {
  it('trims the draft and returns null for empty input', () => {
    expect(buildMessageContent(' \n\t ', [])).toBeNull()
    expect(buildMessageContent('  hello  ', [])).toEqual({
      version: 1,
      nodes: [{ type: 'text', text: 'hello' }],
    })
  })

  it('only structures selected, token-delimited references', () => {
    const ada = actor('actor-1', 'ada')
    const task = item('item-1', 'AW-12')
    expect(buildMessageContent('@ada, see (AW-12). Email x@ada.test and keep AW-123', [ada], [task])).toEqual({
      version: 1,
      nodes: [
        { type: 'mention', actorId: 'actor-1' },
        { type: 'text', text: ', see (' },
        { type: 'work_item_ref', workItemId: 'item-1' },
        { type: 'text', text: '). Email x@ada.test and keep AW-123' },
      ],
    })
  })

  it('prefers the longest selected token', () => {
    expect(buildMessageContent('@ada-lovelace', [actor('short', 'ada'), actor('long', 'ada-lovelace')])).toEqual({
      version: 1,
      nodes: [{ type: 'mention', actorId: 'long' }],
    })
  })

  it('stops structuring mentions at the server limit without losing text', () => {
    const ada = actor('actor-1', 'ada')
    const content = buildMessageContent(Array.from({ length: 33 }, () => '@ada').join(' '), [ada])
    expect(content?.nodes.filter((node) => node.type === 'mention')).toHaveLength(32)
    expect(content?.nodes.at(-1)).toEqual({ type: 'text', text: ' @ada' })
  })
})

describe('draftFromContent', () => {
  it('round-trips resolved nodes and deduplicates suggestion metadata', () => {
    const ada = actor('actor-1', 'ada')
    const task = item('item-1', 'AW-12')
    const draft = draftFromContent({
      content: { version: 1, nodes: [
        { type: 'mention', actorId: ada.id }, { type: 'text', text: ' and ' },
        { type: 'mention', actorId: ada.id }, { type: 'text', text: ' on ' },
        { type: 'work_item_ref', workItemId: task.id },
      ] },
      mentionedActors: [ada],
      referencedWorkItems: [task],
    })
    expect(draft).toEqual({ text: '@ada and @ada on AW-12', mentions: [ada], workItems: [task] })
    expect(buildMessageContent(draft.text, draft.mentions, draft.workItems)).toEqual(draftFromContentFixture())
  })

  it('omits unresolved structured nodes rather than inventing display text', () => {
    expect(draftFromContent({
      content: { version: 1, nodes: [
        { type: 'text', text: 'before ' },
        { type: 'mention', actorId: 'missing' },
        { type: 'work_item_ref', workItemId: 'missing' },
        { type: 'text', text: ' after' },
      ] },
      mentionedActors: [], referencedWorkItems: [],
    })).toEqual({ text: 'before  after', mentions: [], workItems: [] })
  })
})

function draftFromContentFixture() {
  return { version: 1 as const, nodes: [
    { type: 'mention' as const, actorId: 'actor-1' }, { type: 'text' as const, text: ' and ' },
    { type: 'mention' as const, actorId: 'actor-1' }, { type: 'text' as const, text: ' on ' },
    { type: 'work_item_ref' as const, workItemId: 'item-1' },
  ] }
}

describe('composer helpers', () => {
  it('counts Unicode scalar values rather than UTF-16 code units', () => {
    expect(composerScalarValues('  a🙂  ')).toBe(2)
  })

  it('detects mention and work-item triggers only at token boundaries', () => {
    expect(activeTrigger('hello @ad', 9, true)).toEqual({ kind: 'mention', query: 'ad', start: 6 })
    expect(activeTrigger('(#AW-1', 6, true)).toEqual({ kind: 'work', query: 'AW-1', start: 1 })
    expect(activeTrigger('x@ad', 4, true)).toBeNull()
    expect(activeTrigger('#AW', 3, false)).toBeNull()
    expect(activeTrigger('@'.concat('a'.repeat(41)), 42, true)).toBeNull()
  })

  it('finds handles typed without picking anyone, and spells them as stored', () => {
    const muse = actor('actor-9', 'muse')
    expect(typedHandles('@Muse can you look? cc @everyone, me@example.com', [])).toEqual(['Muse'])
    expect(typedHandles('ask @muse. And @MUSE (@mara-)', [muse])).toEqual(['mara'])
    expect(typedHandles('`@code` stays', [])).toEqual([])
    expect(renameHandles('@Muse, then (@MUSE.)', new Map([['muse', 'muse']]))).toBe('@muse, then (@muse.)')
  })

  it('uses the platform UUID generator for idempotency keys', () => {
    const randomUUID = vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue('00000000-0000-4000-8000-000000000000')
    expect(createIdempotencyKey()).toBe('00000000-0000-4000-8000-000000000000')
    expect(randomUUID).toHaveBeenCalledOnce()
  })
})
