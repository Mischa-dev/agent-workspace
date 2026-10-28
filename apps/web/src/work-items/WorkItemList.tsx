import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from 'react'

import { useProjectExperience } from '../experience/layout'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, countLabel } from '../ui'
import { Table, type TableRow } from '../ui/pickers'
import { FieldPicker, type PickerField, type PickResult } from './FieldPicker'
import { pickerShown, useCardFields, type CardFields } from './cardFields'
import { AssigneeMark, DoneToggle, DueDate, GroupHeader, PickerCell, TaskBadges } from './ListCells'
import { LIST_COLUMNS } from './taskDisplay'
import { workItemHref } from './links'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'
import { useCoarsePointer } from './useCoarsePointer'
import { tucksDone, withoutDone } from './doneModel'
import { listQuery, type ListState } from './listModel'
import { buildListRows, DONE_GROUP, DONE_LOADING, visibleTaskIds, withDoneRows, type ListRow } from './listRows'
import type { Notice } from './notice'
import { useWorkItemGroups, useWorkItemPages } from './planningHooks'
import { SelectionBar } from './SelectionBar'
import { useDoneCount, useDoneItems } from './useDoneWork'
import { emptySelection, keepLoaded, MAX_BULK, pick, pickerCurrent, pickerTargets, selectAll, toggleOne, type Selection } from './selectionModel'
import { useTaskShortcuts } from './shortcutKeys'
import { editErrorMessage, useApplyPick, type EditTarget } from './taskEdits'
import type { WorkItemCard, WorkItemWorkflow } from './types'
import { useWindowedRows } from './useWindowedRows'

const PICKER_KEYS: Record<string, PickerField> = { s: 'status', a: 'assignee', l: 'labels', p: 'priority', m: 'milestone' }

function editTarget(card: WorkItemCard): EditTarget {
  return { id: card.id, projectId: card.projectId, version: card.version, key: card.key }
}

type PickerState = { field: PickerField; anchor: HTMLElement; targets: WorkItemCard[] }

function taskCells({
  card,
  disabled,
  fields,
  onOpen,
  onPick,
  onSelect,
  selected,
}: {
  card: WorkItemCard
  disabled: boolean
  fields: CardFields
  onOpen: (card: WorkItemCard, event: MouseEvent<HTMLAnchorElement>) => void
  onPick: (card: WorkItemCard, field: PickerField, anchor: HTMLElement) => void
  onSelect: (card: WorkItemCard, event: MouseEvent<HTMLInputElement>) => void
  selected: boolean
}): Record<string, ReactNode> {
  const editable = !disabled && card.capabilities.edit && !card.archivedAt
  const pick = (field: PickerField, anchor: HTMLElement) => onPick(card, field, anchor)
  return {
    // A read-only list (archived project, viewer or worker role) has nothing to select for.
    select: disabled ? null : (
      // The whole cell is the hit area; the box is the kit checkbox.
      <label className="aw-tasks__check">
        <input
          aria-label={`Select ${card.key}`}
          checked={selected}
          className="aw-check__input"
          onChange={() => undefined}
          onClick={(event) => onSelect(card, event)}
          tabIndex={-1}
          type="checkbox"
        />
        <span aria-hidden="true" className="aw-check__box">
          <Icon name="check" size={12} />
        </span>
      </label>
    ),
    priority: editable ? (
      <PickerCell field="priority" label={`Priority of ${card.key}, change`} onPick={pick}>
        <PriorityIcon priority={card.priority} />
      </PickerCell>
    ) : (
      <PriorityIcon priority={card.priority} />
    ),
    key: <span className="aw-tasks__key">{card.key}</span>,
    status: editable ? (
      <PickerCell field="status" label={`Status of ${card.key}: ${card.status.name}, change`} onPick={pick}>
        <StatusIcon category={card.status.semanticCategory} label={card.status.name} />
      </PickerCell>
    ) : (
      <StatusIcon category={card.status.semanticCategory} label={card.status.name} />
    ),
    title: (
      <>
        <a className="aw-tasks__title" href={workItemHref(card.projectId, card.id)} onClick={(event) => onOpen(card, event)} tabIndex={-1}>
          {fields.keys ? <span className="aw-tasks__phonekey">{card.key} </span> : null}
          {card.title}
        </a>
        {card.archivedAt ? <span className="aw-worklist__tag">Archived</span> : null}
      </>
    ),
    details: <TaskBadges card={card} fields={fields} />,
    due: <DueDate card={card} />,
    assignee: editable ? (
      <PickerCell field="assignee" label={card.assignee ? `Assigned to ${card.assignee.displayName}, change` : `Assign ${card.key}`} onPick={pick}>
        <AssigneeMark card={card} />
      </PickerCell>
    ) : (
      <AssigneeMark card={card} />
    ),
  }
}

