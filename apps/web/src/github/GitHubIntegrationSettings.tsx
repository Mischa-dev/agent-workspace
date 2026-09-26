import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
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
  MessageSkeleton,
  Row,
  RowList,
  StatusChip,
  Switch,
  TextField,
} from '../ui'
import { getGitHubAutomationSettings, getGitHubStatus, syncGitHubInstallation, updateGitHubAutomationSettings } from './api'

type GitHubStatus = Awaited<ReturnType<typeof getGitHubStatus>>

export function GitHubIntegrationSettings({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const queryKey = queryKeys.githubIntegration(projectId)
  const status = useQuery({
    queryKey,
    queryFn: () => getGitHubStatus(projectId),
  })

  if (status.isPending) return <MessageSkeleton label="Loading GitHub" rows={2} />
  if (status.isError) {
    return (
      <ErrorState action={<Button onClick={() => void status.refetch()}>Try again</Button>} title="GitHub didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }
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
          A host operator must install the GitHub App’s private credential files on the server before a Project can connect
          to GitHub.
        </p>
      </div>
    )
  }
  const installation = status.data.installation
  if (!installation) {
    return <GitHubInstallationSetup onVerified={(current) => queryClient.setQueryData(queryKey, current)} projectId={projectId} />
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
              GitHub App {installation.appId} · installation {installation.installationId}
            </p>
          </div>
          <StatusChip icon={connected ? 'circle-check' : 'triangle-alert'} tone={connected ? 'success' : 'warning'}>
            {connected ? 'Connected' : humanize(installation.state)}
          </StatusChip>
        </header>
        {installation.failureCategory && (
          <Banner tone="warning" title="GitHub needs attention.">
            {humanize(installation.failureCategory)}. Reverify the installation below, or ask the host operator to check it.
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
              value: installation.lastSuccessAt ? new Date(installation.lastSuccessAt).toLocaleString() : 'Never',
            },
            { label: 'GitHub API', value: <code className="aw-inline-code">{status.data.apiVersion}</code> },
          ]}
        />
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
                meta={`GitHub repository ${repository.githubRepositoryId}`}
                title={`${repository.owner}/${repository.name}`}
              />
            ))}
          </RowList>
        ) : (
          <p className="aw-quiet">No repositories are selected yet.</p>
        )}
        <p className="aw-quiet">Credential files, installation tokens and host paths never reach this page.</p>
      </div>

      <GitHubAutomationSettings projectId={projectId} />
      <GitHubReverify
        installationId={installation.installationId}
        onVerified={(current) => queryClient.setQueryData(queryKey, current)}
        projectId={projectId}
        repositoryIds={installation.repositories.map((repository) => repository.id)}
      />
    </div>
  )
}

