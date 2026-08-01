import { useQuery } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { getGitHubStatus } from './api'

export function GitHubIntegrationSettings({ projectId }: { projectId: string }) {
  const status = useQuery({
    queryKey: queryKeys.githubIntegration(projectId),
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
    return (
      <div className="integration-empty-state">
        <span className="status-chip" data-state="pending">Host ready</span>
        <p>
          GitHub App credentials are available, but this Project has no verified
          installation yet.
        </p>
      </div>
    )
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
    </div>
  )
}
