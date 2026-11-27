import { useProjectExperience } from '../../experience/layout'
import { TeamSettings } from '../../team/TeamSettings'
import { Banner, PageHeader } from '../../ui'
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
      {project.peopleFromParent ? (
        <Banner role="note" tone="info">
          This project is inside another one and has its people too, with the same roles. To change or remove one of them, do it in that
          project. Anyone you add here is only in this one.
        </Banner>
      ) : null}
      <TeamSettings organizationId={workspace.id} projectId={project.id} />
    </>
  )
}
