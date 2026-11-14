import { Suspense, lazy, useCallback, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession, type SessionUser } from '../auth/session'
import { useFieldMutations } from '../fields/api'
import { clausesWithinLimits, type FieldClause, type FieldDefinition } from '../fields/fieldModel'
import { FieldDialog } from '../fields/FieldManager'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { AppShell } from '../shell/AppShell'
import { usePageTitle } from '../shell/documentTitleModel'
import { Banner, Button, ButtonLink, ConfirmDialog, ErrorState, IconButton, ListSkeleton, Menu, PageHeader, PageTabs, describeError, showToast, type MenuEntry } from '../ui'
import { FieldFilterEditor } from '../work-items/FieldFilters'
import { useDebouncedValue } from '../work-items/pickerModel'
import { usePhoneWidth } from '../ui/useCoarsePointer'
import {
  collectionKeys,
  createView,
  deleteView,
  fieldScope,
  recordsParams,
  type RecordsQuery,
  updateCollection,
  updateView,
  useCollection,
  useCollections,
  type CollectionBody,
  type CollectionView,
  type ViewConfig,
} from './api'
import { activeConfig, boardFields, dateFields, sameConfig, withColumnHidden } from './collectionModel'
import { CollectionDetailsDialog, ViewNameDialog } from './CollectionDialogs'
import { CollectionToolbar } from './CollectionToolbar'
import type { ColumnActions } from './ColumnMenus'
import { RecordBoard } from './RecordBoard'
import { RecordCalendar } from './RecordCalendar'
import { RecordCards } from './RecordCards'
import { RecordGallery } from './RecordGallery'
import { RecordPanel } from './RecordPanel'
import { RecordTable } from './RecordTable'
import { useRecordEdits } from './useRecordEdits'

// Forms are managed now and then, so their dialog loads when it opens.
const FormsDialog = lazy(() => import('./FormsDialog').then((module) => ({ default: module.FormsDialog })))

const NO_CONFIG: ViewConfig = { filters: [], sort: [], groupBy: null, columns: [], titleWidth: null, dateField: null }

type Dialogs =
  | { kind: 'details' }
  | { kind: 'forms' }
  | { kind: 'field'; field: FieldDefinition | null }
  | { kind: 'new-view' }
  | { kind: 'rename-view'; view: CollectionView }
  | { kind: 'delete-view'; view: CollectionView }

type Filtering = { field: FieldDefinition; index: number | null; anchor: HTMLElement }

/** Whether two views find the same records: the same filters, sort and grouping. */
const sameQuery = (left: ViewConfig, right: ViewConfig) =>
  sameConfig({ ...left, columns: [], titleWidth: null, dateField: null }, { ...right, columns: [], titleWidth: null, dateField: null })

/** One collection: its views as tabs, a table you edit in place (cards on a phone), and a record beside it. */
export function CollectionPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', collectionId = '' } = useParams()
  const project = useProject(projectId)
  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading the collection" />
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
  // A collection's lists and drafts start fresh when another one opens.
  return <CollectionWorkspace collectionId={collectionId} key={collectionId} project={project.data} user={session.user} />
}

