import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from 'react'

import { Button, EmptyState, ErrorState, Icon, MessageSkeleton, Table, type TableRow } from '../ui'
import { BulkBar } from './BulkBar'
import { FieldPicker, type PickerField, type PickResult } from './FieldPicker'
import { AssigneeMark, DueDate, GroupHeader, PickerCell, TaskBadges } from './ListCells'
import { LIST_COLUMNS } from './taskDisplay'
import { workItemHref } from './links'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'
import { useCoarsePointer } from './useCoarsePointer'
import { listQuery, type ListState } from './listModel'
import { buildListRows, visibleTaskIds, type ListRow } from './listRows'
import type { Notice } from './notice'
import { bulkUpdateWorkItems } from './planningApi'
import { useInvalidateWorkItems, useWorkItemGroups, useWorkItemPages } from './planningHooks'
import { emptySelection, keepLoaded, MAX_BULK, pick, selectAll, toggleOne, type Selection } from './selectionModel'
import { useTaskShortcuts } from './shortcutKeys'
import { editErrorMessage, useApplyPick, type EditTarget } from './taskEdits'
import type { BulkChanges, WorkItemCard, WorkItemWorkflow } from './types'
import { useWindowedRows } from './useWindowedRows'

const PICKER_KEYS: Record<string, PickerField> = { s: 'status', a: 'assignee', l: 'labels', p: 'priority', m: 'milestone' }

function editTarget(card: WorkItemCard): EditTarget {
  return { id: card.id, projectId: card.projectId, version: card.version, key: card.key }
}

type PickerState = { field: PickerField; anchor: HTMLElement; targets: WorkItemCard[] }

