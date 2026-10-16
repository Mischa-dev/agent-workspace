import { Link } from 'react-router-dom'

import { useGitHubConnection } from '../../github/connect'
import {
  GitHubConnectPanel,
  GitHubConnectionLoading,
  GitHubInstallationList,
} from '../../github/GitHubConnect'
import { Banner, Button, ErrorState, Icon, PageHeader, Row, RowList, Section } from '../../ui'
import { projectSettingsHref } from '../paths'
import type { SettingsPageProps } from '../types'

/** Workspace › GitHub: the GitHub accounts this workspace connected, and where to add their repositories. */
export default function GitHubPage({ projects, workspace }: SettingsPageProps) {
  const inWorkspace = projects.filter((project) => project.organizationId === workspace?.id)
  // Installations belong to the workspace; any of its Projects can read them.
  const via = inWorkspace.find((project) => project.capabilities.manage) ?? inWorkspace[0] ?? null
  const connection = useGitHubConnection(via?.id ?? '')
  if (!workspace) return null

  return (
    <>
      <PageHeader icon="pull-request" title="GitHub">
        GitHub accounts and organizations this workspace connected. Every Project in it can add their repositories.
      </PageHeader>
      {!via ? (
        <Banner role="note" tone="info">
          Create a Project first. GitHub is connected from a Project’s Repositories page.
        </Banner>
      ) : connection.isPending ? (
        <GitHubConnectionLoading />
      ) : connection.isError ? (
        <ErrorState action={<Button onClick={() => void connection.refetch()}>Try again</Button>} title="GitHub didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : (
        <>
          <Section id="github-accounts-heading" title="Connected accounts">
            <GitHubInstallationList connection={connection.data} />
            {connection.data.connectAvailable ? (
              <GitHubConnectPanel connection={connection.data} projectId={via.id} />
            ) : (
              <Banner role="note" tone="info">
                {connection.data.configured
                  ? 'To connect from here, the host operator must set the GitHub App’s slug and its OAuth client ID and secret.'
                  : 'GitHub isn’t set up on this server. A host operator must install the GitHub App’s credentials first.'}
              </Banner>
            )}
          </Section>
          <Section description="Repositories are added to one Project at a time." id="github-projects-heading" title="Add repositories">
            <RowList label="Projects">
              {inWorkspace.map((project) => (
                <Row
                  actions={
                    <Link className="aw-btn aw-btn--ghost aw-btn--sm" to={projectSettingsHref(project.id, 'repositories')}>
                      <span className="aw-btn__label">Repositories</span>
                      <Icon name="chevron-right" size={14} />
                    </Link>
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
