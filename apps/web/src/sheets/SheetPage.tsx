import { useMemo, useState } from 'react'
import { Download } from 'lucide-react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { opPrefix, opQuery } from '../api/ops'
import { AnchoredThreadPanel } from '../discussion/AnchoredThreadPanel'
import { HistoryPanel } from '../pieces/HistoryPanel'
import { RecentEditsBar, RecentEditsToggle } from '../changes/RecentEdits'
import { outlineTitle, useRecentEdits } from '../changes/useRecentEdits'
import { useProjectExperience } from '../experience/layout'
import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { useShellPage } from '../shell/shellStore'
import { usePageTitle } from '../shell/documentTitleModel'
import {
  Banner,
  Button,
  ButtonLink,
  ConfirmDialog,
  Dialog,
  ErrorState,
  IconButton,
  ListSkeleton,
  Menu,
  PageHeader,
  SidePanel,
  TextField,
  describeError,
  showToast,
  type MenuEntry,
} from '../ui'
import { usePhoneWidth } from '../ui/useCoarsePointer'
import { changeSheet, proposeCells, sheetExportHref, sheetKeys, updateSheet, useSheet, type LookChange, type SheetBody, type SheetChange, type SheetTab } from './api'
import { SheetDetailsDialog, TabNameDialog } from './SheetDialogs'
import { SheetGrid, type GridEdit, type GridShown } from './SheetGrid'
import { SheetProposalsDialog } from './SheetProposals'
import { SheetRangesDialog } from './SheetRanges'
import { proposedOn, useSheetProposals } from './sheetProposals'
import { SheetChartsDialog } from './SheetCharts'
import { SheetFind, SheetRulesDialog, SheetViewDialog } from './SheetTools'
import { SheetValidationDialog } from './SheetValidationDialog'
import { deleteSheetView, useSheetViews, viewSpec, type SheetView } from './sheetViews'
import { commentedCells, selectionAnchor, threadAt, type SheetAnchor } from './sheetComments'
import { bounds, cards, cellKey, cellMap, cellName, columnName, columnNumber, extent, patched, viewRows, type Selection } from './sheetModel'

type Dialogs =
  | { kind: 'details' }
  | { kind: 'add-tab' }
  | { kind: 'rename-tab'; tab: SheetTab }
  | { kind: 'remove-tab'; tab: SheetTab }
  | { kind: 'cell'; row: number; column: number; revision: number }

/** Each sheet's writes go one at a time, in order, so a later edit never lands first. */
const writes = new Map<string, Promise<unknown>>()
/** Revisions this screen's own writes made, by sheet: an edit made before one landed isn't a clash with it. */
const own = new Map<string, Set<number>>()
// Cells this screen wrote, by sheet then `tab:row:column`, with the revision
// that wrote them: a queued edit of the same cell builds on it, even while the
// cache waits for a fresh read.
const written = new Map<string, Map<string, number>>()

const START: Selection = {
  anchor: { row: 1, column: 1 },
  focus: { row: 1, column: 1 },
}

/** Fills offered in the format menu: soft colors that read in light and dark themes. */
const FILLS: [string, string][] = [
  ['No fill', ''],
  ['Yellow', '#FFF3B0'],
  ['Green', '#D3F2D8'],
  ['Blue', '#D6E8FF'],
  ['Red', '#FFD8D6'],
  ['Gray', '#E6E6E6'],
]

/** One sheet: a grid you edit in place (cards on a phone), its tabs under it. */
export function SheetPage() {
  const { projectId = '', sheetId = '' } = useParams()
  const project = useProject(projectId)
  useShellPage({ project: project.data })
  if (project.isPending) {
    return (
      <ProjectPageLoading label="Loading the sheet" />
    )
  }
  if (project.isError) {
    return (
      <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
    )
  }
  return <SheetWorkspace key={sheetId} project={project.data} sheetId={sheetId} />
}

