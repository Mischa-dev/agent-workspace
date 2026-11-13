import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Row, RowList, SegmentedControl, countLabel, describeError, formatRelative, showToast } from '../ui'
import { sheetKeys, updateSheet, useSheets, type SheetSummary } from './api'
import { SheetDetailsDialog } from './SheetDialogs'

type Shown = 'open' | 'archived'

/** A project's sheets, each with its tabs and how many cells hold something. */
export function SheetsListPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const sheets = useSheets(projectId)
  const client = useQueryClient()
  const navigate = useNavigate()
  const [shown, setShown] = useState<Shown>('open')
  const [creating, setCreating] = useState(false)

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading sheets" />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const canEdit = Boolean(sheets.data?.canEdit) && !project.data.archivedAt
  const all = sheets.data?.sheets ?? []
  const archivedCount = all.filter((sheet) => sheet.archived).length
  const visible = all.filter((sheet) => (shown === 'archived' ? sheet.archived : !sheet.archived))
  const href = (id: string) => `/projects/${encodeURIComponent(projectId)}/sheets/${encodeURIComponent(id)}`
  const restore = async (sheet: SheetSummary) => {
    try {
      await updateSheet(projectId, sheet.id, { archived: false })
      void client.invalidateQueries({ queryKey: sheetKeys.all(projectId) })
      showToast({ tone: 'success', title: `${sheet.name} restored` })
    } catch (error) {
      showToast({
        tone: 'danger',
        title: `${sheet.name} wasn’t restored`,
        description: describeError(error).message,
      })
    }
  }
  const newSheet = canEdit ? (
    <Button icon="plus" onClick={() => setCreating(true)} variant="primary">
      New sheet
    </Button>
  ) : null

  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-sheets">
        <PageHeader actions={newSheet} icon="table" title="Sheets" />
        {archivedCount || shown === 'archived' ? (
          <SegmentedControl<Shown>
            label="Sheets to show"
            onChange={setShown}
            options={[
              { value: 'open', label: 'Open' },
              { value: 'archived', label: `Archived (${archivedCount})` },
            ]}
            value={shown}
          />
        ) : null}
        {sheets.isPending ? <ListSkeleton label="Loading sheets" rows={3} /> : null}
        {sheets.isError ? <ErrorState error={sheets.error} onRetry={() => void sheets.refetch()} title="Sheets didn’t load" /> : null}
        {sheets.isSuccess && visible.length === 0 ? (
          <EmptyState action={shown === 'archived' ? undefined : newSheet} icon="table" title={shown === 'archived' ? 'No archived sheets' : 'No sheets yet'}>
            {shown === 'archived'
              ? 'Archived sheets show up here.'
              : 'A sheet is a spreadsheet for this project, with formulas worked out as you type. People and agents fill it in together.'}
          </EmptyState>
        ) : null}
        {visible.length ? (
          <RowList label="Sheets">
            {visible.map((sheet) => (
              <Row
                actions={
                  sheet.archived && canEdit ? (
                    <Button onClick={() => void restore(sheet)} size="sm" variant="ghost">
                      Restore
                    </Button>
                  ) : null
                }
                key={sheet.id}
                leading={<Icon name="table" size={18} />}
                meta={[
                  countLabel(sheet.tabs.length, 'tab'),
                  countLabel(sheet.cells, 'filled cell'),
                  sheet.description || null,
                  sheet.updatedAt ? `changed ${formatRelative(sheet.updatedAt)}` : null,
                ]
                  .filter(Boolean)
                  .join(' · ')}
                title={<Link to={href(sheet.id)}>{sheet.name}</Link>}
              />
            ))}
          </RowList>
        ) : null}
      </div>
      {creating ? <SheetDetailsDialog onClose={() => setCreating(false)} onSaved={(body) => void navigate(href(body.sheet.id))} projectId={projectId} sheet={null} /> : null}
    </AppShell>
  )
}
