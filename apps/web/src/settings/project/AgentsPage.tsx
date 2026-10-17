import { AgentProfiles } from '../../agents/AgentProfiles'
import { ExternalAgentSettings } from '../../external-agents/ExternalAgentSettings'
import { PageHeader } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Project › Agents: the project's own agents, then agents connected to it and what they may do. */
export default function AgentsPage({ project }: SettingsPageProps) {
  if (!project) return null
  return (
    <>
      <PageHeader icon="bot" title="Agents">
        Agents that work in {project.name}: ones made here from a template, and ones that connect with their own credentials.
      </PageHeader>
      <AgentProfiles projectId={project.id} />
      <ExternalAgentSettings projectId={project.id} />
    </>
  )
}
