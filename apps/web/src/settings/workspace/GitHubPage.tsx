import { useGitHubConnection } from '../../github/connect'
import {
  GitHubConnectPanel,
  GitHubConnectionLoading,
  GitHubInstallationList,
} from '../../github/GitHubConnect'
import { Banner, ButtonLink, ErrorState, Icon, PageHeader, Row, RowList, Section } from '../../ui'
import { projectSettingsHref } from '../paths'
import type { SettingsPageProps } from '../types'

/** Workspace › GitHub: the GitHub accounts this workspace connected, and where to add their repositories. */
export default function GitHubPage({ projects, workspace }: SettingsPageProps) {
  const inWorkspace = projects.filter((project) => project.organizationId === workspace?.id)
  // Installations belong to the workspace; any of its projects can read them.
  const via = inWorkspace.find((project) => project.capabilities.manage) ?? inWorkspace[0] ?? null
  const connection = useGitHubConnection(via?.id ?? '')
  if (!workspace) return null

  return (
    <>
      <PageHeader icon="pull-request" title="GitHub">
        The GitHub accounts and organizations this workspace connected, and where to add their repositories.
      </PageHeader>
      {!via ? (
        <Banner role="note" tone="info">
          Create a project first. GitHub is connected from a project’s Repositories page.
        </Banner>
      ) : connection.isPending ? (
        <GitHubConnectionLoading />
      ) : connection.isError ? (
        <ErrorState error={connection.error} onRetry={() => void connection.refetch()} title="GitHub didn’t load" />
      ) : (
        <>
          <Section id="github-accounts-heading" title="Connected accounts">
            {connection.data.installations.length > 0 && <GitHubInstallationList connection={connection.data} />}
            {connection.data.connectAvailable ? (
              <GitHubConnectPanel connection={connection.data} projectId={via.id} />
            ) : (
              <Banner role="note" tone="info">
                {connection.data.configured
                  ? 'Connecting from here isn’t set up: whoever runs this server has to finish setting up the GitHub App on it.'
                  : 'GitHub isn’t set up on this server. Whoever runs it has to set up the GitHub App first.'}
              </Banner>
            )}
          </Section>
          <Section
            description={
              connection.data.connectAvailable
                ? 'Repositories are added to one project at a time.'
                : 'Until GitHub is set up, add a repository by hand on a project’s Repositories page.'
            }
            id="github-projects-heading"
            title="Add repositories"
          >
            <RowList label="Projects">
              {inWorkspace.map((project) => (
                <Row
                  actions={
                    <ButtonLink
                      aria-label={`Repositories in ${project.name}`}
                      iconEnd="chevron-right"
                      size="sm"
                      to={projectSettingsHref(project.id, 'repositories')}
                      variant="ghost"
                    >
                      Repositories
                    </ButtonLink>
                  }
                  key={project.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name="repo" size={16} />
                    </span>
                  }
                  title={project.name}
                />
              ))}
            </RowList>
          </Section>
        </>
      )}
    </>
  )
}
