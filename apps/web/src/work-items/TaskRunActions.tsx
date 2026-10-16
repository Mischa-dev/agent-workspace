import { useState, type FormEvent, type RefObject } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'

import { useProjectConversationSidebar } from '../conversations/hooks'
import { listRepositories } from '../repositories/api'
import { Banner, Button, Dialog, Icon, SelectField, TextArea } from '../ui'
import { buildOpenInCodexLink } from './codexDeepLink'
import { useWorkWithWorker } from './hooks'
import type { WorkItem } from './types'

export function OpenInCodex({ item, projectArchived }: { item: WorkItem; projectArchived: boolean }) {
  const result = projectArchived
    ? { href: null, reason: 'Restore this Project before opening a work item in Codex.' }
    : buildOpenInCodexLink(item, window.location.origin)
  const reasonId = `open-in-codex-reason-${item.id}`

  return (
    <section aria-labelledby={`open-in-codex-heading-${item.id}`} className="aw-sidecard">
      <h2 className="aw-sidecard__title" id={`open-in-codex-heading-${item.id}`}>
        Work on it locally
      </h2>
      <p className="aw-sidecard__text">Opens Codex on your computer with this work item ready. Nothing starts until you send it there.</p>
      {result.href ? (
        <a className="aw-btn aw-btn--secondary aw-btn--md" href={result.href}>
          <Icon name="terminal" />
          <span className="aw-btn__label">Open in Codex</span>
        </a>
      ) : (
        <>
          <Button aria-describedby={reasonId} disabled icon="terminal">
            Open in Codex
          </Button>
          <p className="aw-sidecard__note" id={reasonId}>
            {result.reason}
          </p>
        </>
      )}
    </section>
  )
}

export function StartRunDialog({
  item,
  onClose,
  returnFocus,
}: {
  item: WorkItem
  onClose: () => void
  returnFocus: RefObject<HTMLButtonElement | null>
}) {
  const navigate = useNavigate()
  const conversations = useProjectConversationSidebar(item.projectId)
  const repositories = useQuery({
    queryKey: ['projects', item.projectId, 'repositories', 'worker-picker'],
    queryFn: () => listRepositories(item.projectId),
    enabled: Boolean(item.projectId),
  })
  const mutation = useWorkWithWorker()
  const [channelId, setChannelId] = useState('')
  const [repositoryId, setRepositoryId] = useState('')
  const [prompt, setPrompt] = useState('')
  const channels = (conversations.data?.channels ?? []).filter((channel) => !channel.archivedAt)
  const availableRepositories = (repositories.data?.repositories ?? []).filter(
    (repository) => repository.enabled && repository.trusted && repository.preparationState === 'ready' && !repository.archivedAt,
  )

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!channelId || !prompt.trim() || mutation.isPending) return
    mutation.mutate(
      {
        projectId: item.projectId,
        workItemId: item.id,
        expectedVersion: item.version,
        channelId,
        prompt: prompt.trim(),
        repositoryId: repositoryId || undefined,
        idempotencyKey: crypto.randomUUID(),
      },
      {
        onSuccess: (response) => {
          navigate(`/projects/${encodeURIComponent(item.projectId)}/threads/${encodeURIComponent(response.threadId)}`)
        },
      },
    )
  }

  return (
    <Dialog
      description={`Codex works in a new thread in the channel you pick. It gets ${item.key} and your instructions, nothing else.`}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={mutation.isPending} disabled={!channelId || !prompt.trim()} icon="play" type="submit" variant="primary">
            Start run
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      returnFocus={returnFocus}
      title="Start a run"
      width={560}
    >
      <div className="aw-form__grid">
        <SelectField id="start-run-channel" label="Channel" onChange={(event) => setChannelId(event.target.value)} required value={channelId}>
          <option value="">Choose a channel</option>
          {channels.map((channel) => (
            <option key={channel.id} value={channel.id}>
              #{channel.name}
            </option>
          ))}
        </SelectField>
        <SelectField id="start-run-repository" label="Repository" onChange={(event) => setRepositoryId(event.target.value)} value={repositoryId}>
          <option value="">Pick automatically</option>
          {availableRepositories.map((repository) => (
            <option key={repository.id} value={repository.id}>
              {repository.owner}/{repository.name}
            </option>
          ))}
        </SelectField>
      </div>
      <TextArea
        data-autofocus
        id="start-run-instructions"
        label="Instructions"
        maxLength={18000}
        onChange={(event) => setPrompt(event.target.value)}
        placeholder="Describe the change or investigation you want."
        required
        rows={5}
        value={prompt}
      />
      {mutation.isError ? <Banner tone="danger">{mutation.error.message}</Banner> : null}
    </Dialog>
  )
}
