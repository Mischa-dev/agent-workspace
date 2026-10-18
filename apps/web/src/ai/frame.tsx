import type { ReactNode } from 'react'
import { useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { AppShell } from '../shell/AppShell'
import { WorkTabs } from '../shell/ProjectTabs'
import { PageTabs } from '../ui'
import { useProjectAi } from './hooks'
import { planningHref, type PlanningTab } from './links'


/**
 * The shell, project loading states and tabs shared by the planning pages:
 * Up next, Suggestions, Graph and Import a plan.
 */
export function PlanningFrame({
  children,
  label,
  tab,
  wide = false,
}: {
  children: (project: Project) => ReactNode
  label: string
  tab: PlanningTab
  wide?: boolean
}) {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const status = useProjectAi(projectId)
  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label={label} />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }
  const pending = status.data?.pendingSuggestions ?? 0
  return (
    <AppShell project={project.data} user={session.user}>
      <div className={`aw-page ${wide ? 'aw-page--full' : 'aw-page--wide'} aw-ai-page`}>
        <WorkTabs current="planning" projectId={projectId} />
        <PageTabs
          current={tab}
          label="Planning"
          tabs={[
            { id: 'next', label: 'Up next', to: planningHref(projectId, 'next'), icon: 'play' },
            { id: 'suggestions', label: pending ? `Suggestions (${pending})` : 'Suggestions', to: planningHref(projectId, 'suggestions'), icon: 'bot' },
            { id: 'graph', label: 'Graph', to: planningHref(projectId, 'graph'), icon: 'branch' },
            { id: 'plan-import', label: 'Import a plan', to: planningHref(projectId, 'plan-import'), icon: 'list' },
          ]}
        />
        {children(project.data)}
      </div>
    </AppShell>
  )
}
