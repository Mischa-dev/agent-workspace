import { useQuery } from '@tanstack/react-query'

import { queryKeys } from '../../api/queryKeys'
import { listRepositories } from '../../repositories/api'
import { PageHeader } from '../../ui'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'

/** Project › Environments and defaults: the `run.*` settings, each inheriting from the workspace. */
export default function EnvironmentsPage({ project }: SettingsPageProps) {
  const repositories = useQuery({
    queryKey: queryKeys.repositories(project?.id ?? ''),
    queryFn: () => listRepositories(project?.id ?? ''),
    enabled: Boolean(project),
  })
  if (!project) return null
  const repositoryOptions = (repositories.data?.repositories ?? [])
    .filter((repository) => !repository.archivedAt)
    .map((repository) => ({ value: repository.id, label: `${repository.owner}/${repository.name}` }))
  return (
    <>
      <PageHeader icon="settings" title="Environments and defaults">
        What new runs in {project.name} use. Each one follows the workspace until you override it here.
      </PageHeader>
      <ScopedSettings
        options={repositories.isSuccess ? { 'run.repository': repositoryOptions } : undefined}
        prefixes={['run.']}
        target={{ scope: 'project', organizationId: project.organizationId, projectId: project.id }}
      />
    </>
  )
}
