import { useState } from 'react'

import { Avatar, Button, Icon, SegmentedControl, SelectField, TextField, actorKind } from '../ui'
import { Calendar, ComboboxList, Popover } from '../ui/pickers'
import { workItemPriorities } from './constants'
import { CATEGORY_LABELS, FILTER_FIELDS, TOGGLE_FIELDS, chipText, type FilterField } from './filterModel'
import { useAssignableActors, useWorkItem } from './hooks'
import {
  COLUMN_FIELDS,
  GROUPINGS,
  LANE_FIELDS,
  SORTS,
  clearFilters,
  type FilterValues,
  type ListState,
} from './listModel'
import { useLabels, useMilestones } from './planningHooks'
import { useDebouncedValue } from './pickerModel'
import { ShortcutHelp } from './ShortcutHelp'
import { BOARD_SHORTCUTS, LIST_SHORTCUTS } from './shortcutKeys'
import { TaskGlyph } from './TaskGlyph'
import { TaskSearchPicker } from './TaskSearchPicker'
import type { BoardColumnsBy, WorkItemGroupBy, WorkItemSort, WorkItemWorkflow } from './types'
import { useDebouncedSearch, type UpdateListState } from './useListState'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'

type Open = { field: FilterField | null; anchor: HTMLElement }

/** Clears every key of a filter field, then sets `values`. */
function setField(state: ListState, field: FilterField, values: Partial<FilterValues>): ListState {
  const filters: Record<string, unknown> = { ...state.filters }
  for (const key of FILTER_FIELDS.find((entry) => entry.field === field)?.keys ?? []) delete filters[key]
  for (const [key, value] of Object.entries(values)) {
    if (value !== undefined && value !== '' && value !== false) filters[key] = value
  }
  return { ...state, filters: filters as FilterValues }
}

/**
 * Search, filters as chips, sort and grouping (or the board's columns and
 * lanes), and the view switch. Everything here lives in the URL.
 */
