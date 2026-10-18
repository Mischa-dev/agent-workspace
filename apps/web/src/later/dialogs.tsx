import { useMemo, useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { createIdempotencyKey } from '../conversations/composerModel'
import { useProjectConversationSidebar } from '../conversations/hooks'
import type { ConversationMessage } from '../conversations/types'
import { conversationTitle, useDirectOverview } from '../direct/api'
import { useProjects } from '../projects/hooks'
import { Banner, Button, Dialog, SelectField, TextArea, TextField, showToast } from '../ui'
import { createWorkItem } from '../work-items/api'
import { forwardMessage } from './api'

function failure(reason: unknown, fallback: string) {
  return isApiError(reason) ? reason.message : reason instanceof Error ? reason.message : fallback
}

type Destination = { id: string; label: string; group: string }

/** Forward a message to a channel, DM or group you can post in. The quote follows the original's visibility. */
export function ForwardDialog({
  currentConversationId,
  currentProjectId,
  message,
  onClose,
}: {
  currentConversationId: string
  currentProjectId: string
  message: ConversationMessage
  onClose: () => void
}) {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const projects = useProjects()
  const direct = useDirectOverview()
  const choices = useMemo(
    () => (projects.data?.projects ?? []).filter((project) => !project.archivedAt),
    [projects.data?.projects],
  )
  const [projectId, setProjectId] = useState(() =>
    choices.some((project) => project.id === currentProjectId) ? currentProjectId : '',
  )
  const project = projectId || choices[0]?.id || ''
  const sidebar = useProjectConversationSidebar(project)
  const [target, setTarget] = useState('')
  const [comment, setComment] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [key] = useState(createIdempotencyKey)

  const destinations = useMemo<Destination[]>(() => {
    const list: Destination[] = []
    for (const channel of sidebar.data?.channels ?? []) {
      if (channel.archivedAt || channel.id === currentConversationId) continue
      list.push({ id: channel.id, label: `#${channel.name}`, group: 'Channels' })
    }
    for (const conversation of direct.data?.conversations ?? []) {
      if (conversation.id === currentConversationId) continue
      list.push({ id: conversation.id, label: conversationTitle(conversation, session.user.actorId), group: conversation.kind === 'group' ? 'Groups' : 'Direct messages' })
    }
    return list
  }, [currentConversationId, direct.data?.conversations, session.user.actorId, sidebar.data?.channels])
  const chosen = destinations.find((destination) => destination.id === target) ?? null

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!chosen) {
      setError('Pick where to forward it.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await forwardMessage({ messageId: message.id, conversationId: chosen.id, text: comment, idempotencyKey: key })
      onClose()
      const href = chosen.group === 'Channels'
        ? `/projects/${encodeURIComponent(project)}/channels/${encodeURIComponent(chosen.id)}`
        : `/messages/${encodeURIComponent(chosen.id)}`
      showToast({ tone: 'success', title: `Forwarded to ${chosen.label}`, action: { label: 'Open', onSelect: () => navigate(href) } })
    } catch (reason) {
      setError(failure(reason, 'It wasn’t forwarded. Try again.'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description="People there see the original only if they can see where it was said."
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} disabled={!chosen} icon="send" type="submit" variant="primary">
            Forward
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title="Forward message"
      width={480}
    >
      <blockquote className="aw-forward">
        <div className="aw-forward__meta">
          <span className="aw-forward__author">{message.author.displayName}</span>
        </div>
        <p className="aw-forward__text">{message.plainText.slice(0, 400)}</p>
      </blockquote>
      {choices.length > 1 ? (
        <SelectField
          label="Channels from"
          onChange={(event) => {
            setProjectId(event.target.value)
            setTarget('')
          }}
          value={project}
        >
          {choices.map((choice) => (
            <option key={choice.id} value={choice.id}>
              {choice.name}
            </option>
          ))}
        </SelectField>
      ) : null}
      <SelectField data-autofocus label="Forward to" onChange={(event) => setTarget(event.target.value)} value={target}>
        <option value="">Choose a conversation</option>
        {['Channels', 'Direct messages', 'Groups'].map((group) => {
          const options = destinations.filter((destination) => destination.group === group)
          return options.length ? (
            <optgroup key={group} label={group}>
              {options.map((destination) => (
                <option key={destination.id} value={destination.id}>
                  {destination.label}
                </option>
              ))}
            </optgroup>
          ) : null
        })}
      </SelectField>
      <TextArea
        label="Add a message (optional)"
        maxLength={4000}
        onChange={(event) => setComment(event.target.value)}
        rows={2}
        value={comment}
      />
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}

function firstLine(text: string) {
  const line = text.split('\n').find((part) => part.trim()) ?? ''
  return line.trim().slice(0, 240)
}

/** Turn a message into a task in this project, linked back to the message. */
export function MessageTaskDialog({
  message,
  onClose,
  projectId,
}: {
  message: ConversationMessage
  onClose: () => void
  projectId: string
}) {
  const navigate = useNavigate()
  const [title, setTitle] = useState(() => firstLine(message.plainText))
  const [description, setDescription] = useState(() => `From ${message.author.displayName}:\n\n> ${message.plainText.slice(0, 2_000).replace(/\n/g, '\n> ')}`)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [key] = useState(createIdempotencyKey)

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim()) return
    setBusy(true)
    setError(null)
    try {
      const item = await createWorkItem({
        projectId,
        title: title.trim(),
        description,
        priority: 'no_priority',
        sourceMessageId: message.id,
        idempotencyKey: key,
      })
      onClose()
      showToast({
        tone: 'success',
        title: `Created ${item.key}`,
        action: { label: 'Open', onSelect: () => navigate(`/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(item.id)}`) },
      })
    } catch (reason) {
      setError(failure(reason, 'The task wasn’t created. Try again.'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description="The task links back to this message."
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
            Create task
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title="Make a task"
      width={520}
    >
      <TextField data-autofocus label="Title" maxLength={240} onChange={(event) => setTitle(event.target.value)} required value={title} />
      <TextArea label="Description" maxLength={100000} onChange={(event) => setDescription(event.target.value)} rows={5} value={description} />
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