function taskCells({
  card,
  disabled,
  onOpen,
  onPick,
  onSelect,
  selected,
}: {
  card: WorkItemCard
  disabled: boolean
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
      <input
        aria-label={`Select ${card.key}`}
        checked={selected}
        className="aw-tasks__check"
        onChange={() => undefined}
        onClick={(event) => onSelect(card, event)}
        tabIndex={-1}
        type="checkbox"
      />
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
          <span className="aw-tasks__phonekey">{card.key} </span>
          {card.title}
        </a>
        {card.archivedAt ? <span className="aw-worklist__tag">Archived</span> : null}
      </>
    ),
    details: <TaskBadges card={card} />,
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
  openTaskId,
  projectId,
  state,
  workflow,
}: {
  disabled: boolean
  emptyAction: ReactNode
  hasFilters: boolean
  notify: (notice: Notice | null) => void
  onClearFilters: () => void
  onClosePanel: () => void
  onManageLabels: () => void
  onOpenTask: (id: string) => void
  openTaskId: string | null
  projectId: string
  state: ListState
  workflow: WorkItemWorkflow
}) {
  const query = useMemo(() => listQuery(state), [state])
  const queryKey = JSON.stringify(query)
  const pages = useWorkItemPages(projectId, query)
  const groups = useWorkItemGroups(projectId, query, Boolean(state.groupBy))
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
  const invalidate = useInvalidateWorkItems()
  const [bulkBusy, setBulkBusy] = useState(false)
  const coarse = useCoarsePointer()

  const items = useMemo(() => pages.data?.pages.flatMap((page) => page.items) ?? [], [pages.data])
  const total = pages.data?.pages[0]?.total ?? 0
  const rows = useMemo(
    () =>
      buildListRows({
        items,
        grouped: Boolean(state.groupBy),
        groups: groups.data,
        collapsed,
        hasMore: Boolean(pages.hasNextPage),
      }),
    [collapsed, groups.data, items, pages.hasNextPage, state.groupBy],
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
  const loadingIndex = rows.findIndex((row) => row.kind === 'loading')
  const loadingVisible = loadingIndex >= windowed.start && loadingIndex < windowed.end && loadingIndex >= 0
  const { fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } = pages
  useEffect(() => {
    if (loadingVisible && hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage()
  }, [fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage, loadingVisible])

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
    const keys = new Set(collapsed)
    if (keys.has(groupKey)) keys.delete(groupKey)
    else keys.add(groupKey)
    setCollapsedState({ groupBy: state.groupBy, keys })
  }

  const rowByKey = (key: string) => rows.find((row) => row.key === key)
  const selectedCards = () => [...selection.ids].flatMap((id) => cardsById.get(id) ?? [])

  const openPicker = (field: PickerField, anchor: HTMLElement, card?: WorkItemCard) => {
    if (disabled) return
    const selected = selectedCards()
    const targets = card && !selection.ids.has(card.id) ? [card] : selected.length ? selected : card ? [card] : []
    const editable = targets.filter((target) => target.capabilities.edit && !target.archivedAt)
    if (!editable.length) return
    if (editable.length > MAX_BULK) {
      notify({ tone: 'warning', message: `Bulk changes take up to ${MAX_BULK} work items at a time.` })
      return
    }
    setPicker({ field, anchor, targets: editable })
  }

  const runBulk = async (changes: BulkChanges, cards: WorkItemCard[]) => {
    setBulkBusy(true)
    try {
      const result = await bulkUpdateWorkItems({
        projectId,
        items: cards.map((card) => ({ id: card.id, expectedVersion: card.version })),
        changes,
        idempotencyKey: crypto.randomUUID(),
      })
      return result
    } catch (error) {
      notify({ tone: 'danger', message: editErrorMessage(error, 'That change') })
      return null
    } finally {
      setBulkBusy(false)
      void invalidate(projectId)
    }
  }

  const onPickResult = (result: PickResult) => {
    if (!picker) return
    const targets = picker.targets
    applyPick.mutate(
      { projectId, result, targets: targets.map(editTarget) },
      {
        onSuccess: () => {
          if (targets.length > 1) notify({ tone: 'success', message: `Changed ${targets.length} work items.` })
        },
        onError: (error) => notify({ tone: 'danger', message: editErrorMessage(error) }),
      },
    )
  }

  const archiveSelected = async () => {
    const cards = selectedCards()
    if (!cards.length) return
    const result = await runBulk({ archived: true }, cards)
    if (!result) return
    setSelection(emptySelection)
    notify({
      tone: 'success',
      message: `Archived ${cards.length} work item${cards.length === 1 ? '' : 's'}.`,
      action: {
        label: 'Undo',
        run: () => void runBulk({ archived: false }, result.items),
      },
    })
  }

  const restoreSelected = async () => {
    const cards = selectedCards()
    if (!cards.length) return
    const result = await runBulk({ archived: false }, cards)
    if (!result) return
    setSelection(emptySelection)
    notify({ tone: 'success', message: `Restored ${cards.length} work item${cards.length === 1 ? '' : 's'}.` })
  }

  const setDueDate = async (value: string | null) => {
    const cards = selectedCards()
    if (!cards.length) return
    const result = await runBulk({ dueDate: value }, cards)
    if (result) notify({ tone: 'success', message: `Changed ${cards.length} work items.` })
  }

  const onRowSelect = (card: WorkItemCard, event: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => {
    if (disabled) return
    setSelection(pick(selection, card.id, order, { shift: event.shiftKey, toggle: event.metaKey || event.ctrlKey }))
  }

  useTaskShortcuts(!picker, (event) => {
    const key = event.key.toLowerCase()
    if (!disabled && (event.metaKey || event.ctrlKey) && key === 'a' && bodyRef.current?.contains(document.activeElement)) {
      setSelection(selectAll(order))
      if (order.length > MAX_BULK) notify({ tone: 'info', message: `Selected the first ${MAX_BULK}; bulk changes take up to ${MAX_BULK} at a time.` })
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
    if (field && !event.shiftKey) {
      const element = currentKey ? bodyRef.current?.querySelector<HTMLElement>(`[data-row-key="${CSS.escape(currentKey)}"]`) : null
      const card = row?.kind === 'task' ? row.item : undefined
      if (!card && !selection.ids.size) return false
      openPicker(field, element ?? bodyRef.current ?? document.body, card)
      return true
    }
    return false
  })

  if (pages.isPending) return <MessageSkeleton label="Loading work items" rows={4} />
  if (pages.isError && !pages.data) {
    return (
      <ErrorState action={<Button onClick={() => void pages.refetch()}>Try again</Button>} title="Work items didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }
  if (items.length === 0 && !pages.hasNextPage) {
    return (
      <EmptyState
        action={hasFilters ? <Button onClick={onClearFilters}>Clear filters</Button> : emptyAction}
        icon="work"
        title={hasFilters ? 'No matching work items' : 'No work items yet'}
      >
        {hasFilters ? 'Try a different search, or clear some filters.' : 'Write down what needs doing, then hand it to a person or an agent.'}
      </EmptyState>
    )
  }

  const tableRows: TableRow[] = rows.slice(windowed.start, windowed.end).map((row, offset) => toTableRow(row, windowed.start + offset))

  function toTableRow(row: ListRow, index: number): TableRow {
    if (row.kind === 'loading') {
      return {
        key: row.key,
        index,
        inert: true,
        height: rowHeight,
        full: isFetchNextPageError ? (
          <span className="aw-tasks__loading">
            More work items didn’t load.
            <Button onClick={() => void fetchNextPage()} size="sm">
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
    if (row.kind === 'group') {
      return {
        key: row.key,
        index,
        height: rowHeight,
        className: 'aw-tasks__grouprow',
        label: `${row.label || 'None'}, ${row.count ?? row.loaded} work items, ${row.collapsed ? 'collapsed' : 'expanded'}`,
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

  const pickerTargets = picker?.targets ?? []
  const first = pickerTargets[0]
  const sharedLabels = first ? first.labels.filter((label) => pickerTargets.every((target) => target.labels.some((entry) => entry.id === label.id))).map((label) => label.id) : []

  return (
    <div className="aw-tasks">
      <p aria-live="polite" className="aw-tasks__summary">
        {total === 1 ? '1 work item' : `${total.toLocaleString()} work items`}
        {pages.isFetching && !pages.isFetchingNextPage ? <Icon className="aw-spin" name="loader" size={12} /> : null}
      </p>
      <Table
        activeKey={currentKey}
        ariaMultiselectable
        bodyRef={bodyRef}
        className="aw-tasks__table"
        columns={LIST_COLUMNS}
        label="Work items"
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
        <BulkBar
          archived={selection.ids.size > 0 && selectedCards().every((card) => Boolean(card.archivedAt))}
          busy={bulkBusy || applyPick.isPending}
          count={selection.ids.size}
          onArchive={() => void archiveSelected()}
          onClear={() => setSelection(emptySelection)}
          onDueDate={(value) => void setDueDate(value)}
          onPick={(field, anchor) => openPicker(field, anchor)}
          onRestore={() => void restoreSelected()}
        />
      )}
      {picker ? (
        <FieldPicker
          anchor={picker.anchor}
          current={
            pickerTargets.length === 1 && first
              ? {
                  statusId: first.status.id,
                  priority: first.priority,
                  assigneeId: first.assignee?.actorId ?? null,
                  assignee: first.assignee
                    ? { id: first.assignee.actorId, displayName: first.assignee.displayName, handle: first.assignee.handle, kind: first.assignee.kind }
                    : null,
                  labelIds: sharedLabels,
                  milestoneId: first.milestone?.id ?? null,
                  typeId: first.itemType.id,
                }
              : { labelIds: sharedLabels }
          }
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
