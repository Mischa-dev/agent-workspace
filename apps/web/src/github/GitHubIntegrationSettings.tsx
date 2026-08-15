import { FormEvent, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { listRepositories } from '../repositories/api'
import {
  getGitHubAutomationSettings,
  getGitHubStatus,
  syncGitHubInstallation,
  updateGitHubAutomationSettings,
} from './api'

export function GitHubIntegrationSettings({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const queryKey = queryKeys.githubIntegration(projectId)
  const status = useQuery({
    queryKey,
    queryFn: () => getGitHubStatus(projectId),
  })

  if (status.isPending) return <p>Loading GitHub integration…</p>
  if (status.isError) {
    return <p role="alert">GitHub integration status could not be loaded.</p>
  }
  if (!status.data.configured) {
    return (
      <div className="integration-empty-state">
        <span className="status-chip" data-state="unavailable">Not configured</span>
        <p>
          A host operator must install the private GitHub App credential files
          before a Project can connect an installation.
        </p>
      </div>
    )
  }
  const installation = status.data.installation
  if (!installation) {
    return <GitHubInstallationSetup onVerified={(current) => queryClient.setQueryData(queryKey, current)} projectId={projectId} />
  }

  return (
    <div className="github-integration-settings">
      <header className="integration-status-heading">
        <div>
          <h3>{installation.accountLogin}</h3>
          <p>
            GitHub App {installation.appId} · installation{' '}
            {installation.installationId}
          </p>
        </div>
        <span className="status-chip" data-state={installation.state}>
          {installation.state}
        </span>
      </header>
      <dl className="detail-list">
        <div>
          <dt>REST API</dt>
          <dd>{status.data.apiVersion}</dd>
        </div>
        <div>
          <dt>Repository access</dt>
          <dd>{installation.repositorySelection}</dd>
        </div>
        <div>
          <dt>Permissions</dt>
          <dd>
            Metadata read · Contents write · Pull requests write · Checks read
            {installation.permissions.actions === 'read' ? ' · Actions read' : ''}
          </dd>
        </div>
        <div>
          <dt>Last verified</dt>
          <dd>
            {installation.lastSuccessAt
              ? new Date(installation.lastSuccessAt).toLocaleString()
              : 'Never'}
          </dd>
        </div>
      </dl>
      {installation.failureCategory && (
        <p role="alert">
          GitHub needs attention: {installation.failureCategory.replaceAll('_', ' ')}.
        </p>
      )}
      <h4>Selected repositories</h4>
      {installation.repositories.length ? (
        <ul className="linked-record-list">
          {installation.repositories.map((repository) => (
            <li key={repository.id}>
              <div>
                <strong>{repository.owner}/{repository.name}</strong>
                <small>
                  {repository.accessState} · GitHub repository{' '}
                  {repository.githubRepositoryId}
                </small>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <p>No repositories are selected.</p>
      )}
      <p className="compact-note">
        Credential files, installation tokens, and host paths never appear in
        this browser projection.
      </p>
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
    mutationFn: (next: {
      pullRequestOpenEnabled: boolean
      pullRequestMergeEnabled: boolean
    }) => {
      if (!settings.data) throw new Error('GitHub automation settings unavailable')
      return updateGitHubAutomationSettings(projectId, {
        expectedVersion: settings.data.version,
        ...next,
      })
    },
    onSuccess: (current) => queryClient.setQueryData(queryKey, current),
    onError: () => void queryClient.invalidateQueries({ queryKey }),
  })
  if (settings.isPending) return <p>Loading GitHub automation…</p>
  if (settings.isError) return <p role="alert">GitHub automation settings could not be loaded.</p>
  return (
    <fieldset disabled={update.isPending}>
      <legend>Work Item automation</legend>
      <label>
        <input
          checked={settings.data.pullRequestOpenEnabled}
          onChange={(event) => update.mutate({
            pullRequestOpenEnabled: event.target.checked,
            pullRequestMergeEnabled: settings.data.pullRequestMergeEnabled,
          })}
          type="checkbox"
        />
        Move explicitly linked work to Review when a pull request opens
      </label>
      <label>
        <input
          checked={settings.data.pullRequestMergeEnabled}
          onChange={(event) => update.mutate({
            pullRequestOpenEnabled: settings.data.pullRequestOpenEnabled,
            pullRequestMergeEnabled: event.target.checked,
          })}
          type="checkbox"
        />
        Move explicitly linked eligible work to Verification when merged
      </label>
      <p className="compact-note">
        Automation never marks work Done and never reopens Done or Canceled work.
      </p>
      {update.isError && <p role="alert">The automation settings changed. Current values were reloaded.</p>}
    </fieldset>
  )
}

function GitHubInstallationSetup({
  onVerified,
  projectId,
}: {
  onVerified: (status: Awaited<ReturnType<typeof getGitHubStatus>>) => void
  projectId: string
}) {
  const repositories = useQuery({
    queryKey: queryKeys.repositories(projectId),
    queryFn: () => listRepositories(projectId),
  })
  const [installationId, setInstallationId] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const [error, setError] = useState<string | null>(null)
  const verify = useMutation({
    mutationFn: () => syncGitHubInstallation(projectId, {
      installationId: Number(installationId),
      repositoryIds: selected,
    }),
    onSuccess: onVerified,
    onError: (reason) => setError(integrationError(reason)),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setError(null)
    verify.mutate()
  }
  const available = repositories.data?.repositories.filter(
    (repository) => repository.enabled && repository.trusted && !repository.archivedAt,
  ) ?? []

  return (
    <div className="integration-empty-state">
      <span className="status-chip" data-state="pending">Host ready</span>
      <p>
        GitHub App credentials are available, but this Project has no verified
        installation yet. Enter the public installation ID and select only the
        registered repositories it may access.
      </p>
      {repositories.isPending && <p>Loading registered repositories…</p>}
      {repositories.isError && <p role="alert">Registered repositories could not be loaded.</p>}
      {repositories.isSuccess && (
        <form onSubmit={submit}>
          <div className="field">
            <label htmlFor="github-installation-id">GitHub installation ID</label>
            <input
              id="github-installation-id"
              inputMode="numeric"
              onChange={(event) => setInstallationId(event.target.value)}
              value={installationId}
            />
          </div>
          <fieldset>
            <legend>Selected repositories</legend>
            {available.map((repository) => (
              <label key={repository.id}>
                <input
                  checked={selected.includes(repository.id)}
                  onChange={(event) => setSelected((current) => event.target.checked
                    ? [...current, repository.id]
                    : current.filter((id) => id !== repository.id))}
                  type="checkbox"
                />
                {repository.owner}/{repository.name}
              </label>
            ))}
          </fieldset>
          {error && <p role="alert">{error}</p>}
          <button
            disabled={Number(installationId) <= 0 || selected.length === 0 || verify.isPending}
            type="submit"
          >
            {verify.isPending ? 'Verifying…' : 'Verify installation'}
          </button>
        </form>
      )}
    </div>
  )
}

function GitHubReverify({
  installationId,
  onVerified,
  projectId,
  repositoryIds,
}: {
  installationId: number
  onVerified: (status: Awaited<ReturnType<typeof getGitHubStatus>>) => void
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
  const available = repositories.data?.repositories.filter(
    (repository) => repository.enabled && repository.trusted && !repository.archivedAt,
  ) ?? []
  return (
    <div className="repository-actions">
      <fieldset disabled={repositories.isPending || verify.isPending}>
        <legend>Repository selection</legend>
        {repositories.isPending && <p>Loading registered repositories…</p>}
        {repositories.isError && (
          <p role="alert">Registered repositories could not be loaded.</p>
        )}
        {repositories.isSuccess && available.map((repository) => (
          <label key={repository.id}>
            <input
              checked={selected.includes(repository.id)}
              onChange={(event) => setSelected((current) => event.target.checked
                ? [...current, repository.id]
                : current.filter((id) => id !== repository.id))}
              type="checkbox"
            />
            {repository.owner}/{repository.name}
          </label>
        ))}
      </fieldset>
      {error && <p role="alert">{error}</p>}
      <button
        disabled={selected.length === 0 || repositories.isPending || verify.isPending}
        onClick={() => {
          setError(null)
          verify.mutate()
        }}
        type="button"
      >
        {verify.isPending ? 'Verifying…' : 'Reverify installation'}
      </button>
    </div>
  )
}

function integrationError(reason: unknown) {
  return isApiError(reason)
    ? reason.message
    : 'The GitHub installation could not be verified.'
}