/**
 * The list view: dense rows, grouped or not, drawn a screenful at a time and
 * loaded page by page as you scroll. Every row's status, priority, assignee,
 * labels and milestone change from a picker, by click or by letter key.
 */
export function WorkItemList({
  disabled,
  emptyAction,
  hasFilters,
  notify,
  onClearFilters,
  onClosePanel,
  onManageLabels,
  onOpenTask,
  onToggleDone,
  openTaskId,
  projectId,
  state,
  workflow,
}: {
  disabled: boolean
  emptyAction: ReactNode
  hasFilters: boolean
  notify: (notice: Notice) => void
  onClearFilters: () => void
  onClosePanel: () => void
  onManageLabels: () => void
  onOpenTask: (id: string) => void
  onToggleDone: () => void
  openTaskId: string | null
  projectId: string
  state: ListState
  workflow: WorkItemWorkflow
}) {
  // Done and canceled tasks sit in their own section at the bottom, behind the Show done toggle.
  const tucked = tucksDone(state.filters)
  const query = useMemo(() => (tucked ? withoutDone(listQuery(state)) : listQuery(state)), [state, tucked])
  const queryKey = JSON.stringify(query)
  const pages = useWorkItemPages(projectId, query)
  const groups = useWorkItemGroups(projectId, query, Boolean(state.groupBy))
  const fields = useCardFields(projectId)
  const experience = useProjectExperience(projectId)
  const doneCount = useDoneCount(projectId, state.filters, workflow, tucked)
  const done = useDoneItems(projectId, query, tucked && state.showDone)
  const [collapsedState, setCollapsedState] = useState<{ groupBy: string | null; keys: Set<string> }>({ groupBy: null, keys: new Set() })
  const collapsed = useMemo(
    () => (collapsedState.groupBy === state.groupBy ? collapsedState.keys : new Set<string>()),
    [collapsedState, state.groupBy],
  )
  const [selectionState, setSelection] = useState<Selection>(emptySelection)
  const [activeKey, setActiveKey] = useState<string | null>(null)
  const [picker, setPicker] = useState<PickerState | null>(null)
  const focusWanted = useRef(false)
  const bodyRef = useRef<HTMLDivElement>(null)
  const applyPick = useApplyPick()
  const coarse = useCoarsePointer()

  const openItems = useMemo(() => pages.data?.pages.flatMap((page) => page.items) ?? [], [pages.data])
  const items = useMemo(() => (done.items.length ? [...openItems, ...done.items] : openItems), [done.items, openItems])
  const total = pages.data?.pages[0]?.total ?? 0
  const rows = useMemo(
    () =>
      withDoneRows(
        buildListRows({
          items: openItems,
          grouped: Boolean(state.groupBy),
          groups: groups.data,
          collapsed,
          hasMore: Boolean(pages.hasNextPage),
        }),
        { count: tucked ? doneCount : 0, open: state.showDone, items: done.items, hasMore: done.hasMore },
      ),
    [collapsed, done.hasMore, done.items, doneCount, groups.data, openItems, pages.hasNextPage, state.groupBy, state.showDone, tucked],
  )
  const cardsById = useMemo(() => new Map(items.map((item) => [item.id, item])), [items])
  const order = useMemo(() => visibleTaskIds(rows), [rows])
  const selection = useMemo(() => keepLoaded(selectionState, new Set(cardsById.keys())), [cardsById, selectionState])
  const rowHeight = coarse ? 44 : 36
  const heights = useMemo(() => rows.map(() => rowHeight), [rowHeight, rows])
  const windowed = useWindowedRows(bodyRef, heights)
  const statusCategories = useMemo(() => new Map(workflow.statuses.map((status) => [status.id, status.semanticCategory])), [workflow.statuses])

  // Filters changed: the selection and focus belong to the old list.
  const [shownQuery, setShownQuery] = useState(queryKey)
  if (shownQuery !== queryKey) {
    setShownQuery(queryKey)
    setSelection(emptySelection)
    setActiveKey(null)
  }

  // Load the next page when its placeholder row scrolls into view.
  const shownLoading = rows.slice(windowed.start, windowed.end).filter((row) => row.kind === 'loading').map((row) => row.key)
  const loadingVisible = shownLoading.some((key) => key !== DONE_LOADING)
  const doneLoadingVisible = shownLoading.includes(DONE_LOADING)
  const { fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } = pages
  useEffect(() => {
    if (loadingVisible && hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage()
  }, [fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage, loadingVisible])
  const fetchMoreDone = done.fetchMore
  useEffect(() => {
    if (doneLoadingVisible) fetchMoreDone()
  }, [doneLoadingVisible, fetchMoreDone])

  const focusable = useMemo(() => rows.filter((row) => row.kind !== 'loading'), [rows])
  const currentKey = activeKey && rows.some((row) => row.key === activeKey) ? activeKey : null

  useEffect(() => {
    if (!focusWanted.current || !currentKey) return
    focusWanted.current = false
    bodyRef.current?.querySelector<HTMLElement>(`[data-row-key="${CSS.escape(currentKey)}"]`)?.focus({ preventScroll: true })
  })

  const activate = useCallback(
    (key: string, focus: boolean) => {
      setActiveKey(key)
      const index = rows.findIndex((row) => row.key === key)
      if (index >= 0) windowed.scrollToIndex(index)
      focusWanted.current = focus
    },
    [rows, windowed],
  )

  const toggleGroup = (groupKey: string) => {
    if (groupKey === DONE_GROUP) return onToggleDone()
    const keys = new Set(collapsed)
    if (keys.has(groupKey)) keys.delete(groupKey)
    else keys.add(groupKey)
    setCollapsedState({ groupBy: state.groupBy, keys })
  }

  const rowByKey = (key: string) => rows.find((row) => row.key === key)
  const selectedCards = () => [...selection.ids].flatMap((id) => cardsById.get(id) ?? [])

  const openPicker = (field: PickerField, anchor: HTMLElement, card?: WorkItemCard) => {
    if (disabled) return
    const targets = pickerTargets(selectedCards(), card)
    if (!targets.length) return
    if (targets.length > MAX_BULK) {
      notify({ tone: 'info', title: `Bulk changes take up to ${countLabel(MAX_BULK, 'task')} at a time`, description: 'Select fewer and try again.' })
      return
    }
    setPicker({ field, anchor, targets })
  }

  const onPickResult = (result: PickResult) => {
    if (!picker) return
    const targets = picker.targets
    applyPick.mutate(
      { projectId, result, targets: targets.map(editTarget) },
      {
        onSuccess: () => {
          if (targets.length > 1) notify({ tone: 'success', title: `Changed ${countLabel(targets.length, 'task')}` })
        },
        onError: (error) =>
          notify({
            tone: 'danger',
            title: targets.length === 1 ? `${targets[0].key} didn’t change` : `${countLabel(targets.length, 'task')} didn’t change`,
            description: editErrorMessage(error),
          }),
      },
    )
  }

  const onRowSelect = (card: WorkItemCard, event: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => {
    if (disabled) return
    setSelection(pick(selection, card.id, order, { shift: event.shiftKey, toggle: event.metaKey || event.ctrlKey }))
  }

  useTaskShortcuts(!picker, (event) => {
    const key = event.key.toLowerCase()
    if (!disabled && (event.metaKey || event.ctrlKey) && key === 'a' && bodyRef.current?.contains(document.activeElement)) {
      setSelection(selectAll(order))
      if (order.length > MAX_BULK) notify({ tone: 'info', title: `Selected the first ${countLabel(MAX_BULK, 'task')}`, description: `Bulk changes take up to ${MAX_BULK} at a time.` })
      return true
    }
    if (event.metaKey || event.ctrlKey) return false
    if (key === 'escape') {
      if (openTaskId) onClosePanel()
      else if (selection.ids.size) setSelection(emptySelection)
      else return false
      return true
    }
    if (key === 'j' || key === 'k') {
      if (!focusable.length) return false
      const index = focusable.findIndex((row) => row.key === currentKey)
      const next = index < 0 ? 0 : Math.max(0, Math.min(focusable.length - 1, index + (key === 'j' ? 1 : -1)))
      activate(focusable[next].key, true)
      // With a task open beside the list, the panel follows, the way a peek does.
      const target = focusable[next]
      if (openTaskId && target.kind === 'task' && target.item.id !== openTaskId) onOpenTask(target.item.id)
      return true
    }
    const row = currentKey ? rowByKey(currentKey) : undefined
    if (key === 'x' && row?.kind === 'task' && !disabled) {
      setSelection(toggleOne(selection, row.item.id))
      return true
    }
    if (key === 'enter' && row) {
      if (row.kind === 'task') onOpenTask(row.item.id)
      else if (row.kind === 'group') toggleGroup(row.groupKey)
      return true
    }
    const field = PICKER_KEYS[key]
    if (field && !event.shiftKey && pickerShown(field, fields)) {
      const element = currentKey ? bodyRef.current?.querySelector<HTMLElement>(`[data-row-key="${CSS.escape(currentKey)}"]`) : null
      const card = row?.kind === 'task' ? row.item : undefined
      if (!card && !selection.ids.size) return false
      openPicker(field, element ?? bodyRef.current ?? document.body, card)
      return true
    }
    return false
  })

  if (pages.isPending) return <ListSkeleton label={`Loading ${experience.word('task', 'many')}`} rows={6} />
  if (pages.isError && !pages.data) return <ErrorState error={pages.error} onRetry={() => void pages.refetch()} title={`${experience.label('task', 'many')} didn’t load`} />
  if (items.length === 0 && !pages.hasNextPage && !(tucked && doneCount > 0)) {
    return (
      <EmptyState
        action={hasFilters ? <Button onClick={onClearFilters}>Clear filters</Button> : emptyAction}
        icon="work"
        title={hasFilters ? `No matching ${experience.word('task', 'many')}` : `No ${experience.word('task', 'many')} yet`}
      >
        {hasFilters ? 'Try a different search, or clear some filters.' : 'Write down what needs doing, then hand it to a person or an agent.'}
      </EmptyState>
    )
  }

  const tableRows: TableRow[] = rows.slice(windowed.start, windowed.end).map((row, offset) => toTableRow(row, windowed.start + offset))

  function toTableRow(row: ListRow, index: number): TableRow {
    if (row.kind === 'loading') {
      const doneRow = row.key === DONE_LOADING
      return {
        key: row.key,
        index,
        inert: true,
        height: rowHeight,
        full: (doneRow ? done.failed : isFetchNextPageError) ? (
          <span className="aw-tasks__loading">
            More tasks didn’t load.
            <Button onClick={doneRow ? done.retry : () => void fetchNextPage()} size="sm">
              Try again
            </Button>
          </span>
        ) : (
          <span className="aw-tasks__loading" role="status">
            <Icon className="aw-spin" name="loader" size={14} /> Loading more…
          </span>
        ),
      }
    }
    if (row.kind === 'group' && row.groupKey === DONE_GROUP) {
      return {
        key: row.key,
        index,
        height: rowHeight,
        className: 'aw-tasks__grouprow',
        label: `${row.collapsed ? 'Show' : 'Hide'} done and canceled, ${countLabel(row.count ?? row.loaded, 'task')}`,
        full: <DoneToggle count={row.count ?? row.loaded} onToggle={() => toggleGroup(row.groupKey)} open={!row.collapsed} />,
      }
    }
    if (row.kind === 'group') {
      return {
        key: row.key,
        index,
        height: rowHeight,
        className: 'aw-tasks__grouprow',
        label: `${row.label || 'None'}, ${countLabel(row.count ?? row.loaded, 'task')}, ${row.collapsed ? 'collapsed' : 'expanded'}`,
        full: (
          <GroupHeader
            groupBy={state.groupBy}
            onToggle={() => toggleGroup(row.groupKey)}
            row={row}
            statusCategory={state.groupBy === 'status' ? statusCategories.get(row.groupKey) : undefined}
          />
        ),
      }
    }
    const card = row.item
    const selected = selection.ids.has(card.id)
    return {
      key: row.key,
      index,
      height: rowHeight,
      selected,
      className: card.id === openTaskId ? 'is-open' : undefined,
      label: `${card.key} ${card.title}`,
      cells: taskCells({
        card,
        disabled,
        fields,
        selected,
        onOpen: (target, event: MouseEvent<HTMLAnchorElement>) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return
          event.preventDefault()
          setActiveKey(row.key)
          onOpenTask(target.id)
        },
        onPick: (target, field, anchor) => {
          setActiveKey(row.key)
          openPicker(field, anchor, target)
        },
        onSelect: (target, event) => {
          setActiveKey(row.key)
          onRowSelect(target, { shiftKey: event.shiftKey, metaKey: true, ctrlKey: false })
        },
      }),
    }
  }

  return (
    <div className="aw-tasks">
      <p aria-live="polite" className="aw-tasks__summary">
        {countLabel(total, experience.word('task'), experience.word('task', 'many'))}
        {pages.isFetching && !pages.isFetchingNextPage ? <Icon className="aw-spin" name="loader" size={12} /> : null}
      </p>
      <Table
        activeKey={currentKey}
        ariaMultiselectable
        bodyRef={bodyRef}
        className="aw-tasks__table"
        columns={LIST_COLUMNS.filter(
          (column) =>
            (column.id === 'select' || column.id === 'title' || !state.fields || state.fields.includes(column.id)) &&
            // The layout's hidden fields stay out whatever the view asks for.
            !(column.id === 'priority' && !fields.priority) &&
            !(column.id === 'key' && !fields.keys) &&
            !(column.id === 'due' && !fields.dates),
        )}
        label="Tasks"
        onActivate={(key) => {
          const row = rowByKey(key)
          if (row?.kind === 'task') onOpenTask(row.item.id)
          else if (row?.kind === 'group') toggleGroup(row.groupKey)
        }}
        onActiveKeyChange={(key) => activate(key, false)}
        onRowClick={(event, key) => {
          const row = rowByKey(key)
          setActiveKey(key)
          if (row?.kind === 'group') toggleGroup(row.groupKey)
          else if (row?.kind === 'task') {
            if (event.shiftKey || event.metaKey || event.ctrlKey) onRowSelect(row.item, event)
            else onOpenTask(row.item.id)
          }
        }}
        rowCount={rows.length}
        rows={tableRows}
        spaceAfter={windowed.after}
        spaceBefore={windowed.before}
      />
      {disabled ? null : (
        <SelectionBar
          busy={applyPick.isPending}
          notify={notify}
          onClear={() => setSelection(emptySelection)}
          onPick={(field, anchor) => openPicker(field, anchor)}
          projectId={projectId}
          selected={selectedCards()}
        />
      )}
      {picker ? (
        <FieldPicker
          anchor={picker.anchor}
          current={pickerCurrent(picker.targets)}
          field={picker.field}
          onClose={() => setPicker(null)}
          onManageLabels={onManageLabels}
          onPick={onPickResult}
          projectId={projectId}
          workflow={workflow}
        />
      ) : null}
    </div>
  )
}
