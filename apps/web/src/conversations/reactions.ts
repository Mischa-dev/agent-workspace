import type { MessageReaction } from './types'

export type ReactionChoice = { emoji: string; name: string; label: string }

/**
 * The emoji people can react with, in the server's order (src/reactions.rs).
 * The server stores and returns whichever of these it accepts; the timeline
 * draws whatever emoji it gets back, so this list only drives the picker.
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

/**
 * The reactions after you add (`reacted`) or take away one, for showing a
 * change before the server confirms it. Adding a new emoji puts it last, as
 * the server orders by first added.
 */
export function applyReaction(
  reactions: readonly MessageReaction[],
  choice: Pick<ReactionChoice, 'emoji' | 'name'>,
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
    return [...reactions, { emoji: choice.emoji, name: choice.name, count: 1, byViewer: true, actors: [] }]
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
