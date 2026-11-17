import { describe, expect, it } from 'vitest'

import { applyReaction, loadEmojiList, REACTION_CHOICES, reactionNames, searchEmoji } from './reactions'
import type { ConversationActor, MessageReaction } from './types'

const viewer = '019fb600-0000-7000-8000-000000000001'
const actor = (id: string, displayName: string): ConversationActor => ({
  id,
  kind: 'human',
  displayName,
  handle: displayName.toLowerCase(),
  avatarUrl: null,
})
const mara = actor('019fb600-0000-7000-8000-000000000002', 'Mara')
const me = actor(viewer, 'Me')
function choice(name: string) {
  const found = REACTION_CHOICES.find((candidate) => candidate.name === name)
  if (!found) throw new Error(`no ${name}`)
  return found
}
const eyes = choice('eyes')
const check = choice('check')
const reaction = (change: Partial<MessageReaction>): MessageReaction => ({
  emoji: '👀',
  name: 'eyes',
  count: 1,
  byViewer: false,
  actors: [mara],
  ...change,
})

describe('reactions shown before the server answers', () => {
  it('adds and takes away only your own reaction', () => {
    const others = [reaction({})]
    expect(applyReaction(others, eyes, viewer, true)).toEqual([reaction({ count: 2, byViewer: true })])
    expect(applyReaction(others, check, viewer, true)).toEqual([
      reaction({}),
      { emoji: '✅', name: 'check', count: 1, byViewer: true, actors: [] },
    ])
    // Taking away what isn't yours, or adding what is, changes nothing.
    expect(applyReaction(others, eyes, viewer, false)).toEqual(others)
    const mine = [reaction({ count: 2, byViewer: true, actors: [mara, me] })]
    expect(applyReaction(mine, eyes, viewer, true)).toEqual(mine)
    expect(applyReaction(mine, eyes, viewer, false)).toEqual([reaction({})])
    expect(applyReaction([reaction({ byViewer: true, actors: [me] })], eyes, viewer, false)).toEqual([])
  })

  it('names who reacted, you first', () => {
    expect(reactionNames(reaction({ count: 2, byViewer: true, actors: [me, mara] }), viewer)).toBe('You and Mara')
    expect(reactionNames(reaction({ count: 5 }), viewer)).toBe('Mara and 4 others')
    expect(reactionNames(reaction({ count: 2, actors: [mara] }), viewer)).toBe('Mara and 1 other')
  })

  it('offers the twelve choices the server accepts', () => {
    expect(REACTION_CHOICES.map((choice) => choice.emoji).join(' ')).toBe('👍 👎 ❤️ 😄 🎉 😕 🚀 👀 ✅ ❌ 🙏 🤔')
  })
})

describe('the shared emoji list (C2.19)', () => {
  it('starts with the quick reactions, in the server’s order, and names each emoji once', async () => {
    const list = await loadEmojiList()
    expect(list.length).toBeGreaterThan(1000)
    expect(list.slice(0, REACTION_CHOICES.length).map(({ emoji, name }) => ({ emoji, name }))).toEqual(
      REACTION_CHOICES.map(({ emoji, name }) => ({ emoji, name })),
    )
    expect(new Set(list.map((choice) => choice.name)).size).toBe(list.length)
    expect(new Set(list.map((choice) => choice.emoji)).size).toBe(list.length)
  })

  it('finds emoji by name and alias, starts first', async () => {
    const list = await loadEmojiList()
    expect(searchEmoji(list, 'unicorn')[0]?.emoji).toBe('🦄')
    expect(searchEmoji(list, ':white_check_mark:')[0]?.name).toBe('check')
    expect(searchEmoji(list, 'zzzz-nothing')).toEqual([])
  })
})
