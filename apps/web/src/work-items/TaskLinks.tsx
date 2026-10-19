import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'

import { useProjectConversationSidebar } from '../conversations/hooks'
import { humanize } from '../projects/format'
import { listRepositories } from '../repositories/api'
import { Banner, Button, Icon, IconButton, Row, RowList, SelectField } from '../ui'
import { useSetWorkItemPrimaryRepository, useSetWorkItemRepositoryLink, useSetWorkItemThreadLink } from './hooks'
import { TaskSection } from './TaskSection'
import type { WorkItem } from './types'

function pullRequestSummary(pullRequest: WorkItem['linkedPullRequests'][number]) {
  const parts = [
    pullRequest.draft ? 'Draft' : humanize(pullRequest.state),
    `checks ${humanize(pullRequest.checksState).toLocaleLowerCase()}`,
    `review ${humanize(pullRequest.reviewState).toLocaleLowerCase()}`,
    humanize(pullRequest.mergeabilityState).toLocaleLowerCase(),
  ]
  if (!pullRequest.confirmed) parts.push('needs confirmation')
  if (pullRequest.syncState === 'failed') {
    parts.push(`sync failed: ${humanize(pullRequest.syncFailureCategory ?? 'unknown').toLocaleLowerCase()}`)
  }
  if (pullRequest.automationState !== 'not_evaluated') {
    parts.push(
      `automation ${humanize(pullRequest.automationState).toLocaleLowerCase()}${
        pullRequest.automationReason ? `: ${humanize(pullRequest.automationReason).toLocaleLowerCase()}` : ''
      }`,
    )
  }
  return parts.join(' · ')
}

/**
 * The threads, repositories and pull requests linked to a task. Only kinds
 * with something linked show; adding reveals the fields to link a thread or
 * a repository. Pull requests are linked by the runs that open them.
 */