function SheetWorkspace({ project, sheetId }: { project: Project; sheetId: string }) {
  const projectId = project.id
  const { user } = useAuthenticatedSession()
  const client = useQueryClient()
  const phone = usePhoneWidth()
  const [params, setParams] = useSearchParams()
  const tabParam = params.get('tab') ?? ''
  const sheet = useSheet(projectId, sheetId, tabParam)
  const [selection, setSelection] = useState<Selection>(START)
  // Inputs sent and not saved yet, by `tabId:row:column`.
  const [pending, setPending] = useState<Map<string, string>>(() => new Map())
  const [dialog, setDialog] = useState<Dialogs | null>(null)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  useShellPage({ project })
  usePageTitle(sheet.data?.sheet.name ?? null)

  const body = sheet.data
  const cells = useMemo(() => cellMap(body?.cells ?? []), [body?.cells])
  // Comments on cells (C1.19): marks in the grid, and a panel for the
  // thread about the selected cell or range.
  const experience = useProjectExperience(projectId)
  const showComments = experience.shows('thing.comments')
  const comments = useQuery({ ...opQuery('comment_list', { thing: { kind: 'sheet', id: sheetId } }), enabled: showComments })
  // The open discussion: by its thread's id once it has one, since rows and
  // columns moving around it move its anchor.
  const [historyOpen, setHistoryOpen] = useState(false)
  const [commenting, setCommenting] = useState<{ anchor: SheetAnchor; label: string; threadId?: string } | null>(null)
  // Find and replace, and the filter view on screen (C1.16).
  const [finding, setFinding] = useState(false)
  const [viewId, setViewId] = useState<string | null>(null)
  const [viewDialog, setViewDialog] = useState<{ view: SheetView | null } | null>(null)
  const [editingRules, setEditingRules] = useState(false)
  const [editingValidation, setEditingValidation] = useState(false)
  const [charting, setCharting] = useState(false)
  const [naming, setNaming] = useState(false)
  const views = useSheetViews(sheetId, body?.tab, !phone)
  // Proposals (C1.11): a role that only suggests edits a draft, sent as one
  // proposal; editors see the waiting ones outlined and decide them.
  const [draft, setDraft] = useState<Map<string, SheetChange>>(() => new Map())
  const [sendingDraft, setSendingDraft] = useState(false)
  const [reviewing, setReviewing] = useState(false)
  const proposals = useSheetProposals(sheetId, Boolean(body?.canEdit || body?.canSuggest))
  // Recent edits (C1.10): cells others just changed are outlined with who
  // and when; the toggle shows the last day's.
  const [allEdits, setAllEdits] = useState(false)
  const showsRecent = experience.shows('page.recent_edits')
  const recent = useRecentEdits('sheet', sheetId, body?.sheet.revision, { viewerId: user.actorId, all: allEdits, enabled: showsRecent })
  const shownTab = body?.tabs.find((entry) => entry.id === body.tab)
  const activeView = views.data?.views.find((view) => view.id === viewId && view.tabId === body?.tab) ?? null
  const frozenRows = shownTab?.frozenRows ?? 0
  const usedRows = shownTab?.rows ?? 0
  const shown = useMemo<GridShown | null>(() => {
    if (!activeView) return null
    const spec = viewSpec(activeView)
    return {
      rows: viewRows(cells, spec, frozenRows + 1, usedRows),
      hiddenColumns: new Set(spec.hiddenColumns.map(columnNumber).filter(Boolean)),
    }
  }, [activeView, cells, frozenRows, usedRows])

  if (sheet.isPending) {
    return (
      <div aria-busy="true" className="aw-page aw-page--full">
        <ListSkeleton label="Loading the sheet" rows={6} />
      </div>
    )
  }
  if (!body) {
    const gone = isApiError(sheet.error) && sheet.error.status === 404
    return (
      <div className="aw-page">
        {gone ? (
          <ErrorState
            action={
              <ButtonLink to={`/projects/${encodeURIComponent(projectId)}/sheets`} variant="primary">
                See the sheets
              </ButtonLink>
            }
            title="This sheet isn’t here"
          >
            It may have been removed, or the link is wrong.
          </ErrorState>
        ) : (
          <ErrorState error={sheet.error} onRetry={() => void sheet.refetch()} title="The sheet didn’t load" />
        )}
      </div>
    )
  }

  const tabs = [...body.tabs].sort((left, right) => left.position - right.position)
  const tab = tabs.find((entry) => entry.id === body.tab) ?? tabs[0]
  const editable = body.canEdit && !project.archivedAt
  const suggesting = !body.canEdit && body.canSuggest && !project.archivedAt && !body.sheet.archived
  const used = extent({ rows: tab?.rows ?? 0, columns: tab?.columns ?? 0 })
  const size = {
    rows: Math.max(used.rows, selection.focus.row + 20),
    columns: Math.max(used.columns, Math.min(200, selection.focus.column + 2)),
  }
  const focusCell = cells.get(cellKey(selection.focus.row, selection.focus.column))
  const marks = tab && comments.data ? commentedCells(comments.data.threads, tab.id) : undefined
  const focusThread = marks?.get(cellKey(selection.focus.row, selection.focus.column))
  const commentingStart = commenting ? (commenting.anchor.kind === 'cell' ? [commenting.anchor.row, commenting.anchor.column] : commenting.anchor.from) : null
  const commentingThread = !commenting || !comments.data
    ? null
    : commenting.threadId
      ? (comments.data.threads.find((thread) => thread.id === commenting.threadId) ?? null)
      : commentingStart
        ? threadAt(comments.data.threads, commenting.anchor.tabId, commentingStart[0] + 1, commentingStart[1] + 1)
        : null
  const openComments = () => {
    if (!tab) return
    const box = bounds(selection)
    const single = box.top === box.bottom && box.left === box.right
    const found = comments.data ? threadAt(comments.data.threads, tab.id, box.top, box.left) : null
    setCommenting({
      anchor: selectionAnchor(selection, tab.id),
      threadId: found?.id,
      label: single ? cellName({ row: box.top, column: box.left }) : `${cellName({ row: box.top, column: box.left })}:${cellName({ row: box.bottom, column: box.right })}`,
    })
  }
  const prefix = `${tab?.id ?? ''}:`
  const tabPending = new Map([...pending].filter(([at]) => at.startsWith(prefix)).map(([at, input]) => [at.slice(prefix.length), input]))
  for (const [at, change] of draft) if (at.startsWith(prefix) && change.type === 'cell') tabPending.set(at.slice(prefix.length), change.input)
  const proposedHere = tab && body.canEdit ? proposedOn(proposals, tab.id) : undefined
  const editedHere = new Map(
    [...recent.shown].filter(([at]) => at.startsWith(prefix)).map(([at, outline]) => [at.slice(prefix.length), outlineTitle(outline)]),
  )
  const scope = sheetKeys.one(projectId, sheetId)
  const key = sheetKeys.tab(projectId, sheetId, tabParam)

  const refresh = () => client.invalidateQueries({ queryKey: scope })

  /** Sends changes after any still on their way; true when they saved. */
  const send = (changes: SheetChange[], shown?: Map<string, string>) => {
    const run = (writes.get(sheetId) ?? Promise.resolve()).then(async () => {
      try {
        const mine = own.get(sheetId) ?? new Set<number>()
        const wrote = written.get(sheetId) ?? new Map<string, number>()
        const known = cellMap(client.getQueryData<SheetBody>(key)?.cells ?? [])
        const result = await changeSheet(
          projectId,
          sheetId,
          changes.map((change) => {
            if (change.type !== 'cell' || change.ifRevision === undefined) return change
            const seen = Math.max(known.get(cellKey(change.row, change.column))?.revision ?? 0, wrote.get(`${change.tab}:${change.row}:${change.column}`) ?? 0)
            return seen > change.ifRevision && mine.has(seen) ? { ...change, ifRevision: seen } : change
          }),
        )
        // Only a revision this write made is ours; one that changed nothing
        // answers with whatever the sheet was at, maybe someone else's.
        if (result.revision !== result.previousRevision) own.set(sheetId, mine.add(result.revision))
        if (result.revision !== result.previousRevision) {
          for (const change of changes) if (change.type === 'cell') wrote.set(`${change.tab}:${change.row}:${change.column}`, result.revision)
        }
        written.set(sheetId, wrote)
        const current = client.getQueryData<SheetBody>(key)
        const next = current ? patched(current, result) : null
        if (next) client.setQueryData(key, next)
        else void refresh()
        void client.invalidateQueries({ queryKey: sheetKeys.list(projectId) })
        return true
      } catch (error) {
        const conflict = isApiError(error) && error.status === 409
        showToast({
          tone: 'danger',
          title: conflict ? 'Someone changed that first' : 'That change wasn’t saved',
          description: conflict ? 'The sheet now shows their change. Make yours again if it still fits.' : describeError(error).message,
        })
        void refresh()
        return false
      } finally {
        if (shown) {
          setPending((current) => {
            const next = new Map(current)
            for (const [cell, input] of shown) if (next.get(cell) === input) next.delete(cell)
            return next
          })
        }
      }
    })
    writes.set(
      sheetId,
      run.catch(() => undefined),
    )
    return run
  }

  const edit = (edits: GridEdit[]) => {
    if (!tab || !edits.length) return
    const revision = body.sheet.revision
    const at = (entry: GridEdit) => `${tab.id}:${cellKey(entry.row, entry.column)}`
    setPending((current) => new Map([...current, ...edits.map((entry) => [at(entry), entry.input] as const)]))
    for (let start = 0; start < edits.length; start += 2_000) {
      const batch = edits.slice(start, start + 2_000)
      void send(
        batch.map((entry) => ({
          type: 'cell',
          tab: tab.id,
          row: entry.row,
          column: entry.column,
          input: entry.input,
          ifRevision: entry.revision ?? revision,
        })),
        new Map(batch.map((entry) => [at(entry), entry.input])),
      )
    }
  }

  /** A suggest-only edit: kept in the draft until it's sent as a proposal. */
  const suggestEdit = (edits: GridEdit[]) => {
    if (!tab || !edits.length) return
    // Each keeps the revision its edit opened at, so a cell changed since is refused rather than overwritten.
    const revision = body.sheet.revision
    setDraft((current) => {
      const next = new Map(current)
      for (const entry of edits) {
        const at = `${tab.id}:${cellKey(entry.row, entry.column)}`
        const kept = current.get(at)
        const ifRevision = kept?.type === 'cell' && kept.ifRevision !== undefined ? kept.ifRevision : (entry.revision ?? revision)
        next.set(at, { type: 'cell', tab: tab.id, row: entry.row, column: entry.column, input: entry.input, ifRevision })
      }
      return next
    })
  }

  const sendDraft = async () => {
    setSendingDraft(true)
    const sending = new Map(draft)
    try {
      const sent = await proposeCells(projectId, sheetId, [...sending.values()])
      // Edits made while it was on its way stay in the draft.
      setDraft((current) => {
        const next = new Map(current)
        for (const [at, change] of sending) if (next.get(at) === change) next.delete(at)
        return next
      })
      void client.invalidateQueries({ queryKey: opPrefix('proposal_list') })
      showToast(
        sent.proposalId
          ? { tone: 'success', title: 'Suggestion sent', description: 'Someone who edits this sheet can accept it.' }
          : { title: 'Nothing to suggest', description: 'Those cells already hold what you typed.' },
      )
    } catch (error) {
      showToast({ tone: 'danger', title: 'That suggestion wasn’t sent', description: describeError(error).message })
    } finally {
      setSendingDraft(false)
    }
  }

  const structural = async (changes: SheetChange[], done?: () => void) => {
    setBusy(true)
    const saved = await send(changes)
    setBusy(false)
    if (saved) done?.()
  }

  const chooseTab = (id: string) => {
    setSelection(START)
    setViewId(null)
    setParams(
      (current) => {
        const next = new URLSearchParams(current)
        if (id) next.set('tab', id)
        else next.delete('tab')
        return next
      },
      { replace: true },
    )
  }

  const archive = async (archived: boolean) => {
    try {
      const next = await updateSheet(projectId, sheetId, {
        archived,
        tab: tab?.id,
      })
      const current = client.getQueryData<SheetBody>(key)
      if (!current || next.sheet.revision >= current.sheet.revision) client.setQueryData(key, next)
      void client.invalidateQueries({ queryKey: sheetKeys.list(projectId) })
      showToast({
        tone: 'success',
        title: archived ? `${next.sheet.name} archived` : `${next.sheet.name} restored`,
      })
    } catch (error) {
      showToast({
        tone: 'danger',
        title: 'That didn’t work',
        description: describeError(error).message,
      })
    }
  }

  const box = bounds(selection)
  const rowCount = box.bottom - box.top + 1
  const columnCount = box.right - box.left + 1
  const plural = (count: number, one: string) => (count === 1 ? one : `${count} ${one}s`)
  // Sorting moves the data: the selected rows, or every row under the frozen ones.
  const sortBlock = (descending: boolean) => {
    if (!tab) return
    const whole = rowCount === 1
    const rows: [number, number] = whole ? [tab.frozenRows + 1, Math.max(tab.frozenRows + 1, used.rows)] : [box.top, box.bottom]
    const columns: [number, number] = whole ? [1, Math.max(1, used.columns)] : [Math.min(box.left, selection.focus.column), Math.max(box.right, selection.focus.column)]
    void structural([{ type: 'sort', tab: tab.id, rows, columns, by: [[selection.focus.column, descending]] }])
  }
  const sortTarget = rowCount === 1 ? 'the sheet' : `${plural(rowCount, 'row')}`
  const gridItems: MenuEntry[] = tab
    ? [
        { label: `Sort ${sortTarget} by column ${columnName(selection.focus.column)}, A to Z`, icon: 'arrow-down', onSelect: () => sortBlock(false) },
        { label: `Sort ${sortTarget} by column ${columnName(selection.focus.column)}, Z to A`, icon: 'arrow-up', onSelect: () => sortBlock(true) },
        { separator: true },
        {
          label: `Insert ${plural(rowCount, 'row')} above`,
          icon: 'arrow-up',
          onSelect: () =>
            void structural([
              {
                type: 'insertRows',
                tab: tab.id,
                row: box.top,
                count: rowCount,
              },
            ]),
        },
        {
          label: `Insert ${plural(rowCount, 'row')} below`,
          icon: 'arrow-down',
          onSelect: () =>
            void structural([
              {
                type: 'insertRows',
                tab: tab.id,
                row: box.bottom + 1,
                count: rowCount,
              },
            ]),
        },
        {
          label: `Delete ${plural(rowCount, 'row')}`,
          icon: 'trash',
          danger: true,
          onSelect: () =>
            void structural([
              {
                type: 'deleteRows',
                tab: tab.id,
                row: box.top,
                count: rowCount,
              },
            ]),
        },
        {
          label: `Insert ${plural(columnCount, 'column')} left`,
          icon: 'arrow-left',
          onSelect: () =>
            void structural([
              {
                type: 'insertColumns',
                tab: tab.id,
                column: box.left,
                count: columnCount,
              },
            ]),
        },
        {
          label: `Insert ${plural(columnCount, 'column')} right`,
          icon: 'chevron-right',
          onSelect: () =>
            void structural([
              {
                type: 'insertColumns',
                tab: tab.id,
                column: box.right + 1,
                count: columnCount,
              },
            ]),
        },
        {
          label: `Delete ${plural(columnCount, 'column')}`,
          icon: 'trash',
          danger: true,
          onSelect: () =>
            void structural([
              {
                type: 'deleteColumns',
                tab: tab.id,
                column: box.left,
                count: columnCount,
              },
            ]),
        },
        {
          label: 'Keep the first row in view',
          checked: tab.frozenRows > 0,
          onSelect: () =>
            void structural([
              {
                type: 'freezeRows',
                tab: tab.id,
                rows: tab.frozenRows > 0 ? 0 : 1,
              },
            ]),
        },
      ]
    : []
  // Formats and styles reach the whole selection; toggles follow the focused cell.
  const look = (change: LookChange) => {
    if (!tab) return
    void send([{ type: 'format', tab: tab.id, rows: [box.top, box.bottom], columns: [box.left, box.right], look: change }])
  }
  const focusStyle = focusCell?.style ?? {}
  const NUMBER_FORMATS: [string, string][] = [
    ['Automatic', ''],
    ['Number', '#,##0.00'],
    ['Percent', '0.0%'],
    ['Currency', '$#,##0.00'],
    ['Date', 'yyyy-mm-dd'],
    ['Plain text', '@'],
  ]
  const formatItems: MenuEntry[] = tab
    ? [
        { label: 'Bold', shortcut: 'Ctrl B', checked: focusStyle.bold === true, onSelect: () => look({ bold: !focusStyle.bold }) },
        { label: 'Italic', shortcut: 'Ctrl I', checked: focusStyle.italic === true, onSelect: () => look({ italic: !focusStyle.italic }) },
        { label: 'Underline', shortcut: 'Ctrl U', checked: focusStyle.underline === true, onSelect: () => look({ underline: !focusStyle.underline }) },
        { label: 'Strikethrough', checked: focusStyle.strike === true, onSelect: () => look({ strike: !focusStyle.strike }) },
        { label: 'Wrap text', checked: focusStyle.wrap === true, onSelect: () => look({ wrap: !focusStyle.wrap }) },
        { separator: true },
        { heading: 'Number format' },
        ...NUMBER_FORMATS.map(([label, code]): MenuEntry => ({
          label,
          checked: (focusCell?.format ?? '') === code,
          onSelect: () => look({ numberFormat: code }),
        })),
        { separator: true },
        { heading: 'Align' },
        ...(['left', 'center', 'right'] as const).map((align): MenuEntry => ({
          label: align[0].toUpperCase() + align.slice(1),
          checked: focusStyle.align === align,
          onSelect: () => look({ align: focusStyle.align === align ? '' : align }),
        })),
        { separator: true },
        { heading: 'Fill' },
        ...FILLS.map(([label, fill]): MenuEntry => ({
          label,
          checked: (focusStyle.fill ?? '') === fill,
          onSelect: () => look({ fill }),
        })),
        { separator: true },
        { label: 'Conditional formatting', onSelect: () => setEditingRules(true) },
        { label: 'Data validation', onSelect: () => setEditingValidation(true) },
        { label: 'Clear formatting', onSelect: () => look({ clear: true }) },
      ]
    : []
  const mayChange = (view: SheetView) => !view.shared || editable
  const viewItems: MenuEntry[] = tab
    ? [
        { label: 'All rows', checked: !activeView, onSelect: () => setViewId(null) },
        ...(views.data?.views ?? []).map((view): MenuEntry => ({
          label: view.shared ? view.name : `${view.name} (only you)`,
          checked: activeView?.id === view.id,
          onSelect: () => setViewId(view.id),
        })),
        { separator: true },
        { label: 'New filter view', icon: 'plus', onSelect: () => setViewDialog({ view: null }) },
        ...(activeView && mayChange(activeView)
          ? [
              { label: 'Edit this view', icon: 'pencil' as const, onSelect: () => setViewDialog({ view: activeView }) },
              {
                label: 'Remove this view',
                icon: 'trash' as const,
                danger: true,
                onSelect: () =>
                  void deleteSheetView(client, activeView)
                    .then(() => setViewId(null))
                    .catch((error: unknown) => showToast({ tone: 'danger', title: 'That view wasn’t removed', description: describeError(error).message })),
              },
            ]
          : []),
      ]
    : []
  const download = (format: 'xlsx' | 'csv') => {
    const link = document.createElement('a')
    link.href = sheetExportHref(projectId, sheetId, format, body.tab)
    link.download = ''
    link.rel = 'noopener'
    document.body.append(link)
    link.click()
    link.remove()
  }
  const sheetItems: MenuEntry[] = [
    { label: 'Download as Excel (.xlsx)', icon: Download, onSelect: () => download('xlsx') },
    { label: 'Download this tab as CSV', icon: Download, onSelect: () => download('csv') },
    { label: body.ranges.length ? `Named ranges and links (${body.ranges.length})` : 'Named ranges and links', onSelect: () => setNaming(true) },
    { label: body.charts.length ? `Charts (${body.charts.length})` : 'Charts', onSelect: () => setCharting(true) },
    { label: 'History', icon: 'clock', onSelect: () => setHistoryOpen(true) },
    ...(editable ? [{ separator: true as const }] : []),
    ...(editable && !body.sheet.archived
      ? [
          {
            label: 'Edit name and description',
            icon: 'pencil' as const,
            onSelect: () => setDialog({ kind: 'details' }),
          },
        ]
      : []),
    ...(editable
      ? [
          body.sheet.archived
            ? {
                label: 'Restore sheet',
                icon: 'retry' as const,
                onSelect: () => void archive(false),
              }
            : {
                label: 'Archive sheet',
                icon: 'archive' as const,
                onSelect: () => void archive(true),
              },
        ]
      : []),
  ]
  const tabItems = (entry: SheetTab): MenuEntry[] => [
    {
      label: 'Rename tab',
      icon: 'pencil',
      onSelect: () => setDialog({ kind: 'rename-tab', tab: entry }),
    },
    ...(entry.position > 0
      ? [
          {
            label: 'Move left',
            icon: 'arrow-left' as const,
            onSelect: () => void structural([{ type: 'moveTab', tab: entry.id, position: entry.position }]),
          },
        ]
      : []),
    ...(entry.position < tabs.length - 1
      ? [
          {
            label: 'Move right',
            icon: 'chevron-right' as const,
            onSelect: () =>
              void structural([
                {
                  type: 'moveTab',
                  tab: entry.id,
                  position: entry.position + 2,
                },
              ]),
          },
        ]
      : []),
    ...(tabs.length > 1
      ? [
          {
            label: 'Remove tab',
            icon: 'trash' as const,
            danger: true,
            onSelect: () => setDialog({ kind: 'remove-tab', tab: entry }),
          },
        ]
      : []),
  ]
  const writable = editable && !body.sheet.archived

  return (
    <>
      <div
        className="aw-page aw-page--full aw-sheet"
        onKeyDown={(event) => {
          // Ctrl or Cmd with F finds in the sheet rather than the page.
          if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'f' && !phone) {
            event.preventDefault()
            setFinding(true)
          }
        }}
      >
        <PageHeader
          actions={sheetItems.length ? <Menu items={sheetItems} label="Sheet actions" placement="bottom-end" trigger={<IconButton icon="more" label="Sheet actions" />} /> : null}
          icon="table"
          title={body.sheet.name}
        >
          {body.sheet.description ? <p className="aw-sheet__description">{body.sheet.description}</p> : null}
        </PageHeader>
        {body.sheet.archived ? (
          <Banner tone="warning">
            This sheet is archived, so it can’t be changed.{' '}
            {editable ? (
              <Button onClick={() => void archive(false)} size="sm" variant="ghost">
                Restore it
              </Button>
            ) : null}
          </Banner>
        ) : null}
        {suggesting ? (
          <div className="aw-sheet__suggesting">
            <Banner tone="info" title={draft.size ? `${draft.size === 1 ? 'One cell' : `${draft.size} cells`} to suggest` : 'You’re suggesting changes'}>
              {draft.size ? 'Send them as one suggestion for someone who edits this sheet.' : 'Type in cells as usual; nothing changes until someone who edits this sheet accepts.'}
            </Banner>
            {draft.size ? (
              <>
                <Button busy={sendingDraft} onClick={() => void sendDraft()} size="sm" variant="primary">
                  Send suggestion
                </Button>
                <Button disabled={sendingDraft} onClick={() => setDraft(new Map())} size="sm" variant="ghost">
                  Discard
                </Button>
              </>
            ) : null}
          </div>
        ) : null}
        {showsRecent ? <RecentEditsBar canUndo={editable && !body.sheet.archived} change={recent.newest} kind="sheet" onUndone={() => void refresh()} /> : null}
        {phone ? (
          <SheetCards body={body} onOpen={(row, column) => (writable ? setDialog({ kind: 'cell', row, column, revision: body.sheet.revision }) : undefined)} tab={tab} />
        ) : (
          <>
            <div className="aw-sheet__bar">
              <span className="aw-sheet__address">{cellName(selection.focus)}</span>
              <span className="aw-sheet__formula" title={focusCell?.input}>
                {tabPending.get(cellKey(selection.focus.row, selection.focus.column)) ?? focusCell?.input ?? ''}
              </span>
              {showComments && comments.data && (comments.data.acceptsComments || focusThread) ? (
                <Button icon="message" onClick={openComments} size="sm" variant="ghost">
                  {focusThread ? 'Comments' : 'Comment'}
                </Button>
              ) : null}
              <Button icon="search" onClick={() => setFinding(true)} size="sm" variant="ghost">
                Find
              </Button>
              {showsRecent ? <RecentEditsToggle on={allEdits} onChange={setAllEdits} /> : null}
              {proposals.length ? (
                <Button onClick={() => setReviewing(true)} size="sm" variant="ghost">
                  {`Proposals (${proposals.length})`}
                </Button>
              ) : null}
              {viewItems.length ? (
                <Menu
                  items={viewItems}
                  label="Filter views"
                  placement="bottom-end"
                  trigger={
                    <Button size="sm" variant="ghost">
                      {activeView ? activeView.name : 'All rows'}
                    </Button>
                  }
                />
              ) : null}
              {writable && formatItems.length ? (
                <Menu
                  items={formatItems}
                  label="Format"
                  placement="bottom-end"
                  trigger={
                    <Button disabled={busy} size="sm" variant="ghost">
                      Format
                    </Button>
                  }
                />
              ) : null}
              {writable && gridItems.length ? (
                <Menu
                  items={gridItems}
                  label="Rows and columns"
                  placement="bottom-end"
                  trigger={
                    <Button disabled={busy} icon="table" size="sm" variant="ghost">
                      Rows and columns
                    </Button>
                  }
                />
              ) : null}
            </div>
            {finding ? (
              <SheetFind
                editable={writable}
                onClose={() => setFinding(false)}
                onGo={(tabId, row, column) => {
                  if (tabId !== tab?.id) chooseTab(tabId)
                  else if (shown && row > tab.frozenRows && !shown.rows.includes(row)) setViewId(null)
                  setSelection({ anchor: { row, column }, focus: { row, column } })
                }}
                onReplace={async (changes) => {
                  const saved = await send(changes)
                  if (saved) void refresh()
                  return saved
                }}
                sheetId={sheetId}
                tabs={tabs}
              />
            ) : null}
            {tab ? (
              <SheetGrid
                cells={cells}
                commented={marks}
                edited={editedHere}
                editable={writable || suggesting}
                key={tab.id}
                onEdits={suggesting ? suggestEdit : edit}
                onResize={(column, width) => (writable ? void send([{ type: 'columnWidth', tab: tab.id, column, width }]) : undefined)}
                onSelect={setSelection}
                onStyle={writable ? (style) => look({ [style]: !focusStyle[style] }) : undefined}
                pending={tabPending}
                proposed={proposedHere}
                revision={body.sheet.revision}
                selection={selection}
                shown={shown}
                size={size}
                tab={tab}
              />
            ) : null}
          </>
        )}
        <nav aria-label="Tabs" className="aw-sheet__tabs">
          {tabs.map((entry) => (
            <span className={['aw-sheet__tab', entry.id === tab?.id ? 'is-current' : ''].join(' ')} key={entry.id}>
              <button aria-current={entry.id === tab?.id ? 'page' : undefined} className="aw-sheet__tabname" onClick={() => chooseTab(entry.id)} type="button">
                {entry.name}
              </button>
              {writable && entry.id === tab?.id ? (
                <Menu items={tabItems(entry)} label={`${entry.name} tab actions`} trigger={<IconButton icon="chevron-down" label={`${entry.name} tab actions`} size="sm" />} />
              ) : null}
            </span>
          ))}
          {writable ? <IconButton icon="plus" label="Add a tab" onClick={() => setDialog({ kind: 'add-tab' })} size="sm" /> : null}
        </nav>
      </div>
      {commenting && comments.data ? (
        <SidePanel onClose={() => setCommenting(null)} title={`Comments on ${commenting.label}`}>
          <AnchoredThreadPanel
            acceptsComments={comments.data.acceptsComments}
            anchor={commenting.anchor}
            kind="sheet"
            onStarted={(threadId) => setCommenting((open) => (open ? { ...open, threadId } : open))}
            spaceId={comments.data.spaceId ?? projectId}
            thingId={sheetId}
            thread={commentingThread}
            viewerId={user.actorId}
          />
        </SidePanel>
      ) : null}
      {historyOpen ? (
        <SidePanel onClose={() => setHistoryOpen(false)} title="History">
          <HistoryPanel
            emptyText="A version is kept as soon as the sheet changes."
            label="Versions"
            restore={editable && !body.sheet.archived ? { expectedVersion: body.sheet.revision, onRestored: () => void refresh() } : undefined}
            thing={sheetId}
          />
        </SidePanel>
      ) : null}
      {reviewing ? (
        <SheetProposalsDialog
          canDecide={body.canEdit && !body.sheet.archived}
          onClose={() => setReviewing(false)}
          onDecided={() => void refresh()}
          proposals={proposals}
          userId={user.actorId}
        />
      ) : null}
      {editingRules && tab ? (
        <SheetRulesDialog
          onClose={() => setEditingRules(false)}
          onSave={async (rules) => {
            const saved = await send([{ type: 'conditionalFormats', tab: tab.id, rules }])
            if (saved) void refresh()
            return saved
          }}
          selection={`${cellName({ row: box.top, column: box.left })}:${cellName({ row: box.bottom, column: box.right })}`}
          tab={tab}
        />
      ) : null}
      {editingValidation && tab ? (
        <SheetValidationDialog
          onClose={() => setEditingValidation(false)}
          onSave={async (rules) => {
            const saved = await send([{ type: 'validations', tab: tab.id, rules }])
            if (saved) void refresh()
            return saved
          }}
          selection={`${cellName({ row: box.top, column: box.left })}:${cellName({ row: box.bottom, column: box.right })}`}
          tab={tab}
        />
      ) : null}
      {charting && tab ? (
        <SheetChartsDialog
          canEdit={editable && !body.sheet.archived}
          charts={body.charts}
          onChanged={() => void refresh()}
          onClose={() => setCharting(false)}
          selection={`${/^[A-Za-z0-9_]+$/.test(tab.name) ? tab.name : `'${tab.name.replace(/'/g, "''")}'`}!${cellName({ row: box.top, column: box.left })}:${cellName({ row: box.bottom, column: box.right })}`}
          sheetId={sheetId}
        />
      ) : null}
      {naming && tab ? (
        <SheetRangesDialog
          canEdit={editable && !body.sheet.archived}
          onChanged={() => void refresh()}
          onClose={() => setNaming(false)}
          ranges={body.ranges}
          references={experience.shows('page.references')}
          selection={`${cellName({ row: box.top, column: box.left })}:${cellName({ row: box.bottom, column: box.right })}`}
          sheetId={sheetId}
          tab={tab.name}
        />
      ) : null}
      {viewDialog && tab ? (
        <SheetViewDialog
          canShare={editable}
          columns={Math.max(used.columns, 1)}
          onClose={() => setViewDialog(null)}
          onSaved={(view) => setViewId(view.id)}
          sheetId={sheetId}
          tabId={tab.id}
          view={viewDialog.view}
        />
      ) : null}
      {dialog?.kind === 'details' ? (
        <SheetDetailsDialog
          onClose={() => setDialog(null)}
          onSaved={(next) => {
            // The answer holds this tab's cells as of its revision: keep it
            // whole, unless the screen has already read further.
            const current = client.getQueryData<SheetBody>(key)
            if (!current || next.sheet.revision >= current.sheet.revision) client.setQueryData(key, next)
          }}
          projectId={projectId}
          sheet={body.sheet}
          tab={tab?.id}
        />
      ) : null}
      {dialog?.kind === 'add-tab' || dialog?.kind === 'rename-tab' ? (
        <TabNameDialog
          busy={busy}
          initial={dialog.kind === 'rename-tab' ? dialog.tab.name : `Sheet${tabs.length + 1}`}
          onClose={() => {
            setDialog(null)
            setProblem(null)
          }}
          onSave={(name) => {
            setProblem(null)
            const changes: SheetChange[] = dialog.kind === 'rename-tab' ? [{ type: 'renameTab', tab: dialog.tab.id, name }] : [{ type: 'addTab', name }]
            setBusy(true)
            changeSheet(projectId, sheetId, changes)
              .then(() => {
                setDialog(null)
                void refresh()
              })
              .catch((error: unknown) => setProblem(describeError(error).message))
              .finally(() => setBusy(false))
          }}
          problem={problem}
          title={dialog.kind === 'rename-tab' ? 'Rename tab' : 'New tab'}
        />
      ) : null}
      {dialog?.kind === 'remove-tab' ? (
        <ConfirmDialog
          busy={busy}
          confirmLabel="Remove tab"
          onClose={() => setDialog(null)}
          onConfirm={() =>
            void structural([{ type: 'removeTab', tab: dialog.tab.id }], () => {
              setDialog(null)
              chooseTab('')
            })
          }
          open
          title={`Remove ${dialog.tab.name}?`}
        >
          Its cells go with it, and formulas on other tabs that used them show #REF!.
        </ConfirmDialog>
      ) : null}
      {dialog?.kind === 'cell' && tab ? (
        <CellDialog
          initial={cells.get(cellKey(dialog.row, dialog.column))?.input ?? ''}
          name={cellName(dialog)}
          onClose={() => setDialog(null)}
          onSave={(input) => {
            edit([{ row: dialog.row, column: dialog.column, input, revision: dialog.revision }])
            setDialog(null)
          }}
        />
      ) : null}
    </>
  )
}

