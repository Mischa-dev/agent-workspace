import { useProjectExperience } from '../experience/layout'
import type { PartId } from '../experience/parts'
import { PageTabs } from '../ui'

export type WorkTab = 'items' | 'milestones' | 'timeline' | 'planning'

/** Tabs across the pages under a project's Tasks, which the sidebar shows as one row. */
export function WorkTabs({ current, projectId }: { current: WorkTab; projectId: string }) {
  const experience = useProjectExperience(projectId)
  const base = `/projects/${encodeURIComponent(projectId)}`
  // Each tab is a part of the layout; the one you're on always shows.
  const tabs: { id: WorkTab; part: PartId; label: string; to: string; icon: 'work' | 'checklist' | 'clock' | 'branch' }[] = [
    { id: 'items', part: 'area.tasks', label: experience.label('task', 'many'), to: `${base}/work-items`, icon: 'work' },
    { id: 'milestones', part: 'area.milestones', label: experience.label('milestone', 'many'), to: `${base}/milestones`, icon: 'checklist' },
    { id: 'timeline', part: 'area.timeline', label: 'Timeline', to: `${base}/timeline`, icon: 'clock' },
    { id: 'planning', part: 'area.planning', label: 'Planning', to: `${base}/next`, icon: 'branch' },
  ]
  const shown = tabs.filter((tab) => tab.id === current || experience.shows(tab.part))
  if (shown.length < 2) return null
  return <PageTabs current={current} label={experience.label('task', 'many')} replace={false} tabs={shown} />
}

/** Tabs between a project's Pages and its Decisions. */
export function MemoryTabs({ current, projectId }: { current: 'pages' | 'decisions'; projectId: string }) {
  const experience = useProjectExperience(projectId)
  const base = `/projects/${encodeURIComponent(projectId)}`
  const tabs: { id: 'pages' | 'decisions'; part: PartId; label: string; to: string; icon: 'list' | 'check' }[] = [
    { id: 'pages', part: 'area.pages', label: experience.label('page', 'many'), to: `${base}/pages`, icon: 'list' },
    { id: 'decisions', part: 'area.decisions', label: 'Decisions', to: `${base}/decisions`, icon: 'check' },
  ]
  const shown = tabs.filter((tab) => tab.id === current || experience.shows(tab.part))
  if (shown.length < 2) return null
  return <PageTabs current={current} label={experience.label('page', 'many')} replace={false} tabs={shown} />
}