export function TaskLinks({
  adding,
  disabled,
  item,
  onAdding,
}: {
  adding: boolean
  disabled: boolean
  item: WorkItem
  onAdding: (adding: boolean) => void
}) {
  const conversations = useProjectConversationSidebar(item.projectId)
  const repositories = useQuery({
    queryKey: ['projects', item.projectId, 'repositories', 'work-item-picker'],
    queryFn: () => listRepositories(item.projectId),
    enabled: Boolean(item.projectId) && adding && !disabled,
  })
  const threadMutation = useSetWorkItemThreadLink()
  const repositoryMutation = useSetWorkItemRepositoryLink()
  const primaryMutation = useSetWorkItemPrimaryRepository()
  const [threadId, setThreadId] = useState('')
  const [repositoryId, setRepositoryId] = useState('')
  const [repositoryRole, setRepositoryRole] = useState<'primary' | 'related'>('related')
  const availableThreads = (conversations.data?.channels ?? []).flatMap((channel) =>
    channel.threads
      .filter((thread) => !thread.archivedAt && !item.linkedThreads.some((link) => link.id === thread.id))
      .map((thread) => ({ ...thread, channelName: channel.name })),
  )
  const availableRepositories = (repositories.data?.repositories ?? []).filter(
    (repository) => !repository.archivedAt && !item.linkedRepositories.some((link) => link.id === repository.id),
  )
  const busy = threadMutation.isPending || repositoryMutation.isPending || primaryMutation.isPending
  const ids = { projectId: item.projectId, workItemId: item.id }
  const open = adding && !disabled
  if (!item.linkedThreads.length && !item.linkedRepositories.length && !item.linkedPullRequests.length && !open) return null

  return (
    <TaskSection
      actions={
        disabled ? null : open ? (
          <Button onClick={() => onAdding(false)} size="sm" variant="ghost">
            Done
          </Button>
        ) : (
          <IconButton icon="plus" label="Link a thread or repository" onClick={() => onAdding(true)} size="sm" />
        )
      }
      id={`links-${item.id}`}
      title="Links"
    >
      <div className="aw-links">
        {item.linkedThreads.length ? (
          <RowList label="Linked threads">
            {item.linkedThreads.map((thread) => (
              <Row
                actions={
                  !disabled ? (
                    <IconButton
                      disabled={busy}
                      icon="unlink"
                      label={`Unlink ${thread.name}`}
                      onClick={() => threadMutation.mutate({ ...ids, threadId: thread.id, linked: false, idempotencyKey: crypto.randomUUID() })}
                      size="sm"
                    />
                  ) : null
                }
                key={thread.id}
                leading={
                  <span className="aw-row__tile">
                    <Icon name="messages" size={16} />
                  </span>
                }
                meta={`#${thread.parentChannelName}${thread.newestRunState ? ` · run ${humanize(thread.newestRunState).toLocaleLowerCase()}` : ''}${
                  thread.archivedAt ? ' · archived' : ''
                }`}
                title={<Link to={thread.href}>{thread.name}</Link>}
              />
            ))}
          </RowList>
        ) : null}

        {item.linkedRepositories.length ? (
          <RowList label="Linked repositories">
            {item.linkedRepositories.map((repository) => (
              <Row
                actions={
                  !disabled ? (
                    <>
                      {repository.role !== 'primary' && repository.available ? (
                        <Button
                          disabled={busy}
                          onClick={() =>
                            primaryMutation.mutate({ ...ids, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() })
                          }
                          size="sm"
                          variant="ghost"
                        >
                          Make primary
                        </Button>
                      ) : null}
                      <IconButton
                        disabled={busy}
                        icon="unlink"
                        label={`Unlink ${repository.owner}/${repository.name}`}
                        onClick={() =>
                          repositoryMutation.mutate({ ...ids, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() })
                        }
                        size="sm"
                      />
                    </>
                  ) : null
                }
                key={repository.id}
                leading={
                  <span className="aw-row__tile">
                    <Icon name="repo" size={16} />
                  </span>
                }
                meta={`${repository.role === 'primary' ? 'Primary' : 'Related'}${
                  repository.available ? '' : ` · ${humanize(repository.preparationState).toLocaleLowerCase()}`
                }${repository.archivedAt ? ' · archived' : ''}`}
                title={
                  <Link to={repository.href}>
                    {repository.owner}/{repository.name}
                  </Link>
                }
              />
            ))}
          </RowList>
        ) : null}

        {item.linkedPullRequests.length ? (
          <RowList label="Linked pull requests">
            {item.linkedPullRequests.map((pullRequest) => (
              <Row
                key={pullRequest.id}
                leading={
                  <span className="aw-row__tile">
                    <Icon name="pull-request" size={16} />
                  </span>
                }
                meta={pullRequestSummary(pullRequest)}
                title={
                  <a href={pullRequest.url} rel="noreferrer" target="_blank">
                    #{pullRequest.number} {pullRequest.title}
                  </a>
                }
              />
            ))}
          </RowList>
        ) : null}

        {open ? (
          <div className="aw-links__add">
            <form
              className="aw-inline-add"
              onSubmit={(event) => {
                event.preventDefault()
                if (!threadId) return
                threadMutation.mutate({ ...ids, threadId, linked: true, idempotencyKey: crypto.randomUUID() }, { onSuccess: () => setThreadId('') })
              }}
            >
              <SelectField aria-label="Thread to link" autoFocus id={`thread-link-${item.id}`} onChange={(event) => setThreadId(event.target.value)} value={threadId}>
                <option value="">Link a thread…</option>
                {availableThreads.map((thread) => (
                  <option key={thread.id} value={thread.id}>
                    #{thread.channelName} / {thread.name}
                  </option>
                ))}
              </SelectField>
              <Button disabled={!threadId || busy} size="sm" type="submit">
                Link
              </Button>
            </form>
            <form
              className="aw-inline-add"
              onSubmit={(event) => {
                event.preventDefault()
                if (!repositoryId) return
                repositoryMutation.mutate(
                  { ...ids, repositoryId, expectedVersion: item.version, role: repositoryRole, idempotencyKey: crypto.randomUUID() },
                  { onSuccess: () => setRepositoryId('') },
                )
              }}
            >
              <SelectField
                aria-label="Repository to link"
                id={`repository-link-${item.id}`}
                onChange={(event) => setRepositoryId(event.target.value)}
                value={repositoryId}
              >
                <option value="">{repositories.isPending ? 'Loading repositories…' : 'Link a repository…'}</option>
                {availableRepositories.map((repository) => (
                  <option key={repository.id} value={repository.id}>
                    {repository.owner}/{repository.name}
                  </option>
                ))}
              </SelectField>
              <SelectField aria-label="Repository role" onChange={(event) => setRepositoryRole(event.target.value as 'primary' | 'related')} value={repositoryRole}>
                <option value="related">Related</option>
                <option value="primary">Primary</option>
              </SelectField>
              <Button disabled={!repositoryId || busy} size="sm" type="submit">
                Link
              </Button>
            </form>
          </div>
        ) : null}
      </div>
      {threadMutation.isError || repositoryMutation.isError || primaryMutation.isError ? (
        <Banner tone="warning">The links changed before that finished. They’re up to date now, so try again.</Banner>
      ) : null}
    </TaskSection>
  )
}
