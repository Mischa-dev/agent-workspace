import { useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession, type SessionUser } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { AppShell } from '../shell/AppShell'
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
  TextField,
  describeError,
  showToast,
  type MenuEntry,
} from '../ui'
import { usePhoneWidth } from '../work-items/useCoarsePointer'
import { changeSheet, sheetKeys, updateSheet, useSheet, type SheetBody, type SheetChange, type SheetTab } from './api'
import { SheetDetailsDialog, TabNameDialog } from './SheetDialogs'
import { SheetGrid, type GridEdit } from './SheetGrid'
import { bounds, cards, cellKey, cellMap, cellName, extent, patched, type Selection } from './sheetModel'

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
// Cells this screen cleared, by sheet then `tab:row:column`, with the
// revision that cleared them: a queued edit of the same cell builds on it.
const cleared = new Map<string, Map<string, number>>()

const START: Selection = {
  anchor: { row: 1, column: 1 },
  focus: { row: 1, column: 1 },
}

/** One sheet: a grid you edit in place (cards on a phone), its tabs under it. */
export function SheetPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', sheetId = '' } = useParams()
  const project = useProject(projectId)
  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading the sheet" />
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
  return <SheetWorkspace key={sheetId} project={project.data} sheetId={sheetId} user={session.user} />
}

function SheetWorkspace({ project, sheetId, user }: { project: Project; sheetId: string; user: SessionUser }) {
  const projectId = project.id
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
  usePageTitle(sheet.data?.sheet.name ?? null)

  const body = sheet.data
  const cells = useMemo(() => cellMap(body?.cells ?? []), [body?.cells])

  if (sheet.isPending) {
    return (
      <AppShell project={project} user={user}>
        <div aria-busy="true" className="aw-page aw-page--full">
          <ListSkeleton label="Loading the sheet" rows={6} />
        </div>
      </AppShell>
    )
  }
  if (!body) {
    const gone = isApiError(sheet.error) && sheet.error.status === 404
    return (
      <AppShell project={project} user={user}>
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
      </AppShell>
    )
  }

  const tabs = [...body.tabs].sort((left, right) => left.position - right.position)
  const tab = tabs.find((entry) => entry.id === body.tab) ?? tabs[0]
  const editable = body.canEdit && !project.archivedAt
  const used = extent({ rows: tab?.rows ?? 0, columns: tab?.columns ?? 0 })
  const size = {
    rows: Math.max(used.rows, selection.focus.row + 20),
    columns: Math.max(used.columns, Math.min(200, selection.focus.column + 2)),
  }
  const focusCell = cells.get(cellKey(selection.focus.row, selection.focus.column))
  const prefix = `${tab?.id ?? ''}:`
  const tabPending = new Map([...pending].filter(([at]) => at.startsWith(prefix)).map(([at, input]) => [at.slice(prefix.length), input]))
  const scope = sheetKeys.one(projectId, sheetId)
  const key = sheetKeys.tab(projectId, sheetId, tabParam)

  const refresh = () => client.invalidateQueries({ queryKey: scope })

  /** Sends changes after any still on their way; true when they saved. */
  const send = (changes: SheetChange[], shown?: Map<string, string>) => {
    const run = (writes.get(sheetId) ?? Promise.resolve()).then(async () => {
      try {
        const mine = own.get(sheetId) ?? new Set<number>()
        const clears = cleared.get(sheetId) ?? new Map<string, number>()
        const known = cellMap(client.getQueryData<SheetBody>(key)?.cells ?? [])
        const result = await changeSheet(
          projectId,
          sheetId,
          changes.map((change) => {
            if (change.type !== 'cell' || change.ifRevision === undefined) return change
            const seen = known.get(cellKey(change.row, change.column))?.revision ?? clears.get(`${change.tab}:${change.row}:${change.column}`) ?? 0
            return seen > change.ifRevision && mine.has(seen) ? { ...change, ifRevision: seen } : change
          }),
        )
        // Only a revision this write made is ours; one that changed nothing
        // answers with whatever the sheet was at, maybe someone else's.
        if (result.revision !== result.previousRevision) own.set(sheetId, mine.add(result.revision))
        for (const change of changes) {
          if (change.type !== 'cell') continue
          const at = `${change.tab}:${change.row}:${change.column}`
          if (result.revision === result.previousRevision) continue
          if (change.input === '') clears.set(at, result.revision)
          else clears.delete(at)
        }
        cleared.set(sheetId, clears)
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

  const structural = async (changes: SheetChange[], done?: () => void) => {
    setBusy(true)
    const saved = await send(changes)
    setBusy(false)
    if (saved) done?.()
  }

  const chooseTab = (id: string) => {
    setSelection(START)
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
  const gridItems: MenuEntry[] = tab
    ? [
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
  const sheetItems: MenuEntry[] = [
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
    <AppShell project={project} user={user}>
      <div className="aw-page aw-page--full aw-sheet">
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
        {phone ? (
          <SheetCards body={body} onOpen={(row, column) => (writable ? setDialog({ kind: 'cell', row, column, revision: body.sheet.revision }) : undefined)} tab={tab} />
        ) : (
          <>
            <div className="aw-sheet__bar">
              <span className="aw-sheet__address">{cellName(selection.focus)}</span>
              <span className="aw-sheet__formula" title={focusCell?.input}>
                {tabPending.get(cellKey(selection.focus.row, selection.focus.column)) ?? focusCell?.input ?? ''}
              </span>
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
            {tab ? (
              <SheetGrid
                cells={cells}
                editable={writable}
                key={tab.id}
                onEdits={edit}
                onResize={(column, width) => void send([{ type: 'columnWidth', tab: tab.id, column, width }])}
                onSelect={setSelection}
                pending={tabPending}
                revision={body.sheet.revision}
                selection={selection}
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
    </AppShell>
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
