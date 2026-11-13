import { ArrowUpDown, CalendarDays, Columns3, ListFilter, Rows3 } from 'lucide-react'

import { clauseText, type FieldDefinition, type FieldReference } from '../fields/fieldModel'
import { Button, Icon, Menu, TextField, type MenuEntry } from '../ui'
import { useNamedActors } from '../work-items/hooks'
import type { CollectionBody, SortRule, ViewConfig, ViewType } from './api'
import { columnOrder, dateFields, groupableFields, sortName, sortText, withColumnHidden } from './collectionModel'

/** What sorts a view: the title column, the record's number, when it last changed, or a field. */
function sortKeys(body: CollectionBody) {
  return [
    { field: 'title', name: body.collection.titleName },
    { field: 'number', name: 'Number' },
    { field: 'updated', name: 'Last changed' },
    ...body.fields.filter((field) => !field.archived).map((field) => ({ field: field.id, name: field.name })),
  ]
}

/**
 * Above a collection's records: search, then Filter, Sort, Group and Fields
 * menus, each change shown at once. Below them, a chip for each filter, the
 * sort and the grouping, to change or remove it. A view someone changed
 * offers Save view (for people who build the collection) and Reset.
 */
export function CollectionToolbar({
  archived,
  body,
  canBuild,
  canSave,
  changed,
  config,
  layout = 'table',
  onAddField,
  onArchived,
  onChange,
  onEditFilter,
  onFilter,
  onReset,
  onSave,
  onSearch,
  projectId,
  saving,
  search,
}: {
  /** Showing archived records instead. */
  archived: boolean
  body: CollectionBody
  canBuild: boolean
  canSave: boolean
  changed: boolean
  config: ViewConfig
  /** The view's layout: a board's Group sets its columns, a calendar picks its date field, a gallery doesn't group. */
  layout?: ViewType
  onAddField: () => void
  onArchived: (archived: boolean) => void
  onChange: (config: ViewConfig) => void
  onEditFilter: (index: number, anchor: HTMLElement) => void
  onFilter: (field: FieldDefinition, anchor: HTMLElement) => void
  onReset: () => void
  onSave: () => void
  onSearch: (search: string) => void
  projectId: string
  saving: boolean
  search: string
}) {
  const active = body.fields.filter((field) => !field.archived).sort((left, right) => left.position - right.position)
  // People named in filters ("me" reads as itself).
  const people = [...new Set(config.filters.flatMap((clause) => (active.find((field) => field.id === clause.field)?.type === 'person' ? (clause.values ?? []) : [])))]
  const named = useNamedActors(
    projectId,
    people.filter((id) => id !== 'me'),
  )
  const references: FieldReference[] = [
    { id: 'me', kind: 'person', label: 'me', key: null },
    ...(named.data ?? []).map((person) => ({ id: person.id, kind: 'person' as const, label: person.displayName, key: null })),
  ]
  // Focus is back on the menu's button when an entry runs, so the filter opens beside it.
  const anchorHere = (run: (anchor: HTMLElement) => void) => () => {
    if (document.activeElement instanceof HTMLElement) run(document.activeElement)
  }
  const filterItems: MenuEntry[] = active.length
    ? [{ heading: 'Filter by' }, ...active.map((field) => ({ label: field.name, onSelect: anchorHere((anchor) => onFilter(field, anchor)) }))]
    : [{ label: 'Add a field to filter by it', disabled: true, onSelect: () => undefined }]
  const sorted = config.sort[0] ?? null
  const sortItems: MenuEntry[] = [
    { heading: 'Sort by' },
    ...sortKeys(body).map(({ field, name }) => ({
      label: name,
      checked: sorted?.field === field,
      onSelect: () => onChange({ ...config, sort: [{ field, direction: sorted?.field === field && sorted.direction === 'asc' ? 'desc' : 'asc' } satisfies SortRule] }),
    })),
    ...(sorted ? [{ separator: true as const }, { label: 'Don’t sort', icon: 'x' as const, onSelect: () => onChange({ ...config, sort: [] }) }] : []),
  ]
  const groupable = groupableFields(body.fields)
  const groupItems: MenuEntry[] = groupable.length
    ? [
        { heading: 'Group by' },
        ...groupable.map((field) => ({ label: field.name, checked: config.groupBy === field.id, onSelect: () => onChange({ ...config, groupBy: field.id }) })),
        ...(config.groupBy && layout !== 'board' ? [{ separator: true as const }, { label: 'Don’t group', icon: 'x' as const, onSelect: () => onChange({ ...config, groupBy: null }) }] : []),
      ]
    : [{ label: 'Group by a choice, person or checkbox field', disabled: true, onSelect: () => undefined }]
  const dated = dateFields(body.fields)
  const dateItems: MenuEntry[] = dated.length
    ? [{ heading: 'Show records by' }, ...dated.map((field) => ({ label: field.name, checked: config.dateField === field.id, onSelect: () => onChange({ ...config, dateField: field.id }) }))]
    : [{ label: 'Add a date field to show records by it', disabled: true, onSelect: () => undefined }]
  const fieldsWord = layout === 'table' ? 'Columns' : 'Fields'
  const fieldItems: MenuEntry[] = [
    { heading: layout === 'table' ? 'Columns' : 'Fields on cards' },
    ...columnOrder(body.fields, config).map(({ field, hidden }) => ({
      label: field.name,
      checked: !hidden,
      keepOpen: true,
      onSelect: () => onChange(withColumnHidden(body.fields, config, field.id, !hidden)),
    })),
    ...(canBuild ? [{ separator: true as const }, { label: 'Add a field…', icon: 'plus' as const, onSelect: onAddField }] : []),
  ]
  const groupField = layout === 'gallery' || layout === 'calendar' ? undefined : active.find((field) => field.id === config.groupBy)

  return (
    <div className="aw-collection__toolbar">
      <div className="aw-collection__tools">
        <TextField
          aria-label="Search records"
          className="aw-collection__search"
          compact
          maxLength={200}
          onChange={(event) => onSearch(event.target.value)}
          placeholder="Search"
          prefix={<Icon name="search" size={14} />}
          type="search"
          value={search}
        />
        <Menu items={filterItems} label="Filter" trigger={<Button icon={ListFilter} size="sm" variant="ghost">Filter</Button>} />
        <Menu items={sortItems} label="Sort" trigger={<Button icon={ArrowUpDown} size="sm" variant="ghost">Sort</Button>} />
        {layout === 'gallery' || layout === 'calendar' ? null : (
          <Menu
            items={groupItems}
            label={layout === 'board' ? 'Columns by' : 'Group'}
            trigger={
              <Button icon={Rows3} size="sm" variant="ghost">
                {layout === 'board' ? 'Columns by' : 'Group'}
              </Button>
            }
          />
        )}
        {layout === 'calendar' ? (
          <Menu items={dateItems} label="Dates" trigger={<Button icon={CalendarDays} size="sm" variant="ghost">Dates</Button>} />
        ) : null}
        {layout === 'calendar' ? null : (
          <Menu items={fieldItems} label={fieldsWord} trigger={<Button icon={Columns3} size="sm" variant="ghost">{fieldsWord}</Button>} />
        )}
        <span className="aw-collection__spacer" />
        {body.archivedRecords > 0 || archived ? (
          <Button aria-pressed={archived} icon="archive" onClick={() => onArchived(!archived)} size="sm" variant="ghost">
            {archived ? 'Archived' : `Archived (${body.archivedRecords})`}
          </Button>
        ) : null}
        {changed ? (
          <>
            <Button onClick={onReset} size="sm" variant="ghost">
              Reset
            </Button>
            {canSave ? (
              <Button busy={saving} onClick={onSave} size="sm" variant="primary">
                Save view
              </Button>
            ) : null}
          </>
        ) : null}
      </div>
      {config.filters.length || sorted || groupField ? (
        <ul aria-label="What the view shows" className="aw-collection__chips">
          {config.filters.map((clause, index) => {
            const text = clauseText(
              clause,
              active.find((field) => field.id === clause.field),
              references,
            )
            return (
              <Chip
                key={`${clause.field}-${index}`}
                onEdit={(anchor) => onEditFilter(index, anchor)}
                onRemove={() => onChange({ ...config, filters: config.filters.filter((_, at) => at !== index) })}
                text={text}
              />
            )
          })}
          {sorted ? (
            <Chip
              onEdit={() => onChange({ ...config, sort: [{ ...sorted, direction: sorted.direction === 'asc' ? 'desc' : 'asc' }] })}
              onRemove={() => onChange({ ...config, sort: [] })}
              text={`Sorted by ${sortText(sorted, body.fields, body.collection.titleName)}`}
              title={`Sort ${sortName(sorted, body.fields, body.collection.titleName)} the other way`}
            />
          ) : null}
          {groupField && layout !== 'board' ? <Chip onRemove={() => onChange({ ...config, groupBy: null })} text={`Grouped by ${groupField.name}`} /> : null}
        </ul>
      ) : null}
    </div>
  )
}

function Chip({ onEdit, onRemove, text, title }: { onEdit?: (anchor: HTMLElement) => void; onRemove: () => void; text: string; title?: string }) {
  return (
    <li className="aw-collection__chip">
      {onEdit ? (
        <button className="aw-collection__chiptext" onClick={(event) => onEdit(event.currentTarget)} title={title ?? text} type="button">
          {text}
        </button>
      ) : (
        <span className="aw-collection__chiptext">{text}</span>
      )}
      <button aria-label={`Remove: ${text}`} className="aw-collection__chipremove" onClick={onRemove} type="button">
        <Icon name="x" size={12} />
      </button>
    </li>
  )
}
