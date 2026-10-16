import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'

import { useProjectConversationSidebar } from '../conversations/hooks'
import { humanize } from '../projects/format'
import { listRepositories } from '../repositories/api'
import { Banner, Button, Icon, Row, RowList, SelectField } from '../ui'
import { useSetWorkItemPrimaryRepository, useSetWorkItemRepositoryLink, useSetWorkItemThreadLink } from './hooks'
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

export function TaskLinks({
  disabled,
  item,
}: {
  disabled: boolean
  item: WorkItem
}) {
  const conversations = useProjectConversationSidebar(item.projectId)
  const repositories = useQuery({
    queryKey: ['projects', item.projectId, 'repositories', 'work-item-picker'],
    queryFn: () => listRepositories(item.projectId),
    enabled: Boolean(item.projectId),
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

  return (
    <section aria-labelledby={`links-${item.id}`} className="aw-worksection">
      <header className="aw-worksection__head">
        <h2 className="aw-worksection__title" id={`links-${item.id}`}>
          <Icon name="link" size={16} />
          Links
        </h2>
      </header>

      <div className="aw-links">
        <div className="aw-links__group">
          <h3 className="aw-links__title">Threads</h3>
          {item.linkedThreads.length ? (
            <RowList label="Linked threads">
              {item.linkedThreads.map((thread) => (
                <Row
                  actions={
                    !disabled ? (
                      <Button
                        disabled={busy}
                        icon="unlink"
                        onClick={() =>
                          threadMutation.mutate(
                            { ...ids, threadId: thread.id, linked: false, idempotencyKey: crypto.randomUUID() },
                          )
                        }
                        size="sm"
                        variant="ghost"
                      >
                        Unlink
                      </Button>
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
          ) : (
            <p className="aw-quiet">No threads linked.</p>
          )}
          {!disabled ? (
            <form
              className="aw-inline-add"
              onSubmit={(event) => {
                event.preventDefault()
                if (!threadId) return
                threadMutation.mutate(
                  { ...ids, threadId, linked: true, idempotencyKey: crypto.randomUUID() },
                  { onSuccess: () => setThreadId('') },
                )
              }}
            >
              <SelectField aria-label="Thread to link" id={`thread-link-${item.id}`} onChange={(event) => setThreadId(event.target.value)} value={threadId}>
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
          ) : null}
        </div>

        <div className="aw-links__group">
          <h3 className="aw-links__title">Repositories</h3>
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
                              primaryMutation.mutate(
                                { ...ids, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() },
                                  )
                            }
                            size="sm"
                            variant="ghost"
                          >
                            Make primary
                          </Button>
                        ) : null}
                        <Button
                          disabled={busy}
                          icon="unlink"
                          onClick={() =>
                            repositoryMutation.mutate(
                              { ...ids, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() },
                              )
                          }
                          size="sm"
                          variant="ghost"
                        >
                          Unlink
                        </Button>
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
          ) : (
            <p className="aw-quiet">No repositories linked.</p>
          )}
          {!disabled ? (
            <form
              className="aw-inline-add"
              onSubmit={(event) => {
                event.preventDefault()
                if (!repositoryId) return
                repositoryMutation.mutate(
                  {
                    ...ids,
                    repositoryId,
                    expectedVersion: item.version,
                    role: repositoryRole,
                    idempotencyKey: crypto.randomUUID(),
                  },
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
                <option value="">Link a repository…</option>
                {availableRepositories.map((repository) => (
                  <option key={repository.id} value={repository.id}>
                    {repository.owner}/{repository.name}
                  </option>
                ))}
              </SelectField>
              <SelectField
                aria-label="Repository role"
                onChange={(event) => setRepositoryRole(event.target.value as 'primary' | 'related')}
                value={repositoryRole}
              >
                <option value="related">Related</option>
                <option value="primary">Primary</option>
              </SelectField>
              <Button disabled={!repositoryId || busy} size="sm" type="submit">
                Link
              </Button>
            </form>
          ) : null}
        </div>

        <div className="aw-links__group">
          <h3 className="aw-links__title">Pull requests</h3>
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
          ) : (
            <p className="aw-quiet">No pull requests linked. Runs link the ones they open.</p>
          )}
        </div>
      </div>
      {threadMutation.isError || repositoryMutation.isError || primaryMutation.isError ? (
        <Banner tone="warning">The links changed before that finished. They’re up to date now, so try again.</Banner>
      ) : null}
    </section>
  )
}
