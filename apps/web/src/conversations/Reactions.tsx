import { IconButton, Menu, Tooltip } from '../ui'
import { REACTION_CHOICES, reactionNames, type ReactionChoice } from './reactions'
import type { MessageReaction } from './types'

/**
 * A message's reactions as chips. With `onToggle`, each chip adds or takes
 * away your own reaction; without it they're read-only.
 */
export function MessageReactions({
  onToggle,
  reactions,
  viewerId,
}: {
  onToggle?: (reaction: MessageReaction) => void
  reactions: readonly MessageReaction[]
  viewerId?: string
}) {
  if (reactions.length === 0) return null
  return (
    <div aria-label="Reactions" className="aw-reactions" role="group">
      {reactions.map((reaction) => {
        const names = reactionNames(reaction, viewerId)
        const label = `${reaction.emoji} ${reaction.count}${names ? `, ${names}` : ''}`
        const className = reaction.byViewer ? 'aw-reaction is-mine' : 'aw-reaction'
        const face = (
          <>
            <span aria-hidden="true" className="aw-reaction__emoji">
              {reaction.emoji}
            </span>
            <span aria-hidden="true" className="aw-reaction__count">
              {reaction.count}
            </span>
          </>
        )
        return (
          <Tooltip key={reaction.name} label={names || reaction.name}>
            {onToggle ? (
              <button
                aria-label={label}
                aria-pressed={reaction.byViewer}
                className={className}
                onClick={() => onToggle(reaction)}
                type="button"
              >
                {face}
              </button>
            ) : (
              <span aria-label={label} className={className} role="img">
                {face}
              </span>
            )}
          </Tooltip>
        )
      })}
    </div>
  )
}

/** The toolbar's "Add reaction" menu: the 12 choices, yours checked. */
export function ReactionPicker({
  onPick,
  reactions,
}: {
  onPick: (choice: ReactionChoice, reacted: boolean) => void
  reactions: readonly MessageReaction[]
}) {
  const mine = new Set(reactions.filter((reaction) => reaction.byViewer).map((reaction) => reaction.name))
  return (
    <Menu
      items={REACTION_CHOICES.map((choice) => ({
        label: `${choice.emoji} ${choice.label}`,
        checked: mine.has(choice.name),
        onSelect: () => onPick(choice, !mine.has(choice.name)),
      }))}
      label="Add reaction"
      placement="bottom-end"
      trigger={<IconButton icon="smile-plus" label="Add reaction" size="sm" />}
    />
  )
}
