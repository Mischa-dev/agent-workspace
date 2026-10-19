import { TeamSettings } from '../../team/TeamSettings'
import { PageHeader } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Project › Members: who is in this project and what they can do. */
export default function MembersPage({ project, workspace }: SettingsPageProps) {
  if (!project || !workspace) return null
  return (
    <>
      <PageHeader icon="users" title="Members">
        People and agents in {project.name}. Add anyone already in the workspace, or invite someone new.
      </PageHeader>
      <TeamSettings organizationRole={workspace.role} projectId={project.id} />
    </>
  )
}