export function FilterBar({
  projectId,
  state,
  update,
  workflow,
}: {
  projectId: string
  state: ListState
  update: UpdateListState
  workflow: WorkItemWorkflow | undefined
}) {
  const [search, setSearch] = useDebouncedSearch(state.filters.q ?? '', update)
  const [open, setOpen] = useState<Open | null>(null)
  const labels = useLabels(projectId)
  const milestones = useMilestones(projectId)
  const assignees = useAssignableActors(projectId, '')
  const parent = useWorkItem(projectId, state.filters.parentId && state.filters.parentId !== 'none' ? state.filters.parentId : '')
  const assigneeName = state.filters.assigneeActorId
    ? (assignees.data?.find((actor) => actor.id === state.filters.assigneeActorId)?.displayName ?? null)
    : null
  const lookups = { workflow, labels: labels.data, milestones: milestones.data, assigneeName, parentKey: parent.data?.key ?? null }
  const chips = FILTER_FIELDS.flatMap((entry) => {
    const text = chipText(entry.field, state.filters, lookups)
    return text ? [{ field: entry.field, text }] : []
  })
  const apply = (field: FilterField, values: Partial<FilterValues>) => update((current) => setField(current, field, values))

  return (
    <div className="aw-filterbar">
      <div aria-label="Filter work items" className="aw-filterbar__row" role="search">
        <TextField
          aria-label="Search work items"
          className="aw-filterbar__search"
          maxLength={120}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="Search by title or key"
          prefix={<Icon name="search" size={14} />}
          type="search"
          value={search}
        />
        <Button aria-haspopup="dialog" onClick={(event) => setOpen({ field: null, anchor: event.currentTarget })} size="sm" variant="secondary">
          <TaskGlyph name="filter" />
          Filter
        </Button>
        <span className="aw-filterbar__spacer" />
        {state.view === 'list' ? (
          <>
            <SelectField
              aria-label="Sort"
              className="aw-filterbar__select"
              onChange={(event) => update((current) => ({ ...current, sort: event.target.value as WorkItemSort }))}
              prefix={<TaskGlyph name="sort" />}
              value={state.sort}
            >
              {SORTS.map((sort) => (
                <option key={sort.value} value={sort.value}>
                  {sort.label}
                </option>
              ))}
            </SelectField>
            <SelectField
              aria-label="Group by"
              className="aw-filterbar__select"
              onChange={(event) => update((current) => ({ ...current, groupBy: (event.target.value || null) as WorkItemGroupBy | null }))}
              prefix={<TaskGlyph name="group" />}
              value={state.groupBy ?? ''}
            >
              <option value="">No grouping</option>
              {GROUPINGS.map((grouping) => (
                <option key={grouping.value} value={grouping.value}>
                  By {grouping.label.toLocaleLowerCase()}
                </option>
              ))}
            </SelectField>
          </>
        ) : (
          <>
            <SelectField
              aria-label="Columns"
              className="aw-filterbar__select"
              onChange={(event) => {
                const columnsBy = event.target.value as BoardColumnsBy
                // Lanes by the same field as the columns would leave only the diagonal.
                update((current) => ({ ...current, columnsBy, lanes: current.lanes === columnsBy ? 'none' : current.lanes }))
              }}
              prefix={<TaskGlyph name="group" />}
              value={state.columnsBy}
            >
              {COLUMN_FIELDS.map((field) => (
                <option key={field.value} value={field.value}>
                  Columns by {field.label.toLocaleLowerCase()}
                </option>
              ))}
            </SelectField>
            <SelectField
              aria-label="Swimlanes"
              className="aw-filterbar__select"
              onChange={(event) => update((current) => ({ ...current, lanes: event.target.value as ListState['lanes'] }))}
              prefix={<TaskGlyph name="lanes" />}
              value={state.lanes}
            >
              {LANE_FIELDS.filter((field) => field.value !== state.columnsBy).map((field) => (
                <option key={field.value} value={field.value}>
                  {field.value === 'none' ? field.label : `Lanes by ${field.label.toLocaleLowerCase()}`}
                </option>
              ))}
            </SelectField>
          </>
        )}
        <SegmentedControl
          label="View"
          onChange={(next) => update((current) => ({ ...current, view: next }))}
          options={[
            { value: 'list', label: 'List', icon: 'list' },
            { value: 'board', label: 'Board', icon: 'board' },
          ]}
          value={state.view}
        />
        <ShortcutHelp shortcuts={state.view === 'list' ? LIST_SHORTCUTS : BOARD_SHORTCUTS} />
      </div>
      {chips.length ? (
        <ul aria-label="Active filters" className="aw-filterbar__chips">
          {chips.map((chip) => (
            <li className="aw-chipfilter" key={chip.field}>
              <button
                aria-haspopup={TOGGLE_FIELDS[chip.field] ? undefined : 'dialog'}
                className="aw-chipfilter__text"
                disabled={Boolean(TOGGLE_FIELDS[chip.field])}
                onClick={(event) => setOpen({ field: chip.field, anchor: event.currentTarget })}
                type="button"
              >
                {chip.text}
              </button>
              <button aria-label={`Remove filter: ${chip.text}`} className="aw-chipfilter__remove" onClick={() => apply(chip.field, {})} type="button">
                <Icon name="x" size={12} />
              </button>
            </li>
          ))}
          <li>
            <Button onClick={() => update((current) => clearFilters(current))} size="sm" variant="ghost">
              Clear filters
            </Button>
          </li>
        </ul>
      ) : null}
      {open && open.field !== 'parent' ? (
        <Popover anchor={open.anchor} label={open.field ? `Filter by ${FILTER_FIELDS.find((entry) => entry.field === open.field)?.label.toLocaleLowerCase()}` : 'Add a filter'} onClose={() => setOpen(null)} open width={300}>
          {open.field === null ? (
            <ComboboxList
              label="Add a filter"
              onSelect={(value) => {
                const field = value as FilterField
                const toggle = TOGGLE_FIELDS[field]
                if (toggle) {
                  apply(field, toggle)
                  setOpen(null)
                } else setOpen({ field, anchor: open.anchor })
              }}
              options={FILTER_FIELDS.map((entry) => ({ value: entry.field, label: entry.label }))}
              placeholder="Filter by…"
              selected={chips.map((chip) => chip.field)}
            />
          ) : (
            <FilterValueChooser
              field={open.field}
              filters={state.filters}
              labels={labels.data ?? []}
              milestones={milestones.data ?? []}
              onApply={(values, close = true) => {
                apply(open.field as FilterField, values)
                if (close) setOpen(null)
              }}
              projectId={projectId}
              workflow={workflow}
            />
          )}
        </Popover>
      ) : null}
      <TaskSearchPicker
        allowTypedKey={false}
        anchor={open?.field === 'parent' ? open.anchor : null}
        exclude={[]}
        label="Filter by parent"
        noneLabel="Top-level only"
        onClose={() => setOpen(null)}
        onPick={(target) => {
          apply('parent', { parentId: target.id ?? 'none' })
          setOpen(null)
        }}
        projectId={projectId}
      />
    </div>
  )
}

