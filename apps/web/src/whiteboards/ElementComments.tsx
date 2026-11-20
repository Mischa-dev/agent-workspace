import { useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, invalidateOps, newRequestId } from '../api/ops'
import { searchMentionableActors } from '../conversations/api'
import { Button, Dialog, SelectField, TextArea, actorKind, describeError } from '../ui'
import { whiteboardKeys } from './keys'
import { toScreen, type BoardSelection, type BoardViewport, type PinnedThread } from './pins'

/**
 * Pins for the threads about board elements (C1.31), over the canvas where
 * each thread was started; a pin opens its thread.
 */
export function BoardPins({
  onOpen,
  pinned,
  viewport,
}: {
  onOpen: (pinned: PinnedThread) => void
  pinned: readonly PinnedThread[]
  viewport: BoardViewport | null
}) {
  if (!viewport || !pinned.length) return null
  return (
    <div className="aw-wb__overlay">
      {pinned.map((entry) => {
        const { left, top } = toScreen(entry.anchor.at, viewport)
        const count = entry.thread.messageCount
        return (
          <button
            aria-label={`${count === 1 ? 'One comment' : `${count} comments`} on ${entry.anchor.elementIds.length === 1 ? 'an element' : `${entry.anchor.elementIds.length} elements`}`}
            className={['aw-wb__pin', entry.thread.unread ? 'is-unread' : ''].join(' ')}
            key={entry.thread.id}
            onClick={() => onOpen(entry)}
            style={{ left, top }}
            type="button"
          >
            {count}
          </button>
        )
      })}
    </div>
  )
}

/** Comment on or ask about what's selected, beside the selection. */
export function SelectionActions({
  canAsk,
  onAsk,
  onComment,
  selection,
  viewport,
}: {
  canAsk: boolean
  onAsk: () => void
  onComment: () => void
  selection: BoardSelection | null
  viewport: BoardViewport | null
}) {
  if (!selection || !viewport) return null
  const { left, top } = toScreen({ x: selection.x + selection.width, y: selection.y }, viewport)
  return (
    <div className="aw-wb__overlay">
      <div className="aw-wb__selection-actions" style={{ left: Math.max(8, left + 8), top: Math.max(8, top - 40) }}>
        <Button icon="messages" onClick={onComment} size="sm">
          Comment
        </Button>
        {canAsk ? (
          <Button icon="bot" onClick={onAsk} size="sm">
            Ask about this
          </Button>
        ) : null}
      </div>
    </div>
  )
}

/**
 * Ask someone in the board's space, usually an agent, about the selected
 * elements: a thread about them starts with a picture of them attached.
 */
export function AskDialog({
  onAsked,
  onClose,
  selection,
  spaceId,
  viewerId,
  whiteboardId,
}: {
  onAsked: (threadId: string) => void
  onClose: () => void
  selection: BoardSelection
  spaceId: string
  viewerId: string
  whiteboardId: string
}) {
  const queryClient = useQueryClient()
  const people = useQuery({
    queryKey: whiteboardKeys.askable(spaceId),
    queryFn: () => searchMentionableActors(spaceId, ''),
  })
  // Agents first: they are who people ask most.
  const choices = (people.data?.actors ?? [])
    .filter((actor) => actor.id !== viewerId)
    .sort((a, b) => Number(actorKind(b.kind) === 'agent') - Number(actorKind(a.kind) === 'agent'))
  const [accountId, setAccountId] = useState('')
  const [question, setQuestion] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The first choice until someone picks another.
  const chosen = accountId || (choices[0]?.id ?? '')

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!chosen || !question.trim()) return
    setBusy(true)
    setError(null)
    try {
      const asked = await callOp(
        'whiteboard_ask',
        { whiteboardId, elementIds: selection.ids, accountId: chosen, question: question.trim() },
        { requestId: newRequestId() },
      )
      await invalidateOps(queryClient, ['comment_list'])
      onAsked(asked.threadId)
    } catch (reason) {
      setError(describeError(reason).message)
    } finally {
      setBusy(false)
    }
  }

  const count = selection.ids.length
  return (
    <Dialog
      description={`${count === 1 ? 'The element' : `The ${count} elements`} go with your question as a picture, in a thread about them.`}
      dirty={Boolean(question.trim())}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} disabled={!chosen || !question.trim()} type="submit" variant="primary">
            Ask
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title="Ask about this"
    >
      <SelectField label="Ask" onChange={(event) => setAccountId(event.target.value)} value={chosen}>
        {choices.map((actor) => (
          <option key={actor.id} value={actor.id}>
            {actor.displayName}
          </option>
        ))}
      </SelectField>
      <TextArea
        error={error ?? undefined}
        label="Question"
        onChange={(event) => setQuestion(event.target.value)}
        rows={4}
        value={question}
      />
    </Dialog>
  )
}
