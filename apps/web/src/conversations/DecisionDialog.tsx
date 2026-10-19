import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { memoryKeys, pin, recordDecision } from '../memory/api'
import { Banner, Button, Checkbox, Dialog, TextArea, TextField, describeError, showToast } from '../ui'
import type { ConversationMessage } from './types'

function firstLine(text: string) {
  const line = text.trim().split('\n')[0] ?? ''
  return line.length > 200 ? `${line.slice(0, 199)}…` : line
}

/** Record what a message settled as one of the project's decisions. */
export function DecisionDialog({
  conversationId,
  direct,
  message,
  onClose,
  projectId,
}: {
  conversationId: string
  direct: boolean
  message: ConversationMessage
  onClose: () => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [title, setTitle] = useState(firstLine(message.plainText))
  const [detail, setDetail] = useState('')
  const [pinIt, setPinIt] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    void recordDecision(projectId, { title: title.trim(), detail: detail.trim() || undefined, sourceMessageId: message.id })
      .then(async (decision) => {
        if (pinIt) await pin(conversationId, 'decision', decision.id).catch(() => undefined)
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: memoryKeys.decisions(projectId) }),
          queryClient.invalidateQueries({ queryKey: memoryKeys.pins(projectId, conversationId) }),
        ])
        showToast({
          tone: 'success',
          title: pinIt ? 'Decision saved and pinned' : 'Decision saved',
          action: direct
            ? undefined
            : {
                label: 'View',
                onSelect: () => navigate(`/projects/${encodeURIComponent(projectId)}/decisions#${encodeURIComponent(decision.id)}`),
              },
        })
        onClose()
      })
      .catch((reason: unknown) => setError(`The decision wasn’t saved. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      description="Decisions are kept with the project, so people and agents can find what was settled and why without rereading the conversation."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
            Save decision
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Save as decision"
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField label="What was decided" maxLength={300} onChange={(event) => setTitle(event.target.value)} value={title} />
      <TextArea label="Why (optional)" maxLength={20000} onChange={(event) => setDetail(event.target.value)} rows={4} value={detail} />
      <Checkbox checked={pinIt} label="Pin it to this conversation" onChange={setPinIt} />
    </Dialog>
  )
}
