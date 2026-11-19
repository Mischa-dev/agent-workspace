import { useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { callOp, newRequestId } from '../api/ops'
import { collectionKeys, listCollections } from '../collections/api'
import { projectAreaHref } from '../projects/locations'
import { Banner, Button, Checkbox, Dialog, SelectField, TextField, describeError, showToast } from '../ui'
import type { ConversationMessage } from './types'

function firstLine(text: string) {
  const line = text.trim().split('\n')[0]?.trim() ?? ''
  return line.length > 300 ? `${line.slice(0, 299)}…` : line
}

/**
 * Make a message into a record in one of the space's collections (C2.21).
 * The record links back to the message, which then shows what it was made
 * into; "Say so" also tells the conversation.
 */
export function RecordFromMessageDialog({
  message,
  onClose,
  projectId,
}: {
  message: ConversationMessage
  onClose: () => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const collections = useQuery({
    queryKey: collectionKeys.list(projectId, false),
    queryFn: () => listCollections(projectId),
  })
  const choices = (collections.data?.collections ?? []).filter((collection) => !collection.archived && !collection.actsLikeTask)
  const [picked, setPicked] = useState('')
  const collectionId = picked || choices[0]?.id || ''
  const [title, setTitle] = useState(() => firstLine(message.plainText))
  const [saySo, setSaySo] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [requestId] = useState(newRequestId)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!collectionId || busy) return
    setBusy(true)
    setError(null)
    void callOp(
      'record_create',
      { collection: collectionId, title: title.trim() || undefined, fromMessageId: message.id, saySo },
      { requestId },
    )
      .then((record) => {
        void queryClient.invalidateQueries({ queryKey: collectionKeys.allRecords(projectId, collectionId) })
        const name = choices.find((collection) => collection.id === collectionId)?.name ?? 'the collection'
        showToast({
          tone: 'success',
          title: `Added to ${name}`,
          action: {
            label: 'Open',
            onSelect: () => navigate(projectAreaHref(projectId, 'collections', collectionId, { record: record.id })),
          },
        })
        onClose()
      })
      .catch((reason: unknown) => setError(`The record wasn’t made. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description="The record links back to this message."
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!collectionId} type="submit" variant="primary">
            Make record
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Make a record from this message"
      width={480}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {collections.isPending ? null : choices.length ? (
        <SelectField label="Collection" onChange={(event) => setPicked(event.target.value)} value={collectionId}>
          {choices.map((collection) => (
            <option key={collection.id} value={collection.id}>
              {collection.name}
            </option>
          ))}
        </SelectField>
      ) : (
        <Banner tone="info">This space has no collections to add records to yet.</Banner>
      )}
      <TextField data-autofocus label="Title" maxLength={300} onChange={(event) => setTitle(event.target.value)} value={title} />
      <Checkbox checked={saySo} label="Say so in the conversation" onChange={setSaySo} />
    </Dialog>
  )
}

export default RecordFromMessageDialog
