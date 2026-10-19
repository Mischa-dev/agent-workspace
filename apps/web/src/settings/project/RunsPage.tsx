import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { queryKeys } from '../../api/queryKeys'
import type { AuthenticatedSession } from '../../auth/session'
import { listHarnesses } from '../../execution/api'
import { useEnvironments } from '../../launch/hooks'
import { EnvironmentSettings } from '../../launch/LaunchSettings'
import type { Project } from '../../projects/types'
import { listRepositories } from '../../repositories/api'
import { Banner, PageHeader, Section } from '../../ui'
import { useEffectiveSettings, type SettingsTarget } from '../api'
import { workspaceSettingsHref } from '../paths'
import { LaunchPrompt } from '../runs/LaunchPrompt'
import { ProjectModelLists } from '../runs/ModelLists'
import { RUN_UNSET_LABELS, harnessAt, runSettingOptions } from '../runs/options'
import { ProjectSuggestions } from '../runs/ProjectSuggestions'
import { useSectionScroll } from '../runs/useSectionScroll'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'
import { defaultWorkspace } from '../workspaceChoice'

/**
 * Project › Runs and models: what new runs here use (each value following the
 * workspace until it's set here), where they work, the models people can pick,
 * what suggestions and summaries run on, and the launch prompt.
 */
export default function RunsPage({ project, session }: SettingsPageProps) {
  return project ? <ProjectRuns project={project} session={session} /> : null
}

/** First-run setup's part of the page: where runs work and the defaults they start with. */
export function RunSetup({ project, session }: SettingsPageProps) {
  return project ? <ProjectRuns project={project} session={session} setup /> : null
}

function ProjectRuns({ project, session, setup = false }: { project: Project; session: AuthenticatedSession; setup?: boolean }) {
  const queryClient = useQueryClient()
  const target: SettingsTarget = { scope: 'project', organizationId: project.organizationId, projectId: project.id }
  const effective = useEffectiveSettings(target)
  const harnesses = useQuery({ queryKey: queryKeys.harnesses(project.id), queryFn: () => listHarnesses(project.id) })
  const repositories = useQuery({ queryKey: queryKeys.repositories(project.id), queryFn: () => listRepositories(project.id) })
  const environments = useEnvironments(project.id)
  useSectionScroll(effective.isSuccess && !harnesses.isPending)
  // Workspace settings show the default workspace; link there only when it's this project's.
  const workspaceHref = defaultWorkspace(session)?.id === project.organizationId ? workspaceSettingsHref('runs') : undefined
  const readOnly = effective.data ? !effective.data.capabilities.manageProject : false
  const canEditLists = harnesses.data?.some((harness) => harness.canEditCatalog) ?? false
  // The environments below mark the default, so they follow a change to it here.
  const environmentsChanged = (key: string) => {
    if (key === 'run.environment') void queryClient.invalidateQueries({ queryKey: ['project', project.id, 'environments'] })
  }
  const options = {
    ...runSettingOptions(harnesses.data ?? [], harnessAt(effective.data, target)),
    ...(repositories.isSuccess
      ? {
          'run.repository': repositories.data.repositories
            .filter((repository) => !repository.archivedAt)
            .map((repository) => ({ value: repository.id, label: `${repository.owner}/${repository.name}` })),
        }
      : {}),
    ...(environments.isSuccess
      ? {
          'run.environment': environments.data.environments.map((environment) => ({
            value: environment.id,
            label: `${environment.name} · ${environment.harnessName}`,
          })),
        }
      : {}),
  }

  const defaults = (
    <Section description="New runs here use these unless the launch or the agent picks something else." id="defaults" title="Defaults">
      <ScopedSettings
        onSaved={environmentsChanged}
        options={options}
        prefixes={['run.']}
        readOnlyNote={false}
        target={target}
        unsetLabels={RUN_UNSET_LABELS}
        workspaceHref={workspaceHref}
      />
    </Section>
  )
  const where = (
    <Section
      description="Somewhere an agent can work, such as a cloud environment set up with the repository. Launch picks the default."
      id="environments"
      title="Environments"
    >
      <EnvironmentSettings projectId={project.id} />
    </Section>
  )
  const readOnlyNote = readOnly ? (
    <Banner role="note" tone="info">
      Only project owners and admins can change these.
    </Banner>
  ) : null
  if (setup) {
    return (
      <>
        {readOnlyNote}
        {where}
        {defaults}
      </>
    )
  }

  return (
    <>
      <PageHeader icon="activity" title="Runs and models">
        What runs and agents in {project.name} use. Each setting follows the{' '}
        {workspaceHref ? <Link to={workspaceHref}>workspace</Link> : 'workspace'} until you change it here.
      </PageHeader>
      {readOnlyNote}
      {defaults}
      {where}
      <Section
        description={
          <>
            Everyone in the workspace picks from the same lists.{' '}
            {canEditLists && workspaceHref ? (
              <Link to={`${workspaceHref}#models`}>Change them in workspace settings.</Link>
            ) : (
              'Workspace owners and admins change them.'
            )}
          </>
        }
        id="models"
        title="Models"
      >
        <ProjectModelLists projectId={project.id} />
      </Section>
      <ProjectSuggestions project={project} workspaceHref={workspaceHref} />
      <Section
        description="The prompt Launch writes from a task. Fields in double braces are filled in; a task’s Launch dialog shows the result."
        id="launch-prompt"
        title="Launch prompt"
      >
        <LaunchPrompt target={target} workspaceHref={workspaceHref} />
      </Section>
    </>
  )
}
