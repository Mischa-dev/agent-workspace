import { PageTabs } from '../ui'

export type WorkTab = 'items' | 'milestones' | 'timeline' | 'planning'

/** Tabs across the pages under a Project's Work items, which the sidebar shows as one row. */
export function WorkTabs({ current, projectId }: { current: WorkTab; projectId: string }) {
  const base = `/projects/${encodeURIComponent(projectId)}`
  return (
    <PageTabs
      current={current}
      label="Work items"
      replace={false}
      tabs={[
        { id: 'items', label: 'Work items', to: `${base}/work-items`, icon: 'work' },
        { id: 'milestones', label: 'Milestones', to: `${base}/milestones`, icon: 'checklist' },
        { id: 'timeline', label: 'Timeline', to: `${base}/timeline`, icon: 'clock' },
        { id: 'planning', label: 'Planning', to: `${base}/next`, icon: 'branch' },
      ]}
    />
  )
}

/** Tabs between a Project's Pages and its Decisions. */
export function MemoryTabs({ current, projectId }: { current: 'pages' | 'decisions'; projectId: string }) {
  const base = `/projects/${encodeURIComponent(projectId)}`
  return (
    <PageTabs
      current={current}
      label="Pages"
      replace={false}
      tabs={[
        { id: 'pages', label: 'Pages', to: `${base}/pages`, icon: 'list' },
        { id: 'decisions', label: 'Decisions', to: `${base}/decisions`, icon: 'check' },
      ]}
    />
  )
}
