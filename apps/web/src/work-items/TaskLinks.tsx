import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'

import { useProjectConversationSidebar } from '../conversations/hooks'
import { humanize } from '../projects/format'
import { listRepositories } from '../repositories/api'
import { Banner, Button, ConfirmDialog, Icon, IconButton, Row, RowList, SelectField, describeError } from '../ui'
import {
  useMergeWorkItemPullRequest,
  useSetWorkItemPrimaryRepository,
  useSetWorkItemPullRequestLink,
  useSetWorkItemRepositoryLink,
  useSetWorkItemThreadLink,
} from './hooks'
import { TaskSection } from './TaskSection'
import type { PullRequestCheck, WorkItem } from './types'

const CHECKS: Record<string, string> = { success: 'checks passed', failure: 'checks failing', pending: 'checks running' }
const REVIEW: Record<string, string> = { approved: 'approved', changes_requested: 'changes requested', pending: 'waiting for review' }
const MERGE: Record<string, string> = { ready: 'ready to merge', blocked: 'can’t merge yet' }

/** One plain line, like "Open · checks failing · approved"; states nobody knows yet are left out. */
function pullRequestSummary(pullRequest: WorkItem['linkedPullRequests'][number]) {
  const open = !pullRequest.draft && pullRequest.state === 'open'
  const parts = [
    pullRequest.draft ? 'Draft' : humanize(pullRequest.state),
    CHECKS[pullRequest.checksState],
    REVIEW[pullRequest.reviewState],
    open ? MERGE[pullRequest.mergeabilityState] : undefined,
    pullRequest.confirmed ? undefined : 'suggested because it names this task',
    pullRequest.syncState === 'failed' ? 'couldn’t refresh' : undefined,
  ]
  return parts.filter(Boolean).join(' · ')
}

const FAILED = new Set(['failure', 'timed_out', 'startup_failure', 'action_required', 'cancelled'])

/** How one check stands, in words and as an icon. */
function checkStanding(check: PullRequestCheck): { icon: 'check' | 'x' | 'loader' | 'clock'; label: string; tone: string } {
  if (check.status !== 'completed') return check.status === 'in_progress' ? { icon: 'loader', label: 'running', tone: 'agent' } : { icon: 'clock', label: 'queued', tone: 'muted' }
  if (check.conclusion && FAILED.has(check.conclusion)) return { icon: 'x', label: humanize(check.conclusion).toLocaleLowerCase(), tone: 'danger' }
  if (check.conclusion === 'success') return { icon: 'check', label: 'passed', tone: 'success' }
  return { icon: 'check', label: humanize(check.conclusion ?? 'done').toLocaleLowerCase(), tone: 'muted' }
}

const SHOWN_CHECKS = 5

/** A pull request's branch and checks, under its line on the task (AWD-59). */
function PullRequestDetails({ pullRequest }: { pullRequest: WorkItem['linkedPullRequests'][number] }) {
  const [all, setAll] = useState(false)
  const checks = all ? pullRequest.checks : pullRequest.checks.slice(0, SHOWN_CHECKS)
  const hidden = pullRequest.checks.length - checks.length
  const from = [
    pullRequest.headRef && pullRequest.baseRef ? `${pullRequest.headRef} → ${pullRequest.baseRef}` : undefined,
    pullRequest.authorLogin ? `by ${pullRequest.authorLogin}` : undefined,
    pullRequest.requestedReviewers.length ? `review asked of ${pullRequest.requestedReviewers.join(', ')}` : undefined,
  ].filter(Boolean)
  if (!from.length && !checks.length) return null
  return (
    <div className="aw-pr">
      {from.length ? <p className="aw-pr__from">{from.join(' · ')}</p> : null}
      {checks.length ? (
        <ul aria-label={`Checks on #${pullRequest.number}`} className="aw-pr__checks">
          {checks.map((check, index) => {
            const standing = checkStanding(check)
            return (
              <li className={`aw-pr__check aw-pr__check--${standing.tone}`} key={`${check.name}-${index}`}>
                <Icon name={standing.icon} size={14} />
                {check.url ? (
                  <a href={check.url} rel="noreferrer" target="_blank">
                    {check.name}
                  </a>
                ) : (
                  <span>{check.name}</span>
                )}
                <span>{standing.label}</span>
              </li>
            )
          })}
        </ul>
      ) : null}
      {hidden > 0 ? (
        <Button onClick={() => setAll(true)} size="sm" variant="ghost">
          Show {hidden} more {hidden === 1 ? 'check' : 'checks'}
        </Button>
      ) : null}
    </div>
  )
}

/**
 * The threads, repositories and pull requests linked to a task. Only kinds
 * with something linked show; adding reveals the fields to link a thread or
 * a repository. Pull requests are linked by the runs that open them, or
 * suggested when they name the task's key (confirm or dismiss those). A
 * layout can hide the threads (no chat) or the code (no pull requests).
 */