/** A tab's rows as cards, for a phone: row 1 names each value. */
function SheetCards({ body, onOpen, tab }: { body: SheetBody; onOpen: (row: number, column: number) => void; tab: SheetTab | undefined }) {
  const rows = cards(body, tab?.frozenRows ?? 1)
  if (!rows.length) return <p className="aw-sheet__empty">Nothing here yet. Open this sheet on a bigger screen to fill it in.</p>
  return (
    <ul aria-label="Rows" className="aw-sheet__cards">
      {rows.map((card) => (
        <li className="aw-sheet__card" key={card.row}>
          <span className="aw-sheet__cardrow">Row {card.row}</span>
          <dl>
            {card.fields.map((field) => (
              <div key={field.column}>
                <dt>{field.label}</dt>
                <dd>
                  <button onClick={() => onOpen(card.row, field.column)} type="button">
                    {field.text}
                  </button>
                </dd>
              </div>
            ))}
          </dl>
        </li>
      ))}
    </ul>
  )
}

function CellDialog({ initial, name, onClose, onSave }: { initial: string; name: string; onClose: () => void; onSave: (input: string) => void }) {
  const [input, setInput] = useState(initial)
  return (
    <Dialog
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        onSave(input)
      }}
      open
      title={`Edit ${name}`}
    >
      <TextField autoFocus hint="A value, or a formula starting with =." label={name} onChange={(event) => setInput(event.target.value)} value={input} />
    </Dialog>
  )
}
