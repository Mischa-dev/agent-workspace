import { ListFilter } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { Avatar, Button, Icon, Menu, SegmentedControl, SelectField, TextField, actorKind, type MenuEntry } from '../ui'
import { Calendar, ComboboxList, Popover, type ComboboxOption } from '../ui/pickers'
import { workItemPriorities } from './constants'
import {
  CATEGORY_LABELS,
  FILTER_FIELDS,
  LIST_FIELDS,
  TOGGLE_FIELDS,
  filterChips,
  isListField,
  operatorChoices,
  operatorText,
  operatorsOf,
  valuesOf,
  withOperator,
  withValues,
  type FilterChip,
  type FilterField,
  type FilterOperator,
  type ListField,
} from './filterModel'
import { useAssignableActors, useWorkItem } from './hooks'
import {
  COLUMN_FIELDS,
  GROUPINGS,
  LANE_FIELDS,
  MAX_FILTER_VALUES,
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

type Open =
  /** The Filter menu's list of fields. */
  | { field: null; anchor: HTMLElement }
  /** A list filter's values, matched one way. */
  | { field: ListField; operator: FilterOperator; anchor: HTMLElement }
  | { field: Exclude<FilterField, ListField>; anchor: HTMLElement }

// Done work has its own Show done toggle on the list and board. A saved view
// that hides it outright still shows its chip, so it can be removed.
const MENU_FIELDS = FILTER_FIELDS.filter((entry) => entry.field !== 'done')

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
 * Search, filters as chips, and sort and grouping (or the board's columns and
 * lanes). Everything here lives in the URL; the list/board switch is in the
 * page header.
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
  const body = useRef<HTMLDivElement>(null)
  const labels = useLabels(projectId)
  const milestones = useMilestones(projectId)
  const assignees = useAssignableActors(projectId, '')
  const parent = useWorkItem(projectId, state.filters.parentId && state.filters.parentId !== 'none' ? state.filters.parentId : '')
  const actorNames = useMemo(() => new Map((assignees.data ?? []).map((actor) => [actor.id, actor.displayName])), [assignees.data])
  const lookups = { workflow, labels: labels.data, milestones: milestones.data, actorNames, parentKey: parent.data?.key ?? null }
  const chips = filterChips(state.filters, lookups)
  const apply = (field: FilterField, values: Partial<FilterValues>) => update((current) => setField(current, field, values))
  const change = (next: (filters: FilterValues) => FilterValues) => update((current) => ({ ...current, filters: next(current.filters) }))
  /** Opens a field's choices: a list field where its chip already matches, or "is". */
  const choose = (field: FilterField, anchor: HTMLElement) => {
    if (!isListField(field)) return setOpen({ field, anchor })
    const existing = chips.find((chip) => chip.kind === 'list' && chip.field === field)
    setOpen({ field, operator: existing?.kind === 'list' ? existing.operator : 'any', anchor })
  }
  // Picking a field swaps what the popover shows; focus goes to its search,
  // or else its first control, as it does when the popover opens.
  useEffect(() => {
    const root = body.current
    if (!root) return
    const search = root.querySelector<HTMLElement>('input:not([disabled])')
    if (search) search.focus({ preventScroll: true })
    else if (!root.contains(document.activeElement)) root.querySelector<HTMLElement>('button:not([disabled])')?.focus({ preventScroll: true })
  }, [open?.field])

  return (
    <div className="aw-filterbar">
      <div aria-label="Filter tasks" className="aw-filterbar__row" role="search">
        <TextField
          aria-label="Search tasks"
          className="aw-filterbar__search"
          maxLength={120}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="Search by title or key"
          prefix={<Icon name="search" size={14} />}
          type="search"
          value={search}
        />
        <Button aria-haspopup="dialog" icon={ListFilter} onClick={(event) => setOpen({ field: null, anchor: event.currentTarget })} size="sm" variant="secondary">
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
        <ShortcutHelp shortcuts={state.view === 'list' ? LIST_SHORTCUTS : BOARD_SHORTCUTS} />
      </div>
      {chips.length ? (
        <ul aria-label="Active filters" className="aw-filterbar__chips">
          {chips.map((chip) => (
            <li className="aw-chipfilter" key={chip.kind === 'list' ? chip.key : chip.field}>
              <ChipFace chip={chip} filters={state.filters} onChange={change} onOpen={(anchor) => (chip.kind === 'list' ? setOpen({ field: chip.field, operator: chip.operator, anchor }) : setOpen({ field: chip.field as Exclude<FilterField, ListField>, anchor }))} />
              <button
                aria-label={`Remove filter: ${chip.text}`}
                className="aw-chipfilter__remove"
                onClick={() => (chip.kind === 'list' ? change((filters) => withValues(filters, chip.key, [])) : apply(chip.field, {}))}
                type="button"
              >
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
          <div className="aw-filterbar__popover" ref={body}>
            {open.field === null ? (
              <ComboboxList
                label="Add a filter"
                onSelect={(value) => {
                  const field = value as FilterField
                  const toggle = TOGGLE_FIELDS[field]
                  if (toggle) {
                    apply(field, toggle)
                    setOpen(null)
                  } else choose(field, open.anchor)
                }}
                options={MENU_FIELDS.map((entry) => ({ value: entry.field, label: entry.label }))}
                placeholder="Filter by…"
                selected={chips.map((chip) => chip.field)}
              />
            ) : 'operator' in open ? (
              <ListFilterChooser
                field={open.field}
                filters={state.filters}
                labels={labels.data ?? []}
                milestones={milestones.data ?? []}
                onChange={change}
                onOperator={(operator) => {
                  change((filters) => withOperator(filters, open.field, open.operator, operator))
                  setOpen({ ...open, operator })
                }}
                operator={open.operator}
                projectId={projectId}
                workflow={workflow}
              />
            ) : (
              <FilterValueChooser
                field={open.field}
                filters={state.filters}
                onApply={(values, close = true) => {
                  apply(open.field, values)
                  if (close) setOpen(null)
                }}
              />
            )}
          </div>
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

/**
 * A chip's face: the field, how it matches (a menu to switch between "is"
 * and "is not") and its values, each opening what changes it. A filter that
 * reads as one phrase, such as "Unassigned", is one button.
 */
function ChipFace({
  chip,
  filters,
  onChange,
  onOpen,
}: {
  chip: FilterChip
  filters: FilterValues
  onChange: (next: (filters: FilterValues) => FilterValues) => void
  onOpen: (anchor: HTMLElement) => void
}) {
  const disabled = chip.kind === 'text' && Boolean(TOGGLE_FIELDS[chip.field])
  if (chip.kind === 'text' || !chip.verb) {
    return (
      <button
        aria-haspopup={disabled ? undefined : 'dialog'}
        className="aw-chipfilter__text"
        disabled={disabled}
        onClick={(event) => onOpen(event.currentTarget)}
        type="button"
      >
        {chip.kind === 'text' ? chip.text : chip.values}
      </button>
    )
  }
  const count = valuesOf(filters, chip.key).length
  const operators: MenuEntry[] = operatorsOf(chip.field).map((operator) => ({
    label: operatorText(chip.field, operator, chip.field === 'labels' ? 2 : count),
    checked: operator === chip.operator,
    onSelect: () => onChange((current) => withOperator(current, chip.field, chip.operator, operator)),
  }))
  return (
    <>
      <span aria-hidden="true" className="aw-chipfilter__field">
        {chip.label}
      </span>
      <Menu
        items={operators}
        label={`How ${chip.label.toLocaleLowerCase()} matches`}
        trigger={
          <button className="aw-chipfilter__op" type="button">
            <span className="visually-hidden">{chip.label} </span>
            {chip.verb}
          </button>
        }
      />
      <button aria-haspopup="dialog" className="aw-chipfilter__text aw-chipfilter__values" onClick={(event) => onOpen(event.currentTarget)} title={chip.text} type="button">
        <span className="visually-hidden">
          {chip.label} {chip.verb}{' '}
        </span>
        {chip.values}
      </button>
    </>
  )
}

/**
 * The values of a list filter, any number of them, and whether a task
 * matches with any of them, all of them (labels) or none of them. Each pick
 * applies at once and the list stays open.
 */
function ListFilterChooser({
  field,
  filters,
  labels,
  milestones,
  onChange,
  onOperator,
  operator,
  projectId,
  workflow,
}: {
  field: ListField
  filters: FilterValues
  labels: Array<{ id: string; name: string; color: string }>
  milestones: Array<{ id: string; name: string; targetDate: string | null }>
  onChange: (next: (filters: FilterValues) => FilterValues) => void
  onOperator: (operator: FilterOperator) => void
  operator: FilterOperator
  projectId: string
  workflow: WorkItemWorkflow | undefined
}) {
  const key = LIST_FIELDS[field].keys[operator] as keyof FilterValues
  const selected = valuesOf(filters, key)
  const full = selected.length >= MAX_FILTER_VALUES
  const toggle = (value: string) =>
    onChange((current) => {
      const values = valuesOf(current, key)
      if (values.includes(value)) return withValues(current, key, values.filter((entry) => entry !== value))
      // Every one of several labels can't also be no labels.
      if (operator === 'all') return withValues(current, key, value === 'none' ? ['none'] : [...values.filter((entry) => entry !== 'none'), value])
      return withValues(current, key, [...values, value])
    })
  const limit = <T extends { value: string; disabled?: boolean }>(options: T[]) =>
    full ? options.map((option) => (selected.includes(option.value) ? option : { ...option, disabled: true })) : options
  const label = FILTER_FIELDS.find((entry) => entry.field === field)?.label ?? field
  return (
    <div className="aw-listfilter">
      <SegmentedControl label={`How ${label.toLocaleLowerCase()} matches`} onChange={onOperator} options={operatorChoices(field)} value={operator} />
      {field === 'assignee' ? (
        <AssigneeOptions limit={limit} onSelect={toggle} projectId={projectId} selected={selected} />
      ) : (
        <ComboboxList
          label={label}
          multiple
          onSelect={toggle}
          options={limit(listOptions(field, workflow, labels, milestones))}
          selected={selected}
        />
      )}
      {full ? <p className="aw-listfilter__note">A filter takes up to {MAX_FILTER_VALUES} values.</p> : null}
    </div>
  )
}

function listOptions(
  field: Exclude<ListField, 'assignee'>,
  workflow: WorkItemWorkflow | undefined,
  labels: Array<{ id: string; name: string; color: string }>,
  milestones: Array<{ id: string; name: string; targetDate: string | null }>,
): ComboboxOption[] {
  switch (field) {
    case 'status':
      return (workflow?.statuses ?? []).map((status) => ({ value: status.id, label: status.name, leading: <StatusIcon category={status.semanticCategory} /> }))
    case 'category':
      return Object.entries(CATEGORY_LABELS).map(([value, label]) => ({ value, label, leading: <StatusIcon category={value} /> }))
    case 'priority':
      return workItemPriorities.map((priority) => ({ value: priority.value, label: priority.label, leading: <PriorityIcon decorative priority={priority.value} /> }))
    case 'type':
      return (workflow?.types ?? []).map((type) => ({ value: type.id, label: type.name }))
    case 'milestone':
      return [{ value: 'none', label: 'No milestone' }, ...milestones.map((milestone) => ({ value: milestone.id, label: milestone.name, description: milestone.targetDate ?? undefined }))]
    case 'labels':
      return [
        { value: 'none', label: 'No labels' },
        ...labels.map((label) => ({ value: label.id, label: label.name, leading: <span aria-hidden="true" className="aw-label__dot" style={{ backgroundColor: label.color }} /> })),
      ]
  }
}

/** People and agents to filter by, searched on the server; "Unassigned" first. */
function AssigneeOptions({
  limit,
  onSelect,
  projectId,
  selected,
}: {
  limit: (options: ComboboxOption[]) => ComboboxOption[]
  onSelect: (value: string) => void
  projectId: string
  selected: string[]
}) {
  const [query, setQuery] = useState('')
  const debounced = useDebouncedValue(query, 200)
  const actors = useAssignableActors(projectId, debounced)
  // Who's picked already stays at the top while nothing is typed.
  const listed = [...(actors.data ?? [])].sort((a, b) => (debounced.trim() ? 0 : Number(selected.includes(b.id)) - Number(selected.includes(a.id))))
  return (
    <ComboboxList
      label="Assignee"
      loading={actors.isFetching}
      multiple
      onQueryChange={setQuery}
      onSelect={onSelect}
      options={limit([
        ...(debounced.trim() ? [] : [{ value: 'none', label: 'Unassigned', leading: <Icon name="unassigned" size={16} /> }]),
        ...listed.map((actor) => ({
          value: actor.id,
          label: actor.displayName,
          description: `@${actor.handle}`,
          leading: <Avatar kind={actorKind(actor.kind)} name={actor.displayName} seed={actor.id} size={18} />,
        })),
      ])}
      placeholder="Search people and agents"
      selected={selected}
    />
  )
}

/** The choices of a filter that holds one value, such as Blocked or Due date. */
function FilterValueChooser({
  field,
  filters,
  onApply,
}: {
  field: Exclude<FilterField, ListField>
  filters: FilterValues
  onApply: (values: Partial<FilterValues>, close?: boolean) => void
}) {
  const one = (label: string, options: ComboboxOption[], selected: string | undefined, toValues: (value: string) => Partial<FilterValues>) => (
    <ComboboxList label={label} onSelect={(value) => onApply(toValues(value))} options={options} selected={selected ? [selected] : []} />
  )
  switch (field) {
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
    case 'due':
      return <DueFilter filters={filters} onApply={onApply} />
    default:
      return null
  }
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

