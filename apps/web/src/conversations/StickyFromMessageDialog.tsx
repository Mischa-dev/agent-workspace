import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { callOp, newRequestId } from '../api/ops'
import { listWhiteboards, whiteboardPath } from '../whiteboards/api'
import { whiteboardKeys } from '../whiteboards/keys'
import { Banner, Button, Checkbox, Dialog, SelectField, describeError, showToast } from '../ui'
import type { ConversationMessage } from './types'

const NEW_BOARD = 'new'
/** A sticky's colour: drawn on the board, the same in light and dark. */
const STICKY = '#ffec99'

function firstLine(text: string) {
  const line = text.trim().split('\n')[0]?.trim() ?? ''
  return (line.length > 120 ? `${line.slice(0, 119)}…` : line) || 'From a conversation'
}

/**
 * Put a message on a whiteboard as a sticky (C2.22): on one of the space's
 * boards, or on a new one named after it. The sticky links back to the
 * message, which then shows what it was made into; "Say so" also tells the
 * conversation.
 */
export function StickyFromMessageDialog({
  message,
  onClose,
  projectId,
}: {
  message: ConversationMessage
  onClose: () => void
  projectId: string
}) {
  const navigate = useNavigate()
  const boards = useQuery({
    queryKey: whiteboardKeys.list({ projectId }),
    queryFn: () => listWhiteboards({ projectId }),
  })
  const choices = (boards.data?.whiteboards ?? []).filter((board) => board.projectId === projectId)
  const [picked, setPicked] = useState('')
  const target = picked || choices[0]?.id || NEW_BOARD
  const [saySo, setSaySo] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [requestId] = useState(newRequestId)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busy) return
    setBusy(true)
    setError(null)
    const text = message.plainText.trim().slice(0, 500)
    const boardId =
      target === NEW_BOARD
        ? callOp('whiteboard_create', { title: firstLine(message.plainText), projectId }, { requestId }).then((made) => made.whiteboard.id)
        : Promise.resolve(target)
    void boardId
      .then(async (id) => {
        await callOp('whiteboard_update_elements', {
          whiteboardId: id,
          fromMessageId: message.id,
          saySo,
          operations: [
            {
              op: 'add',
              type: 'rectangle',
              x: 80 + Math.round(Math.random() * 400),
              y: 80 + Math.round(Math.random() * 300),
              width: 240,
              height: 160,
              text,
              backgroundColor: STICKY,
              fillStyle: 'solid',
              rounded: true,
            },
          ],
        })
        return id
      })
      .then((id) => {
        showToast({
          tone: 'success',
          title: 'Put on the whiteboard',
          action: { label: 'Open', onSelect: () => navigate(whiteboardPath(id)) },
        })
        onClose()
      })
      .catch((reason: unknown) => setError(`The sticky wasn’t made. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description="The sticky links back to this message."
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            Put on whiteboard
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Put this message on a whiteboard"
      width={480}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <SelectField data-autofocus label="Whiteboard" onChange={(event) => setPicked(event.target.value)} value={target}>
        {choices.map((board) => (
          <option key={board.id} value={board.id}>
            {board.title}
          </option>
        ))}
        <option value={NEW_BOARD}>A new whiteboard</option>
      </SelectField>
      <Checkbox checked={saySo} label="Say so in the conversation" onChange={setSaySo} />
    </Dialog>
  )
}

export default StickyFromMessageDialog
