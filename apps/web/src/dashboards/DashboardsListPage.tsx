import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { opQuery } from '../api/ops'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useShellPage } from '../shell/shellStore'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Row, RowList, Section, countLabel, formatRelative } from '../ui'
import type { DashboardEntry } from '../api/generated/operations'
import { DashboardDetailsDialog } from './DashboardDialogs'
import { dashboardHref, splitDashboards } from './dashboardModel'
import { OutsideDataLink } from './OutsideDataPage'

/** A project's dashboards: the shared ones, then the person's own. */
export function DashboardsListPage() {
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const list = useQuery({ ...opQuery('dashboard_list', { spaceId: projectId }), enabled: Boolean(projectId) })
  const navigate = useNavigate()
  const [creating, setCreating] = useState(false)

  useShellPage({ project: project.data })
  if (project.isPending) return <ProjectPageLoading label="Loading dashboards" />
  if (project.isError) return <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />

  const open = !project.data.archivedAt
  const { shared, own } = splitDashboards(list.data?.dashboards ?? [])
  const newDashboard = open ? (
    <Button icon="plus" onClick={() => setCreating(true)} variant="primary">
      New dashboard
    </Button>
  ) : null
  const actions = (
    <>
      <OutsideDataLink projectId={projectId} />
      {newDashboard}
    </>
  )
  const rows = (entries: DashboardEntry[], label: string) => (
    <RowList label={label}>
      {entries.map((dashboard) => (
        <Row
          key={dashboard.id}
          leading={<Icon name="chart" size={18} />}
          meta={[dashboard.shared ? null : 'Only you', countLabel(dashboard.tiles.length, 'tile'), dashboard.description || null, `changed ${formatRelative(dashboard.updatedAt)}`].filter(Boolean).join(' · ')}
          title={<Link to={dashboardHref(projectId, dashboard.id)}>{dashboard.name}</Link>}
        />
      ))}
    </RowList>
  )

  return (
    <div className="aw-page aw-dashboards">
      <PageHeader actions={actions} icon="chart" title="Dashboards" />
      {list.isPending ? <ListSkeleton label="Loading dashboards" rows={3} /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Dashboards didn’t load" /> : null}
      {list.isSuccess && shared.length + own.length === 0 ? (
        <EmptyState action={newDashboard} icon="chart" title="No dashboards yet">
          A dashboard puts charts, counts and notes from this project’s collections on one page. Each tile reads the records live, as whoever looks at it can see them.
        </EmptyState>
      ) : null}
      {shared.length ? (own.length ? <Section title="Shared">{rows(shared, 'Shared dashboards')}</Section> : rows(shared, 'Dashboards')) : null}
      {own.length ? <Section title="Yours">{rows(own, 'Your dashboards')}</Section> : null}
      {creating ? (
        <DashboardDetailsDialog
          canShare={list.data?.canShare === true}
          dashboard={null}
          onClose={() => setCreating(false)}
          onSaved={(saved) => void navigate(dashboardHref(projectId, saved.id))}
          projectId={projectId}
        />
      ) : null}
    </div>
  )
}