function FilterValueChooser({
  field,
  filters,
  labels,
  milestones,
  onApply,
  projectId,
  workflow,
}: {
  field: FilterField
  filters: FilterValues
  labels: Array<{ id: string; name: string; color: string }>
  milestones: Array<{ id: string; name: string; targetDate: string | null }>
  onApply: (values: Partial<FilterValues>, close?: boolean) => void
  projectId: string
  workflow: WorkItemWorkflow | undefined
}) {
  const one = (label: string, options: Parameters<typeof ComboboxList>[0]['options'], selected: string | undefined, toValues: (value: string) => Partial<FilterValues>) => (
    <ComboboxList label={label} onSelect={(value) => onApply(toValues(value))} options={options} selected={selected ? [selected] : []} />
  )
  switch (field) {
    case 'status':
      return one(
        'Status',
        (workflow?.statuses ?? []).map((status) => ({ value: status.id, label: status.name, leading: <StatusIcon category={status.semanticCategory} /> })),
        filters.statusId,
        (value) => ({ statusId: value }),
      )
    case 'category':
      return one(
        'Status group',
        Object.entries(CATEGORY_LABELS).map(([value, label]) => ({ value, label, leading: <StatusIcon category={value} /> })),
        filters.semanticCategory,
        (value) => ({ semanticCategory: value }),
      )
    case 'priority':
      return one(
        'Priority',
        workItemPriorities.map((priority) => ({ value: priority.value, label: priority.label, leading: <PriorityIcon decorative priority={priority.value} /> })),
        filters.priority,
        (value) => ({ priority: value as FilterValues['priority'] }),
      )
    case 'type':
      return one('Type', (workflow?.types ?? []).map((type) => ({ value: type.id, label: type.name })), filters.typeId, (value) => ({ typeId: value }))
    case 'milestone':
      return one(
        'Milestone',
        [{ value: 'none', label: 'No milestone' }, ...milestones.map((milestone) => ({ value: milestone.id, label: milestone.name, description: milestone.targetDate ?? undefined }))],
        filters.milestoneId,
        (value) => ({ milestoneId: value }),
      )
    case 'blocked':
      return one(
        'Blocked',
        [
          { value: 'true', label: 'Blocked' },
          { value: 'false', label: 'Not blocked' },
        ],
        filters.blocked,
        (value) => ({ blocked: value as 'true' | 'false' }),
      )
    case 'claimed':
      return one(
        'Claimed',
        [
          { value: 'true', label: 'Claimed by an agent' },
          { value: 'false', label: 'Not claimed' },
        ],
        filters.claimed,
        (value) => ({ claimed: value as 'true' | 'false' }),
      )
    case 'labels':
      return <LabelFilter labels={labels} onApply={onApply} value={filters.labelIds} />
    case 'assignee':
      return <AssigneeFilter filters={filters} onApply={onApply} projectId={projectId} />
    case 'due':
      return <DueFilter filters={filters} onApply={onApply} />
    default:
      return null
  }
}

