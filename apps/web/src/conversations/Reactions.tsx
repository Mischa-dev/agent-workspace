import { useRef, useState, type KeyboardEvent } from 'react'

import { Icon, IconButton, Tooltip } from '../ui'
import { Popover } from '../ui/Popover'
import { REACTION_CHOICES, reactionNames, type ReactionChoice } from './reactions'
import type { MessageReaction } from './types'
import '../styles/conversation.css'

/** Columns in the emoji grid, for arrow keys. */
const COLUMNS = 6

/**
 * A message's reactions as chips. With `onToggle`, each chip adds or takes
 * away your own reaction, and a last chip adds a new one; without it they're
 * read-only.
 */
export function MessageReactions({
  onPick,
  onToggle,
  reactions,
  viewerId,
}: {
  /** Adds the "add reaction" chip at the end of the row. */
  onPick?: (choice: ReactionChoice, reacted: boolean) => void
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
        const named = REACTION_CHOICES.find((choice) => choice.name === reaction.name)?.label ?? reaction.emoji
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
          <Tooltip key={reaction.name} label={names ? `${names} reacted with ${reaction.emoji}` : named}>
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
      {onPick ? <ReactionPicker onPick={onPick} reactions={reactions} variant="chip" /> : null}
    </div>
  )
}

/** The reactions one click away in a message's toolbar, before the full picker. */
const QUICK_REACTIONS = ['check', 'eyes', 'thumbsup']
  .map((name) => REACTION_CHOICES.find((choice) => choice.name === name))
  .filter((choice): choice is ReactionChoice => Boolean(choice))

/** One-click reactions for a message's toolbar: each adds yours, or takes it away again. */
export function QuickReactions({
  onPick,
  reactions,
}: {
  onPick: (choice: ReactionChoice, reacted: boolean) => void
  reactions: readonly MessageReaction[]
}) {
  const mine = new Set(reactions.filter((reaction) => reaction.byViewer).map((reaction) => reaction.name))
  return QUICK_REACTIONS.map((choice) => (
    <Tooltip key={choice.name} label={mine.has(choice.name) ? `Remove your ${choice.emoji}` : `React with ${choice.emoji}`}>
      <button
        aria-label={`React with ${choice.label}`}
        aria-pressed={mine.has(choice.name)}
        className="aw-iconbtn aw-iconbtn--sm aw-quickreact"
        onClick={() => onPick(choice, !mine.has(choice.name))}
        type="button"
      >
        <span aria-hidden="true">{choice.emoji}</span>
      </button>
    </Tooltip>
  ))
}

/**
 * "Add reaction": a small grid of emoji, yours marked. One click reacts (or
 * takes yours away) and closes it; arrow keys move around the grid.
 */
export function ReactionPicker({
  onPick,
  reactions,
  variant = 'toolbar',
}: {
  onPick: (choice: ReactionChoice, reacted: boolean) => void
  reactions: readonly MessageReaction[]
  /** The toolbar's icon button, or the chip at the end of a message's reactions. */
  variant?: 'toolbar' | 'chip'
}) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null)
  const grid = useRef<HTMLDivElement>(null)
  const mine = new Set(reactions.filter((reaction) => reaction.byViewer).map((reaction) => reaction.name))
  const toggle = (element: HTMLElement) => setAnchor((current) => (current ? null : element))

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const buttons = [...(grid.current?.querySelectorAll<HTMLButtonElement>('button') ?? [])]
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
    if (index < 0) return
    const step = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: COLUMNS, ArrowUp: -COLUMNS }[event.key]
    if (!step) return
    event.preventDefault()
    buttons[Math.min(buttons.length - 1, Math.max(0, index + step))]?.focus()
  }

  return (
    <>
      {variant === 'chip' ? (
        <Tooltip label="Add reaction">
          <button
            aria-expanded={Boolean(anchor)}
            aria-haspopup="dialog"
            aria-label="Add reaction"
            className="aw-reaction aw-reaction--add"
            onClick={(event) => toggle(event.currentTarget)}
            type="button"
          >
            <Icon name="smile-plus" size={14} />
          </button>
        </Tooltip>
      ) : (
        <IconButton
          aria-expanded={Boolean(anchor)}
          aria-haspopup="dialog"
          icon="smile-plus"
          label="Add reaction"
          onClick={(event) => toggle(event.currentTarget)}
          pressed={Boolean(anchor)}
          size="sm"
        />
      )}
      <Popover anchor={anchor} label="Add reaction" onClose={() => setAnchor(null)} open={Boolean(anchor)} placement="bottom-end">
        <div className="aw-emojigrid" onKeyDown={onKeyDown} ref={grid} role="group">
          {REACTION_CHOICES.map((choice) => (
            <button
              aria-label={choice.label}
              aria-pressed={mine.has(choice.name)}
              className={mine.has(choice.name) ? 'aw-emojigrid__item is-mine' : 'aw-emojigrid__item'}
              key={choice.name}
              onClick={() => {
                onPick(choice, !mine.has(choice.name))
                setAnchor(null)
              }}
              title={choice.label}
              type="button"
            >
              <span aria-hidden="true">{choice.emoji}</span>
            </button>
          ))}
        </div>
      </Popover>
    </>
  )
}
