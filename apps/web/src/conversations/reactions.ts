import type { MessageReaction } from './types'

export type ReactionChoice = {
  emoji: string
  name: string
  label: string
  /** Other names it's found by. */
  aliases?: readonly string[]
  /** A workspace's own emoji, drawn from its image. */
  imageUrl?: string
}

/**
 * The emoji one click away, in the server's order (src/reactions.rs): the
 * first entries of the shared list (server/src/reactions/emoji.json), which
 * has every standard emoji people can react with (C2.19). The timeline draws
 * whatever it gets back, so these lists only drive the picker.
 */
export const REACTION_CHOICES: readonly ReactionChoice[] = [
  { emoji: '👍', name: 'thumbsup', label: 'Thumbs up' },
  { emoji: '👎', name: 'thumbsdown', label: 'Thumbs down' },
  { emoji: '❤️', name: 'heart', label: 'Heart' },
  { emoji: '😄', name: 'smile', label: 'Smile' },
  { emoji: '🎉', name: 'tada', label: 'Celebrate' },
  { emoji: '😕', name: 'confused', label: 'Confused' },
  { emoji: '🚀', name: 'rocket', label: 'Rocket' },
  { emoji: '👀', name: 'eyes', label: 'Looking at it' },
  { emoji: '✅', name: 'check', label: 'Done' },
  { emoji: '❌', name: 'x', label: 'No' },
  { emoji: '🙏', name: 'pray', label: 'Thanks' },
  { emoji: '🤔', name: 'thinking', label: 'Thinking' },
]

type EmojiEntry = { emoji: string; name: string; aliases?: string[] }

/** The shared list of every standard emoji, loaded when the picker searches. */
export function loadEmojiList(): Promise<ReactionChoice[]> {
  return import('../../../server/src/reactions/emoji.json').then((module) => emojiChoices(module.default as EmojiEntry[]))
}

export function emojiChoices(entries: readonly EmojiEntry[]): ReactionChoice[] {
  return entries.map((entry) => ({
    emoji: entry.emoji,
    name: entry.name,
    label: entry.name.replaceAll('_', ' '),
    aliases: entry.aliases ?? [],
  }))
}

/** Emoji whose name or alias starts with the query, then those that contain it. */
export function searchEmoji(choices: readonly ReactionChoice[], query: string, limit = 48): ReactionChoice[] {
  const needle = query.trim().toLowerCase().replace(/^:+|:+$/g, '').replace(/[\s-]+/g, '_')
  if (!needle) return choices.slice(0, limit)
  const starts: ReactionChoice[] = []
  const contains: ReactionChoice[] = []
  for (const choice of choices) {
    const names = [choice.name, ...(choice.aliases ?? [])]
    if (choice.emoji === query.trim() || names.some((name) => name.startsWith(needle))) starts.push(choice)
    else if (names.some((name) => name.includes(needle))) contains.push(choice)
  }
  return [...starts, ...contains].slice(0, limit)
}

/**
 * The reactions after you add (`reacted`) or take away one, for showing a
 * change before the server confirms it. Adding a new emoji puts it last, as
 * the server orders by first added.
 */
export function applyReaction(
  reactions: readonly MessageReaction[],
  choice: Pick<ReactionChoice, 'emoji' | 'name' | 'imageUrl'>,
  viewerId: string,
  reacted: boolean,
): MessageReaction[] {
  const existing = reactions.find((reaction) => reaction.name === choice.name)
  if (reacted) {
    if (existing?.byViewer) return [...reactions]
    if (existing) {
      return reactions.map((reaction) =>
        reaction === existing ? { ...reaction, count: reaction.count + 1, byViewer: true } : reaction,
      )
    }
    return [
      ...reactions,
      { emoji: choice.emoji, name: choice.name, count: 1, byViewer: true, actors: [], ...(choice.imageUrl ? { imageUrl: choice.imageUrl } : {}) },
    ]
  }
  if (!existing?.byViewer) return [...reactions]
  if (existing.count <= 1) return reactions.filter((reaction) => reaction !== existing)
  return reactions.map((reaction) =>
    reaction === existing
      ? {
          ...reaction,
          count: reaction.count - 1,
          byViewer: false,
          actors: reaction.actors.filter((actor) => actor.id !== viewerId),
        }
      : reaction,
  )
}

/** At most this many names in a reaction's tooltip; the rest are counted. */
const NAMED = 6

/** Who reacted, in words: "You, Mara and 3 others". */
export function reactionNames(reaction: MessageReaction, viewerId: string | undefined) {
  const names = [
    ...(reaction.byViewer ? ['You'] : []),
    ...reaction.actors.filter((actor) => actor.id !== viewerId).map((actor) => actor.displayName),
  ].slice(0, Math.min(NAMED, Math.max(1, reaction.count)))
  const others = reaction.count - names.length
  if (others > 0) names.push(others === 1 ? '1 other' : `${others} others`)
  if (names.length === 0) return ''
  if (names.length === 1) return names[0]
  return `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`
}
