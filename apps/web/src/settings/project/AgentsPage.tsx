import { ExternalAgentSettings } from '../../external-agents/ExternalAgentSettings'
import { PageHeader } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Project › Agents: agents connected to this Project and what they may do. */
export default function AgentsPage({ project }: SettingsPageProps) {
  if (!project) return null
  return (
    <>
      <PageHeader icon="bot" title="Agents">
        Agents that work in {project.name} as themselves, with their own credentials.
      </PageHeader>
      <ExternalAgentSettings projectId={project.id} />
    </>
  )
}
