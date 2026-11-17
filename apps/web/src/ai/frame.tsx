import type { ReactNode } from 'react'
import { Link, useParams } from 'react-router-dom'

import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { RememberPlace } from '../projects/RememberPlace'
import { useProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { useShellPage } from '../shell/shellStore'
import { WorkTabs } from '../shell/ProjectTabs'
import { Icon, PageHeader, type IconName } from '../ui'
import { useProjectAi } from './hooks'
import { planningHref, type PlanningTab } from './links'
// Every planning page comes through here, so the Graph and Import pages get the styles even when opened first.
import './ai.css'

const SUBPAGES: { id: PlanningTab; label: string; icon: IconName }[] = [
  { id: 'next', label: 'Up next', icon: 'play' },
  { id: 'suggestions', label: 'Suggestions', icon: 'bot' },
  { id: 'graph', label: 'Graph', icon: 'branch' },
  { id: 'plan-import', label: 'Import a plan', icon: 'list' },
]

/**
 * The shell, project loading states and navigation shared by the planning
 * pages: the page title first, then the project's task tabs (as on Tasks,
 * Milestones and Timeline), then a compact switch between Up next,
 * Suggestions, Graph and Import a plan.
 */
export function PlanningFrame({
  children,
  label,
  tab,
}: {
  children: (project: Project) => ReactNode
  label: string
  tab: PlanningTab
}) {
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const status = useProjectAi(projectId)
  useShellPage({ project: project.data })
  if (project.isPending) {
    return (
      <ProjectPageLoading label={label} />
    )
  }
  if (project.isError) {
    return (
      <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
    )
  }
  const pending = status.data?.pendingSuggestions ?? 0
  return (
    <>
      <div className="aw-page aw-page--full aw-ai-page">
        <PageHeader icon="branch" title="Planning" />
        <WorkTabs current="planning" projectId={projectId} />
        <RememberPlace location={{ kind: 'planning' }} project={project.data} />
        <nav aria-label="Planning" className="aw-seg aw-ai-subnav">
          {SUBPAGES.map((page) => (
            <Link aria-current={page.id === tab ? 'page' : undefined} className="aw-seg__opt" key={page.id} to={planningHref(projectId, page.id)}>
              <Icon name={page.icon} size={14} />
              {page.label}
              {page.id === 'suggestions' && pending ? <span className="aw-ai-subnav__count">{pending}</span> : null}
            </Link>
          ))}
        </nav>
        {children(project.data)}
      </div>
    </>
  )
}

/** What a planning page shows, with its actions beside it; a title only where the page is about one thing, like a plan. */
export function PlanningIntro({ actions, children, title }: { actions?: ReactNode; children?: ReactNode; title?: ReactNode }) {
  return (
    <div className="aw-ai-intro">
      <div className="aw-ai-intro__text">
        {title ? <h2 className="aw-ai-intro__title">{title}</h2> : null}
        {children ? <p className="aw-ai-intro__lede">{children}</p> : null}
      </div>
      {actions ? <div className="aw-ai-intro__actions">{actions}</div> : null}
    </div>
  )
}