function CollectionWorkspace({ collectionId, project, user }: { collectionId: string; project: Project; user: SessionUser }) {
  const projectId = project.id
  const client = useQueryClient()
  const navigate = useNavigate()
  const collection = useCollection(projectId, collectionId)
  const edits = useRecordEdits(projectId, collectionId)
  const fieldChanges = useFieldMutations(fieldScope(projectId, collectionId))
  const phone = usePhoneWidth()
  const [params, setParams] = useSearchParams()
  const [drafts, setDrafts] = useState<Record<string, ViewConfig>>({})
  const [search, setSearch] = useState('')
  const searched = useDebouncedValue(search, 250)
  const [showArchived, setShowArchived] = useState(false)
  const [dialog, setDialog] = useState<Dialogs | null>(null)
  const collections = useCollections(projectId)
  const [filtering, setFiltering] = useState<Filtering | null>(null)
  const [saving, setSaving] = useState(false)
  const [deleteProblem, setDeleteProblem] = useState<string | null>(null)
  const recordId = params.get('record')
  usePageTitle(collection.data?.collection.name ?? null)

  const openRecord = useCallback(
    (id: string) =>
      setParams(
        (current) => {
          const next = new URLSearchParams(current)
          next.set('record', id)
          return next
        },
        { replace: Boolean(recordId) },
      ),
    [recordId, setParams],
  )
  const closeRecord = useCallback(
    () =>
      setParams((current) => {
        const next = new URLSearchParams(current)
        next.delete('record')
        return next
      }),
    [setParams],
  )

  if (collection.isPending) {
    return (
      <AppShell project={project} user={user}>
        <div aria-busy="true" className="aw-page aw-page--full">
          <ListSkeleton label="Loading the collection" rows={6} />
        </div>
      </AppShell>
    )
  }
  // A refetch that fails keeps showing what loaded before.
  if (!collection.data) {
    const gone = isApiError(collection.error) && collection.error.status === 404
    return (
      <AppShell project={project} user={user}>
        <div className="aw-page">
          {gone ? (
            <ErrorState
              action={
                <ButtonLink to={`/projects/${encodeURIComponent(projectId)}/collections`} variant="primary">
                  See the collections
                </ButtonLink>
              }
              title="This collection isn’t here"
            >
              It may have been removed, or the link is wrong.
            </ErrorState>
          ) : (
            <ErrorState error={collection.error} onRetry={() => void collection.refetch()} title="The collection didn’t load" />
          )}
        </div>
      </AppShell>
    )
  }

  // The built-in Tasks collection (AWD-110) reads the project's tasks, and
  // the records of a collection that acts like a task are tasks too: a
  // record opens its task, and an edit here is a task change. Tasks' name
  // and fields are the project's, changed in the task settings.
  const tasks = collection.data.collection.builtin === 'tasks'
  const taskLike = tasks || collection.data.collection.actsLikeTask === true
  // A link to such a record (an embed, an old link) opens its task.
  if (taskLike && recordId) return <Navigate replace to={`/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(recordId)}`} />
  const body = tasks ? { ...collection.data, canBuild: false } : collection.data
  const canSaveViews = collection.data.canBuild && !(Boolean(project.archivedAt) || body.collection.archived)
  const views = [...body.views].sort((left, right) => left.position - right.position)
  const view = views.find((entry) => entry.id === params.get('view')) ?? views[0] ?? null
  // What the view and its unsaved changes say, without what names an archived field.
  const viewConfig = view ? activeConfig(view.config, body.fields) : NO_CONFIG
  const unsaved = view ? drafts[view.id] : undefined
  const draft = unsaved && activeConfig(unsaved, body.fields)
  const config = draft ?? viewConfig
  const changed = Boolean(view && draft && !sameConfig(draft, viewConfig))
  const readOnly = Boolean(project.archivedAt) || body.collection.archived
  const canBuild = body.canBuild && !readOnly
  const editable = body.canEditRecords && !readOnly && !showArchived
  const filtered = config.filters.length > 0 || Boolean(searched.trim())
  const archived = showArchived ? ('only' as const) : undefined
  const recordsQuery: RecordsQuery =
    view && draft && !sameQuery(draft, viewConfig)
      ? { view: view.id, filters: config.filters, sort: config.sort, groupBy: config.groupBy, groupBucket: config.groupBucket, search: searched, archived }
      : { view: view?.id ?? null, search: searched, archived }
  const recordQuery = recordsParams(recordsQuery)
  const layout = view?.type ?? 'table'
  const recordHref = (id: string) => {
    if (taskLike) return `/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(id)}`
    const next = new URLSearchParams(params)
    next.set('record', id)
    return `?${next.toString()}`
  }

  const setConfig = (next: ViewConfig) => {
    if (view) setDrafts((current) => ({ ...current, [view.id]: next }))
  }
  /** Puts a view back as saved. After a save, `sent` is the draft it saved: one changed since stays. */
  const dropDraft = (viewId: string, sent?: ViewConfig) =>
    setDrafts((current) => {
      const now = current[viewId]
      if (!now || (sent && now !== sent)) return current
      const next = { ...current }
      delete next[viewId]
      return next
    })
  const keepView = (saved: CollectionView) => {
    client.setQueryData<CollectionBody>(collectionKeys.one(projectId, collectionId), (current) =>
      current ? { ...current, views: current.views.some((entry) => entry.id === saved.id) ? current.views.map((entry) => (entry.id === saved.id ? saved : entry)) : [...current.views, saved] } : current,
    )
  }
  const saveView = async () => {
    if (!view || !draft) return
    setSaving(true)
    try {
      keepView(await updateView(projectId, collectionId, view, { config: draft }))
      dropDraft(view.id, unsaved)
      void client.invalidateQueries({ queryKey: collectionKeys.one(projectId, collectionId) })
      showToast({ tone: 'success', title: `${view.name} saved` })
    } catch (error) {
      const conflict = isApiError(error) && error.code === 'version_conflict'
      showToast({
        tone: 'danger',
        title: `${view.name} wasn’t saved`,
        description: conflict ? 'Someone changed this view first. Its newest version is loading; save again to put yours on top.' : describeError(error).message,
      })
      if (conflict) void collection.refetch()
    } finally {
      setSaving(false)
    }
  }
  const setCollectionArchived = async (next: boolean) => {
    try {
      const saved = await updateCollection(projectId, collectionId, { expectedVersion: body.collection.version, archived: next })
      client.setQueryData(collectionKeys.one(projectId, collectionId), saved)
      void client.invalidateQueries({ queryKey: collectionKeys.all(projectId) })
      showToast({
        tone: 'success',
        title: next ? `${body.collection.name} archived` : `${body.collection.name} restored`,
        ...(next
          ? {
              onUndo: async () => {
                const restored = await updateCollection(projectId, collectionId, { expectedVersion: saved.collection.version, archived: false })
                client.setQueryData(collectionKeys.one(projectId, collectionId), restored)
                void client.invalidateQueries({ queryKey: collectionKeys.all(projectId) })
              },
            }
          : {}),
      })
    } catch (error) {
      showToast({ tone: 'danger', title: next ? 'The collection wasn’t archived' : 'The collection wasn’t restored', description: describeError(error).message })
    }
  }
  const archiveField = (field: FieldDefinition) =>
    fieldChanges.update.mutate(
      { fieldId: field.id, expectedVersion: field.version, archived: true, idempotencyKey: crypto.randomUUID() },
      {
        onSuccess: (saved) =>
          showToast({
            tone: 'success',
            title: `${field.name} archived`,
            description: 'Records keep its values for when it comes back.',
            onUndo: () => fieldChanges.update.mutateAsync({ fieldId: field.id, expectedVersion: saved.version, archived: false, idempotencyKey: crypto.randomUUID() }),
          }),
        onError: (error) => showToast({ tone: 'danger', title: `${field.name} wasn’t archived`, description: describeError(error).message }),
      },
    )
  const actions: ColumnActions = {
    sort: (rule) => setConfig({ ...config, sort: [rule] }),
    filter: (field, anchor) => setFiltering({ field, index: null, anchor }),
    group: (fieldId) => setConfig({ ...config, groupBy: fieldId }),
    hide: (fieldId) => setConfig(withColumnHidden(body.fields, config, fieldId, true)),
    editField: (field) => setDialog({ kind: 'field', field }),
    archiveField,
    renameTitle: () => setDialog({ kind: 'details' }),
    addField: () => setDialog({ kind: 'field', field: null }),
  }
  const clearFilters = () => {
    setConfig({ ...config, filters: [] })
    setSearch('')
  }
  const newRecord = async () => {
    const added = await edits.add('')
    if (added) openRecord(added.id)
  }

  const collectionMenu: MenuEntry[] = [
    ...(body.canEditRecords && !taskLike ? [{ label: 'Forms…', icon: 'list' as const, onSelect: () => setDialog({ kind: 'forms' }) }] : []),
    ...(canBuild
      ? ([
          { label: 'Edit details…', icon: 'pencil', onSelect: () => setDialog({ kind: 'details' }) },
          { label: 'Add a field…', icon: 'plus', onSelect: () => setDialog({ kind: 'field', field: null }) },
          { separator: true },
          { label: 'Archive collection', icon: 'archive', onSelect: () => void setCollectionArchived(true) },
        ] satisfies MenuEntry[])
      : []),
  ]
  const viewMenu: MenuEntry[] = view
    ? [
        { label: 'Rename view…', icon: 'pencil', onSelect: () => setDialog({ kind: 'rename-view', view }) },
        ...(views.length > 1 ? [{ label: 'Delete view…', icon: 'trash' as const, danger: true, onSelect: () => setDialog({ kind: 'delete-view', view }) }] : []),
      ]
    : []
  const shared = {
    body,
    config,
    editable,
    edits,
    filtered,
    onClearFilters: clearFilters,
    onOpenRecord: taskLike ? (id: string) => navigate(recordHref(id)) : openRecord,
    openRecordId: recordId,
    params: recordQuery,
    projectId,
  }
  const panel = recordId && !taskLike ? (
    <RecordPanel body={body} canEdit={body.canEditRecords && !readOnly} edits={edits} onClose={closeRecord} projectId={projectId} recordId={recordId} />
  ) : undefined

  return (
    <AppShell context={panel} contextKey={recordId ?? undefined} onContextDismiss={closeRecord} project={project} user={user}>
      <div className="aw-page aw-page--full aw-collection">
        <PageHeader
          actions={
            <>
              {collectionMenu.length ? (
                <Menu items={collectionMenu} label="Collection options" placement="bottom-end" trigger={<IconButton icon="more" label="Collection options" />} />
              ) : null}
              {editable ? (
                <Button icon="plus" onClick={() => void newRecord()} variant="primary">
                  New record
                </Button>
              ) : null}
            </>
          }
          icon="table"
          title={body.collection.name}
        />
        {body.collection.archived ? (
          <Banner className="aw-page__banner" icon="archive" tone="warning">
            This collection is archived, so its records can’t change.{' '}
            {body.canBuild && !project.archivedAt ? (
              <Button onClick={() => void setCollectionArchived(false)} size="sm">
                Restore
              </Button>
            ) : null}
          </Banner>
        ) : null}
        <div className="aw-collection__views">
          <PageTabs current={view?.id ?? ''} label="Views" tabs={views.map((entry) => ({ id: entry.id, label: entry.name, to: `?view=${encodeURIComponent(entry.id)}` }))} />
          {canSaveViews ? (
            <>
              <IconButton icon="plus" label="New view" onClick={() => setDialog({ kind: 'new-view' })} size="sm" />
              {viewMenu.length ? <Menu items={viewMenu} label="View options" trigger={<IconButton icon="more" label="View options" size="sm" />} /> : null}
            </>
          ) : null}
        </div>
        <CollectionToolbar
          archived={showArchived}
          body={body}
          canBuild={canBuild}
          canSave={canSaveViews}
          changed={changed}
          config={config}
          layout={layout}
          onAddField={() => setDialog({ kind: 'field', field: null })}
          onArchived={setShowArchived}
          onChange={setConfig}
          onEditFilter={(index, anchor) => {
            const field = body.fields.find((entry) => entry.id === config.filters[index]?.field)
            if (field) setFiltering({ field, index, anchor })
          }}
          onFilter={(field, anchor) => setFiltering({ field, index: null, anchor })}
          onReset={() => view && dropDraft(view.id)}
          onSave={() => void saveView()}
          onSearch={setSearch}
          projectId={projectId}
          saving={saving}
          search={search}
        />
        {showArchived ? (
          <Banner className="aw-page__banner" icon="archive">
            Showing archived records. Open one to restore it.
          </Banner>
        ) : null}
        {layout === 'board' ? (
          <RecordBoard
            {...shared}
            groupable={boardFields(body.fields)}
            onGroupBy={(fieldId) => setConfig({ ...config, groupBy: fieldId })}
            query={recordsQuery}
            recordHref={recordHref}
          />
        ) : layout === 'gallery' ? (
          <RecordGallery {...shared} params={recordsParams({ ...recordsQuery, groupBy: null })} recordHref={recordHref} />
        ) : layout === 'calendar' ? (
          <RecordCalendar
            {...shared}
            dateFields={dateFields(body.fields)}
            onDateField={(fieldId) => setConfig({ ...config, dateField: fieldId })}
            query={recordsQuery}
            recordHref={recordHref}
          />
        ) : phone ? (
          <RecordCards {...shared} />
        ) : (
          <RecordTable {...shared} actions={actions} />
        )}
      </div>
      {filtering ? (
        <FieldFilterEditor
          anchor={filtering.anchor}
          clause={filtering.index === null ? null : (config.filters[filtering.index] ?? null)}
          field={filtering.field}
          fits={(clause) => clausesWithinLimits(withClause(config.filters, filtering.index, clause))}
          onApply={(clause) => setConfig({ ...config, filters: withClause(config.filters, filtering.index, clause) })}
          onClose={() => setFiltering(null)}
          projectId={projectId}
        />
      ) : null}
      {dialog?.kind === 'details' ? (
        <CollectionDetailsDialog collection={body.collection} onClose={() => setDialog(null)} onSaved={() => undefined} projectId={projectId} />
      ) : null}
      {dialog?.kind === 'forms' ? (
        <Suspense fallback={null}>
          <FormsDialog body={body} onClose={() => setDialog(null)} projectId={projectId} />
        </Suspense>
      ) : null}
      {dialog?.kind === 'field' ? (
        <FieldDialog
          field={dialog.field}
          linkable={(collections.data?.collections ?? []).filter((entry) => !entry.archived && !entry.builtin)}
          onClose={() => setDialog(null)}
          records="records"
          scope={fieldScope(projectId, collectionId)}
        />
      ) : null}
      {dialog?.kind === 'new-view' ? (
        <ViewNameDialog
          description="It starts with the filters, sort and fields showing now."
          initial={`View ${views.length + 1}`}
          layout="table"
          onClose={() => setDialog(null)}
          onSubmit={async (name, type) => {
            const created = await createView(projectId, collectionId, { name, type, config })
            keepView(created)
            if (view) dropDraft(view.id)
            void navigate(`?view=${encodeURIComponent(created.id)}`)
          }}
          submitLabel="Add view"
          title="New view"
        />
      ) : null}
      {dialog?.kind === 'rename-view' ? (
        <ViewNameDialog
          initial={dialog.view.name}
          onClose={() => setDialog(null)}
          onSubmit={async (name) => keepView(await updateView(projectId, collectionId, dialog.view, { name }))}
          submitLabel="Rename"
          title="Rename view"
        />
      ) : null}
      {dialog?.kind === 'delete-view' ? (
        <ConfirmDialog
          confirmLabel="Delete view"
          error={deleteProblem}
          onClose={() => {
            setDeleteProblem(null)
            setDialog(null)
          }}
          onConfirm={() => {
            const removed = dialog.view
            setDeleteProblem(null)
            deleteView(projectId, collectionId, removed.id).then(
              () => {
                client.setQueryData<CollectionBody>(collectionKeys.one(projectId, collectionId), (current) =>
                  current ? { ...current, views: current.views.filter((entry) => entry.id !== removed.id) } : current,
                )
                dropDraft(removed.id)
                setDialog(null)
                void navigate('?', { replace: true })
              },
              (error: unknown) => setDeleteProblem(describeError(error).message),
            )
          }}
          open
          title={`Delete ${dialog.view.name}?`}
        >
          The records stay. Only this way of showing them goes, for everyone.
        </ConfirmDialog>
      ) : null}
    </AppShell>
  )
}

/** The filters with one added, or the one at `index` changed. */
function withClause(filters: readonly FieldClause[], index: number | null, clause: FieldClause) {
  return index === null ? [...filters, clause] : filters.map((entry, at) => (at === index ? clause : entry))
}
