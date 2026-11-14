import { useGitHubConnection } from '../../github/connect'
import {
  GitHubConnectPanel,
  GitHubConnectUnavailable,
  GitHubConnectionLoading,
  GitHubRepositoryPicker,
} from '../../github/GitHubConnect'
import { GitHubIntegrationSettings } from '../../github/GitHubIntegrationSettings'
import { RepositorySettings } from '../../repositories/RepositorySettings'
import { ErrorState, PageHeader, Section } from '../../ui'
import { GitHubReturnBanner } from '../GitHubReturnBanner'
import type { SettingsPageProps } from '../types'
import '../../styles/pages.css'

/**
 * Project › Repositories: connect GitHub, pick a repository, done. GitHub sends
 * the browser back here with `?github=connected` or `?github=error&reason=`.
 */
export default function RepositoriesPage({ project }: SettingsPageProps) {
  const projectId = project?.id ?? ''
  const connection = useGitHubConnection(projectId, project?.capabilities.manage === true)

  if (!project) return null
  const manage = project.capabilities.manage

  return (
    <>
      <PageHeader icon="repo" title="Repositories">
        The code runs can work in. The server keeps its own copy of each repository, so nothing is read from your computer.
      </PageHeader>
      <GitHubReturnBanner className="aw-page__banner" repositoriesHere />
      {manage && (
        <Section id="github-connect-heading" title="Add from GitHub">
          {connection.isPending ? (
            <GitHubConnectionLoading />
          ) : connection.isError ? (
            <ErrorState error={connection.error} onRetry={() => void connection.refetch()} title="GitHub didn’t load" />
          ) : connection.data.connectAvailable ? (
            <div className="aw-settings-stack">
              <GitHubConnectPanel connection={connection.data} projectId={projectId} />
              {connection.data.installations.length > 0 && <GitHubRepositoryPicker projectId={projectId} />}
            </div>
          ) : (
            <GitHubConnectUnavailable connection={connection.data} projectId={projectId} />
          )}
        </Section>
      )}
      <Section id="repositories-heading" title="In this project">
        <RepositorySettings projectId={projectId} showRegistration={false} />
      </Section>
      {manage && connection.data?.configured && (
        <Section
          description="The installation this project’s pull requests and checks come through."
          id="github-integration-heading"
          title="GitHub installation"
        >
          <GitHubIntegrationSettings projectId={projectId} />
        </Section>
      )}
    </>
  )
}