function GitHubAutomationSettings({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const queryKey = queryKeys.githubAutomation(projectId)
  const settings = useQuery({
    queryKey,
    queryFn: () => getGitHubAutomationSettings(projectId),
  })
  const update = useMutation({
    mutationFn: (next: { pullRequestOpenEnabled: boolean; pullRequestMergeEnabled: boolean }) => {
      if (!settings.data) throw new Error('GitHub automation settings unavailable')
      return updateGitHubAutomationSettings(projectId, { expectedVersion: settings.data.version, ...next })
    },
    onSuccess: (current) => queryClient.setQueryData(queryKey, current),
    onError: () => void queryClient.invalidateQueries({ queryKey }),
  })
  if (settings.isPending) return <MessageSkeleton label="Loading work item automation" rows={1} />
  if (settings.isError) return <Banner tone="danger">Work item automation didn’t load.</Banner>
  return (
    <div className="aw-subsection">
      <h3 className="aw-subsection__title">Work item automation</h3>
      <div className="aw-card aw-switches">
        <Switch
          checked={settings.data.pullRequestOpenEnabled}
          description="Only work items linked to the pull request move."
          disabled={update.isPending}
          label="Move linked work items to Review when a pull request opens"
          onChange={(checked) =>
            update.mutate({ pullRequestOpenEnabled: checked, pullRequestMergeEnabled: settings.data.pullRequestMergeEnabled })
          }
        />
        <Switch
          checked={settings.data.pullRequestMergeEnabled}
          description="Automation never marks work done, and never reopens work that’s done or canceled."
          disabled={update.isPending}
          label="Move linked work items to Verification when the pull request merges"
          onChange={(checked) =>
            update.mutate({ pullRequestOpenEnabled: settings.data.pullRequestOpenEnabled, pullRequestMergeEnabled: checked })
          }
        />
      </div>
      {update.isError && <Banner tone="warning">Someone else changed these settings. The current values are shown.</Banner>}
    </div>
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
    return <p className="aw-quiet">No trusted repositories are registered yet. Add one on the Repositories tab first.</p>
  }
  return (
    <fieldset className="aw-choices" disabled={disabled}>
      <legend className="aw-choices__legend">Repositories it may access</legend>
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

function GitHubInstallationSetup({ onVerified, projectId }: { onVerified: (status: GitHubStatus) => void; projectId: string }) {
  const repositories = useQuery({
    queryKey: queryKeys.repositories(projectId),
    queryFn: () => listRepositories(projectId),
  })
  const [installationId, setInstallationId] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const [error, setError] = useState<string | null>(null)
  const verify = useMutation({
    mutationFn: () => syncGitHubInstallation(projectId, { installationId: Number(installationId), repositoryIds: selected }),
    onSuccess: onVerified,
    onError: (reason) => setError(integrationError(reason)),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setError(null)
    verify.mutate()
  }
  const available =
    repositories.data?.repositories.filter((repository) => repository.enabled && repository.trusted && !repository.archivedAt) ?? []

  return (
    <form aria-labelledby="github-setup-heading" className="aw-card aw-formcard aw-form" onSubmit={submit}>
      <header className="aw-profile__head">
        <span className="aw-row__tile">
          <Icon name="pull-request" size={16} />
        </span>
        <div className="aw-profile__titles">
          <h3 className="aw-profile__name" id="github-setup-heading">
            Connect a GitHub installation
          </h3>
          <p className="aw-profile__sub">
            The GitHub App is ready on the server. Enter its installation ID and choose which registered repositories it may reach.
          </p>
        </div>
        <StatusChip icon="clock" tone="warning">
          Ready to connect
        </StatusChip>
      </header>
      {repositories.isPending && <MessageSkeleton label="Loading registered repositories" rows={1} />}
      {repositories.isError && <Banner tone="danger">Registered repositories didn’t load.</Banner>}
      {repositories.isSuccess && (
        <>
          <TextField
            hint="The number at the end of the installation’s URL on GitHub."
            id="github-installation-id"
            inputMode="numeric"
            label="GitHub installation ID"
            mono
            onChange={(event) => setInstallationId(event.target.value)}
            value={installationId}
          />
          <RepositoryChoices available={available} onChange={setSelected} selected={selected} />
          {error && <Banner tone="danger">{error}</Banner>}
          <div className="aw-form__actions">
            <Button
              busy={verify.isPending}
              disabled={Number(installationId) <= 0 || selected.length === 0}
              icon="shield-check"
              type="submit"
              variant="primary"
            >
              Verify installation
            </Button>
          </div>
        </>
      )}
    </form>
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
  return (
    <div className="aw-subsection">
      <h3 className="aw-subsection__title">Change repositories</h3>
      <div className="aw-card aw-formcard aw-form">
        {repositories.isPending && <MessageSkeleton label="Loading registered repositories" rows={1} />}
        {repositories.isError && <Banner tone="danger">Registered repositories didn’t load.</Banner>}
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
            Reverify installation
          </Button>
        </div>
      </div>
    </div>
  )
}

function integrationError(reason: unknown) {
  return isApiError(reason) ? reason.message : 'GitHub couldn’t verify the installation. Check the ID and try again.'
}
