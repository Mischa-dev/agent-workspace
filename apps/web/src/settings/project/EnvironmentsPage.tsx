import { useQuery } from '@tanstack/react-query'

import { queryKeys } from '../../api/queryKeys'
import { useEnvironments } from '../../launch/hooks'
import { EnvironmentSettings, TemplateEditor } from '../../launch/LaunchSettings'
import { listRepositories } from '../../repositories/api'
import { PageHeader, Section } from '../../ui'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'

/**
 * Project › Environments and defaults: where launched agents work, the `run.*` settings (each inheriting from the
 * workspace) and the prompt Launch writes from a task.
 */
export default function EnvironmentsPage({ project }: SettingsPageProps) {
  const repositories = useQuery({
    queryKey: queryKeys.repositories(project?.id ?? ''),
    queryFn: () => listRepositories(project?.id ?? ''),
    enabled: Boolean(project),
  })
  const environments = useEnvironments(project?.id ?? '')
  if (!project) return null
  const repositoryOptions = (repositories.data?.repositories ?? [])
    .filter((repository) => !repository.archivedAt)
    .map((repository) => ({ value: repository.id, label: `${repository.owner}/${repository.name}` }))
  const environmentOptions = (environments.data?.environments ?? []).map((environment) => ({
    value: environment.id,
    label: `${environment.name} · ${environment.harnessName}`,
  }))
  return (
    <>
      <PageHeader icon="play" title="Environments and defaults">
        Where agents launched in {project.name} work and what they run with. Each default follows the workspace until you
        override it here.
      </PageHeader>
      <Section
        description="Somewhere an agent can work, such as a cloud environment set up with your repository. Launch picks the default."
        id="environments-heading"
        title="Environments"
      >
        <EnvironmentSettings projectId={project.id} />
      </Section>
      <Section id="run-defaults-heading" title="Run defaults">
        <ScopedSettings
          options={{
            ...(repositories.isSuccess ? { 'run.repository': repositoryOptions } : {}),
            ...(environments.isSuccess ? { 'run.environment': environmentOptions } : {}),
          }}
          prefixes={['run.']}
          target={{ scope: 'project', organizationId: project.organizationId, projectId: project.id }}
        />
      </Section>
      <Section
        description="The prompt Launch writes from a task. Fields in double braces are filled in; a task’s Launch dialog shows the result."
        id="launch-template-heading"
        title="Launch prompt"
      >
        <TemplateEditor projectId={project.id} />
      </Section>
    </>
  )
}
