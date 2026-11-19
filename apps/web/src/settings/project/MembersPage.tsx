import { useProjectExperience } from '../../experience/layout'
import { TeamSettings } from '../../team/TeamSettings'
import { PageHeader } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Project › Members: who is in this project and what they can do. */
export default function MembersPage({ project, workspace }: SettingsPageProps) {
  const agentsOn = useProjectExperience(project?.id).moduleOn('agents')
  if (!project || !workspace) return null
  return (
    <>
      <PageHeader icon="users" title="Members">
        {agentsOn ? 'People and agents' : 'People'} in {project.name}. Add anyone already in the workspace, or invite someone new.
      </PageHeader>
      <TeamSettings organizationId={workspace.id} organizationRole={workspace.role} projectId={project.id} />
    </>
  )
}