export function TaskLinks({
  adding,
  code = true,
  disabled,
  item,
  onAdding,
  threads = true,
}: {
  adding: boolean
  code?: boolean
  disabled: boolean
  item: WorkItem
  onAdding: (adding: boolean) => void
  threads?: boolean
}) {
  const conversations = useProjectConversationSidebar(item.projectId)
  const repositories = useQuery({
    queryKey: ['projects', item.projectId, 'repositories', 'work-item-picker'],
    queryFn: () => listRepositories(item.projectId),
    enabled: Boolean(item.projectId) && adding && !disabled && code,
  })
  const threadMutation = useSetWorkItemThreadLink()
  const repositoryMutation = useSetWorkItemRepositoryLink()
  const primaryMutation = useSetWorkItemPrimaryRepository()
  const pullRequestMutation = useSetWorkItemPullRequestLink()
  const mergeMutation = useMergeWorkItemPullRequest()
  // The pull request asked to merge, while its confirmation is open.
  const [merging, setMerging] = useState<WorkItem['linkedPullRequests'][number] | null>(null)
  // Merged here and waiting for the sync to say so.
  const [merged, setMerged] = useState<string[]>([])
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
  const busy = threadMutation.isPending || repositoryMutation.isPending || primaryMutation.isPending || pullRequestMutation.isPending
  const failure = threadMutation.error ?? repositoryMutation.error ?? primaryMutation.error ?? pullRequestMutation.error
  const ids = { projectId: item.projectId, workItemId: item.id }
  const open = adding && !disabled
  const linkedThreads = threads ? item.linkedThreads : []
  const linkedRepositories = code ? item.linkedRepositories : []
  const linkedPullRequests = code ? item.linkedPullRequests : []
  if (!linkedThreads.length && !linkedRepositories.length && !linkedPullRequests.length && !open) return null

  return (
    <TaskSection
      actions={
        disabled ? null : open ? (
          <Button onClick={() => onAdding(false)} size="sm" variant="ghost">
            Done
          </Button>
        ) : (
          <IconButton
            icon="plus"
            label={threads && code ? 'Link a thread or repository' : threads ? 'Link a thread' : 'Link a repository'}
            onClick={() => onAdding(true)}
            size="sm"
          />
        )
      }
      id={`links-${item.id}`}
      title="Links"
    >
      <div className="aw-links">
        {linkedThreads.length ? (
          <RowList label="Linked threads">
            {linkedThreads.map((thread) => (
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

        {linkedRepositories.length ? (
          <RowList label="Linked repositories">
            {linkedRepositories.map((repository) => (
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

        {linkedPullRequests.length ? (
          <RowList label="Linked pull requests">
            {linkedPullRequests.map((pullRequest) => (
              <Row
                actions={
                  item.capabilities.mergePullRequests &&
                  pullRequest.confirmed &&
                  pullRequest.state === 'open' &&
                  !pullRequest.draft ? (
                    merged.includes(pullRequest.id) ? (
                      <Button disabled size="sm" variant="ghost">
                        Merged
                      </Button>
                    ) : (
                      <Button
                        disabled={mergeMutation.isPending}
                        onClick={() => {
                          mergeMutation.reset()
                          setMerging(pullRequest)
                        }}
                        size="sm"
                        variant="ghost"
                      >
                        Merge
                      </Button>
                    )
                  ) : !disabled && !pullRequest.confirmed ? (
                    <>
                      <Button
                        disabled={busy}
                        onClick={() =>
                          pullRequestMutation.mutate({
                            ...ids,
                            pullRequestId: pullRequest.id,
                            linked: true,
                            expectedVersion: item.version,
                            idempotencyKey: crypto.randomUUID(),
                          })
                        }
                        size="sm"
                        variant="ghost"
                      >
                        Link
                      </Button>
                      <IconButton
                        disabled={busy}
                        icon="x"
                        label={`Dismiss #${pullRequest.number}`}
                        onClick={() =>
                          pullRequestMutation.mutate({
                            ...ids,
                            pullRequestId: pullRequest.id,
                            linked: false,
                            expectedVersion: item.version,
                            idempotencyKey: crypto.randomUUID(),
                          })
                        }
                        size="sm"
                      />
                    </>
                  ) : null
                }
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
              >
                <PullRequestDetails pullRequest={pullRequest} />
              </Row>
            ))}
          </RowList>
        ) : null}

        <ConfirmDialog
          busy={mergeMutation.isPending}
          confirmLabel="Merge"
          error={mergeMutation.error ? describeError(mergeMutation.error).message : null}
          onClose={() => setMerging(null)}
          onConfirm={() => {
            if (!merging) return
            const pullRequest = merging
            mergeMutation.mutate(
              { ...ids, pullRequestId: pullRequest.id, expectedHeadSha: pullRequest.headSha },
              {
                onSuccess: () => {
                  setMerged((current) => [...current, pullRequest.id])
                  setMerging(null)
                },
              },
            )
          }}
          open={merging !== null}
          title={merging ? `Merge #${merging.number}?` : 'Merge'}
          tone="primary"
        >
          {merging
            ? `It merges ${merging.headRef || 'its branch'} into ${merging.baseRef || 'its base'} on GitHub at the commit shown here. Linked tasks move once GitHub confirms it.`
            : null}
        </ConfirmDialog>

        {open ? (
          <div className="aw-links__add">
            {threads ? (
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
            ) : null}
            {code ? (
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
                  autoFocus={!threads}
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
            ) : null}
          </div>
        ) : null}
      </div>
      {failure ? (
        describeError(failure).stale ? (
          <Banner tone="warning">The links changed before that finished. They’re up to date now, so try again.</Banner>
        ) : (
          <Banner tone="danger">The link wasn’t saved. {describeError(failure).message}</Banner>
        )
      ) : null}
    </TaskSection>
  )
}
