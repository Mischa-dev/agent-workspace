import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { describeError } from '../app/mutations'
import { queryKeys } from '../api/queryKeys'
import { humanize } from '../projects/format'
import { listRepositories } from '../repositories/api'
import {
  Banner,
  Button,
  Checkbox,
  DetailList,
  ErrorState,
  Icon,
  Row,
  RowList,
  SettingsSkeleton,
  StatusChip,
  Switch,
  formatDateTime,
  formatFullDate,
  formatRelative,
} from '../ui'
import type { WorkItemStatus } from '../work-items/types'
import { getGitHubAutomationSettings, getGitHubStatus, syncGitHubInstallation, updateGitHubAutomationSettings } from './api'

type GitHubStatus = Awaited<ReturnType<typeof getGitHubStatus>>

export function GitHubIntegrationSettings({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const queryKey = queryKeys.githubIntegration(projectId)
  const status = useQuery({
    queryKey,
    queryFn: () => getGitHubStatus(projectId),
  })

  if (status.isPending) return <SettingsSkeleton label="Loading GitHub" />
  if (status.isError) return <ErrorState error={status.error} onRetry={() => void status.refetch()} title="GitHub didn’t load" />
  if (!status.data.configured) {
    return (
      <div className="aw-card aw-profile">
        <header className="aw-profile__head">
          <span className="aw-row__tile">
            <Icon name="pull-request" size={16} />
          </span>
          <div className="aw-profile__titles">
            <h3 className="aw-profile__name">GitHub App</h3>
          </div>
          <StatusChip icon="circle-minus">Not configured</StatusChip>
        </header>
        <p className="aw-quiet">
          Whoever runs this server has to set up the GitHub App on it before a project can connect to GitHub.
        </p>
      </div>
    )
  }
  const installation = status.data.installation
  if (!installation) {
    return (
      <p className="aw-quiet">
        GitHub isn’t linked to this project yet. Connect GitHub and add a repository above, and the connection shows here.
      </p>
    )
  }
  const connected = installation.state === 'available'

  return (
    <div className="aw-settings-stack">
      <article aria-labelledby="github-installation-heading" className="aw-card aw-profile">
        <header className="aw-profile__head">
          <span className="aw-row__tile">
            <Icon name="pull-request" size={16} />
          </span>
          <div className="aw-profile__titles">
            <h3 className="aw-profile__name" id="github-installation-heading">
              {installation.accountLogin}
            </h3>
            <p className="aw-profile__sub">
              {installation.targetType === 'Organization' ? 'GitHub organization' : 'GitHub account'}
            </p>
          </div>
          <StatusChip icon={connected ? 'circle-check' : 'triangle-alert'} tone={connected ? 'success' : 'warning'}>
            {connected ? 'Connected' : humanize(installation.state)}
          </StatusChip>
        </header>
        {installation.failureCategory && (
          <Banner tone="warning" title="GitHub needs attention.">
            {humanize(installation.failureCategory)}. Check the connection below, or ask whoever runs this server to look at it.
          </Banner>
        )}
        <DetailList
          items={[
            { label: 'Repository access', value: humanize(installation.repositorySelection) },
            {
              label: 'Permissions',
              value: `Metadata read · Contents write · Pull requests write · Checks read${
                installation.permissions.actions === 'read' ? ' · Actions read' : ''
              }`,
            },
            {
              label: 'Last verified',
              value: installation.lastSuccessAt ? (
                <time dateTime={installation.lastSuccessAt} title={formatFullDate(installation.lastSuccessAt)}>
                  {formatDateTime(installation.lastSuccessAt)}
                </time>
              ) : (
                'Never'
              ),
            },
          ]}
        />
        <details className="aw-disclosure">
          <summary>Details</summary>
          <DetailList
            items={[
              { label: 'GitHub App', value: <code className="aw-inline-code">{installation.appId}</code> },
              { label: 'Installation', value: <code className="aw-inline-code">{installation.installationId}</code> },
              { label: 'GitHub API', value: <code className="aw-inline-code">{status.data.apiVersion}</code> },
            ]}
          />
        </details>
      </article>

      <div className="aw-subsection">
        <h3 className="aw-subsection__title">Repositories it can reach</h3>
        {installation.repositories.length ? (
          <RowList label="Repositories GitHub can reach">
            {installation.repositories.map((repository) => (
              <Row
                actions={
                  repository.accessState === 'selected' ? (
                    <StatusChip icon="circle-check" tone="success">
                      Reachable
                    </StatusChip>
                  ) : (
                    <StatusChip icon="triangle-alert" tone="warning">
                      Missing on GitHub
                    </StatusChip>
                  )
                }
                key={repository.id}
                leading={
                  <span className="aw-row__tile">
                    <Icon name="repo" size={16} />
                  </span>
                }
                meta={
                  repository.lastVerifiedAt ? (
                    <span title={formatFullDate(repository.lastVerifiedAt)}>Checked {formatRelative(repository.lastVerifiedAt)}</span>
                  ) : null
                }
                title={`${repository.owner}/${repository.name}`}
              />
            ))}
          </RowList>
        ) : (
          <p className="aw-quiet">No repositories are selected yet.</p>
        )}
      </div>

      <GitHubReverify
        installationId={installation.installationId}
        onVerified={(current) => queryClient.setQueryData(queryKey, current)}
        projectId={projectId}
        repositoryIds={installation.repositories.map((repository) => repository.id)}
      />
    </div>
  )
}

const CATEGORY_WORDS = { review: 'review', verification: 'verification' } as const

/** The status a pull request moves linked tasks to, from the project's workflow. Moves only when exactly one fits. */
function targetStatus(statuses: WorkItemStatus[] | undefined, category: keyof typeof CATEGORY_WORDS) {
  const matching = (statuses ?? []).filter((status) => status.semanticCategory === category)
  const word = CATEGORY_WORDS[category]
  if (!statuses) return { name: word, note: null }
  if (matching.length === 1) return { name: matching[0].name, note: null }
  if (matching.length === 0) return { name: word, note: `The workflow has no ${word} status, so nothing moves yet.` }
  return {
    name: word,
    note: `The workflow has ${matching.length} ${word} statuses (${matching.map((status) => status.name).join(', ')}), so nothing moves until only one is left.`,
  }
}

/** Moving linked tasks when pull requests open and merge, to the statuses the project's workflow names. */
export function GitHubAutomationSettings({ projectId, statuses }: { projectId: string; statuses?: WorkItemStatus[] }) {
  const queryClient = useQueryClient()
  const queryKey = queryKeys.githubAutomation(projectId)
  const settings = useQuery({
    queryKey,
    queryFn: () => getGitHubAutomationSettings(projectId),
  })
  const update = useMutation({
    mutationFn: (next: { pullRequestOpenEnabled: boolean; pullRequestMergeEnabled: boolean }) => {
      if (!settings.data) throw new Error('Pull request automation hasn’t loaded yet.')
      return updateGitHubAutomationSettings(projectId, { expectedVersion: settings.data.version, ...next })
    },
    onSuccess: (current) => queryClient.setQueryData(queryKey, current),
    onError: (reason) => {
      if (isApiError(reason) && reason.status === 409) void queryClient.invalidateQueries({ queryKey })
    },
  })
  if (settings.isPending) return <SettingsSkeleton label="Loading pull request automation" sections={1} />
  if (settings.isError) {
    return (
      <Banner onRetry={() => void settings.refetch()} tone="danger">
        Pull request automation didn’t load.
      </Banner>
    )
  }
  const opened = targetStatus(statuses, 'review')
  const merged = targetStatus(statuses, 'verification')
  const conflict = isApiError(update.error) && update.error.status === 409
  return (
    <>
      <div className="aw-card aw-switches">
        <Switch
          checked={settings.data.pullRequestOpenEnabled}
          description={opened.note ?? 'Only tasks linked to the pull request move. Drafts don’t count until they’re ready.'}
          disabled={update.isPending}
          label={`Move linked tasks to ${opened.name} when a pull request opens`}
          onChange={(checked) =>
            update.mutate({ pullRequestOpenEnabled: checked, pullRequestMergeEnabled: settings.data.pullRequestMergeEnabled })
          }
        />
        <Switch
          checked={settings.data.pullRequestMergeEnabled}
          description={merged.note ?? 'Automation never marks a task done, and never reopens one that’s done or canceled.'}
          disabled={update.isPending}
          label={`Move linked tasks to ${merged.name} when the pull request merges`}
          onChange={(checked) =>
            update.mutate({ pullRequestOpenEnabled: settings.data.pullRequestOpenEnabled, pullRequestMergeEnabled: checked })
          }
        />
      </div>
      {update.isError ? (
        <Banner tone={conflict ? 'warning' : 'danger'}>
          {conflict ? 'Someone else changed these settings. The current values are shown.' : describeError(update.error).message}
        </Banner>
      ) : null}
    </>
  )
}

function RepositoryChoices({
  available,
  disabled,
  onChange,
  selected,
}: {
  available: { id: string; owner: string; name: string }[]
  disabled?: boolean
  onChange: (next: string[]) => void
  selected: string[]
}) {
  if (!available.length) {
    return <p className="aw-quiet">No trusted repositories yet. Add one from GitHub above first.</p>
  }
  return (
    <fieldset className="aw-choices" disabled={disabled}>
      <legend className="aw-choices__legend">Repositories it may use</legend>
      {available.map((repository) => (
        <Checkbox
          checked={selected.includes(repository.id)}
          key={repository.id}
          label={`${repository.owner}/${repository.name}`}
          onChange={(checked) =>
            onChange(checked ? [...selected, repository.id] : selected.filter((id) => id !== repository.id))
          }
        />
      ))}
    </fieldset>
  )
}

function GitHubReverify({
  installationId,
  onVerified,
  projectId,
  repositoryIds,
}: {
  installationId: number
  onVerified: (status: GitHubStatus) => void
  projectId: string
  repositoryIds: string[]
}) {
  const repositories = useQuery({
    queryKey: queryKeys.repositories(projectId),
    queryFn: () => listRepositories(projectId),
  })
  const [selected, setSelected] = useState(repositoryIds)
  const [error, setError] = useState<string | null>(null)
  const verify = useMutation({
    mutationFn: () => syncGitHubInstallation(projectId, { installationId, repositoryIds: selected }),
    onSuccess: onVerified,
    onError: (reason) => setError(integrationError(reason)),
  })
  const available =
    repositories.data?.repositories.filter((repository) => repository.enabled && repository.trusted && !repository.archivedAt) ?? []
  const changed = [...selected].sort().join() !== [...repositoryIds].sort().join()
  return (
    <div className="aw-subsection">
      <h3 className="aw-subsection__title">Connection</h3>
      <div className="aw-card aw-formcard aw-form">
        {repositories.isPending && <SettingsSkeleton label="Loading repositories" sections={1} />}
        {repositories.isError && (
          <Banner onRetry={() => void repositories.refetch()} tone="danger">
            Repositories didn’t load.
          </Banner>
        )}
        {repositories.isSuccess && (
          <RepositoryChoices available={available} disabled={verify.isPending} onChange={setSelected} selected={selected} />
        )}
        {error && <Banner tone="danger">{error}</Banner>}
        <div className="aw-form__actions">
          <Button
            busy={verify.isPending}
            disabled={selected.length === 0 || repositories.isPending}
            icon="retry"
            onClick={() => {
              setError(null)
              verify.mutate()
            }}
          >
            {changed ? 'Save and check connection' : 'Check connection'}
          </Button>
        </div>
      </div>
    </div>
  )
}

function integrationError(reason: unknown) {
  return isApiError(reason) ? reason.message : 'The connection to GitHub couldn’t be checked. Try again.'
}