function LabelFilter({ labels, onApply, value }: { labels: Array<{ id: string; name: string; color: string }>; onApply: (values: Partial<FilterValues>, close?: boolean) => void; value: string | undefined }) {
  const selected = value && value !== 'none' ? value.split(',') : value === 'none' ? ['none'] : []
  return (
    <ComboboxList
      label="Labels"
      multiple
      onSelect={(picked) => {
        if (picked === 'none') return onApply({ labelIds: selected.includes('none') ? undefined : 'none' }, false)
        const next = selected.filter((id) => id !== 'none')
        const ids = next.includes(picked) ? next.filter((id) => id !== picked) : [...next, picked]
        onApply({ labelIds: ids.length ? ids.join(',') : undefined }, false)
      }}
      options={[
        { value: 'none', label: 'No labels' },
        ...labels.map((label) => ({ value: label.id, label: label.name, leading: <span aria-hidden="true" className="aw-label__dot" style={{ backgroundColor: label.color }} /> })),
      ]}
      selected={selected}
    />
  )
}

function AssigneeFilter({ filters, onApply, projectId }: { filters: FilterValues; onApply: (values: Partial<FilterValues>) => void; projectId: string }) {
  const [query, setQuery] = useState('')
  const debounced = useDebouncedValue(query, 200)
  const actors = useAssignableActors(projectId, debounced)
  return (
    <ComboboxList
      label="Assignee"
      loading={actors.isFetching}
      onQueryChange={setQuery}
      onSelect={(value) => onApply(value === 'none' ? { unassigned: true } : { assigneeActorId: value })}
      options={[
        ...(debounced.trim() ? [] : [{ value: 'none', label: 'Unassigned', leading: <Icon name="unassigned" size={16} /> }]),
        ...(actors.data ?? []).map((actor) => ({
          value: actor.id,
          label: actor.displayName,
          description: `@${actor.handle}`,
          leading: <Avatar kind={actorKind(actor.kind)} name={actor.displayName} seed={actor.id} size={18} />,
        })),
      ]}
      placeholder="Search people and agents"
      selected={filters.unassigned ? ['none'] : filters.assigneeActorId ? [filters.assigneeActorId] : []}
    />
  )
}

function DueFilter({ filters, onApply }: { filters: FilterValues; onApply: (values: Partial<FilterValues>, close?: boolean) => void }) {
  const [edge, setEdge] = useState<'before' | 'after'>('before')
  const current = { overdue: filters.overdue, dueBefore: filters.dueBefore, dueAfter: filters.dueAfter }
  return (
    <div className="aw-duefilter">
      <Button onClick={() => onApply({ ...current, overdue: !filters.overdue || undefined }, true)} size="sm" variant={filters.overdue ? 'primary' : 'secondary'}>
        Overdue
      </Button>
      <SegmentedControl
        label="Due date edge"
        onChange={setEdge}
        options={[
          { value: 'before', label: 'Due by' },
          { value: 'after', label: 'Due on or after' },
        ]}
        value={edge}
      />
      <Calendar
        key={edge}
        label={edge === 'before' ? 'Due by' : 'Due on or after'}
        onSelect={(day) => onApply({ ...current, ...(edge === 'before' ? { dueBefore: day } : { dueAfter: day }) }, false)}
        value={(edge === 'before' ? filters.dueBefore : filters.dueAfter) ?? null}
      />
    </div>
  )
}
