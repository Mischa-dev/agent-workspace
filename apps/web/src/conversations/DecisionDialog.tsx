import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { memoryKeys, pin, recordDecision } from '../memory/api'
import type { Project } from '../projects/types'
import { Banner, Button, Checkbox, Dialog, SelectField, TextArea, TextField, describeError, showToast } from '../ui'
import type { ConversationMessage } from './types'

function firstLine(text: string) {
  const line = text.trim().split('\n')[0] ?? ''
  return line.length > 200 ? `${line.slice(0, 199)}…` : line
}

/**
 * Record what a message settled as one of a project's decisions. A channel's
 * goes in its own project, linked to the message; a DM's goes in a project
 * the person picks, without a link, since that project's members can't open
 * the DM.
 */
export function DecisionDialog({
  conversationId,
  message,
  onClose,
  projectId,
  projects,
}: {
  conversationId: string
  message: ConversationMessage
  onClose: () => void
  projectId: string
  /** For a DM: the projects to pick from. */
  projects: Project[] | null
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const direct = projects !== null
  const [title, setTitle] = useState(firstLine(message.plainText))
  const [detail, setDetail] = useState('')
  const [pinIt, setPinIt] = useState(true)
  const [target, setTarget] = useState(() => projects?.[0]?.id ?? projectId)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const pinned = pinIt && !direct
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || !target || busy) return
    setBusy(true)
    setError(null)
    void recordDecision(target, {
      title: title.trim(),
      detail: detail.trim() || undefined,
      sourceMessageId: direct ? undefined : message.id,
    })
      .then(async (decision) => {
        if (pinned) await pin(conversationId, 'decision', decision.id).catch(() => undefined)
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: memoryKeys.decisions(target) }),
          queryClient.invalidateQueries({ queryKey: memoryKeys.pins(projectId, conversationId) }),
        ])
        const place = direct ? projects.find((project) => project.id === target)?.name : null
        showToast({
          tone: 'success',
          title: pinned ? 'Decision saved and pinned' : place ? `Decision saved in ${place}` : 'Decision saved',
          action: {
            label: 'View',
            onSelect: () => navigate(`/projects/${encodeURIComponent(target)}/decisions#${encodeURIComponent(decision.id)}`),
          },
        })
        onClose()
      })
      .catch((reason: unknown) => setError(`The decision wasn’t saved. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      description={
        direct
          ? 'Decisions are kept with a project, so its people and agents can find what was settled and why.'
          : 'Decisions are kept with the project, so people and agents can find what was settled and why without rereading the conversation.'
      }
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
      {projects ? (
        <SelectField
          hint="Its members will see the decision, but not this conversation."
          label="Project"
          onChange={(event) => setTarget(event.target.value)}
          value={target}
        >
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </SelectField>
      ) : null}
      <TextField label="What was decided" maxLength={300} onChange={(event) => setTitle(event.target.value)} value={title} />
      <TextArea label="Why (optional)" maxLength={20000} onChange={(event) => setDetail(event.target.value)} rows={4} value={detail} />
      {direct ? null : <Checkbox checked={pinIt} label="Pin it to this conversation" onChange={setPinIt} />}
    </Dialog>
  )
}
