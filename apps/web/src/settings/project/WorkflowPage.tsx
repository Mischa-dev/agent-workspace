import { PageHeader } from '../../ui'
import { WorkflowSettings } from '../../work-items/WorkflowSettings'
import type { SettingsPageProps } from '../types'

/** Project › Workflow: the project's statuses, types, status changes and estimates. */
export default function WorkflowPage({ project }: SettingsPageProps) {
  if (!project) return null
  return (
    <>
      <PageHeader icon="board" title="Workflow">
        The statuses, types and estimates tasks in {project.name} use.
      </PageHeader>
      <WorkflowSettings projectId={project.id} />
    </>
  )
}
