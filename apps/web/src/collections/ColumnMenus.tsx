import { canGroup, type FieldDefinition } from '../fields/fieldModel'
import { Button, IconButton, Menu, type MenuEntry } from '../ui'
import type { SortRule, ViewConfig } from './api'
import { sortText } from './collectionModel'

/** What a column's menu does; the page applies it to the view as shown, or opens a dialog. */
export type ColumnActions = {
  sort: (rule: SortRule) => void
  filter: (field: FieldDefinition, anchor: HTMLElement) => void
  group: (fieldId: string | null) => void
  hide: (fieldId: string) => void
  editField: (field: FieldDefinition) => void
  archiveField: (field: FieldDefinition) => void
  renameTitle: () => void
  addField: () => void
}

function sortEntries(field: string, fields: readonly FieldDefinition[], titleName: string, actions: ColumnActions): MenuEntry[] {
  return (['asc', 'desc'] as const).map((direction) => {
    const words = sortText({ field, direction }, fields, titleName).split(', ').slice(1).join(', ')
    return {
      label: `Sort ${words}`,
      icon: direction === 'asc' ? ('arrow-up' as const) : ('arrow-down' as const),
      onSelect: () => actions.sort({ field, direction }),
    }
  })
}

function sortMark(field: string, config: ViewConfig) {
  const first = config.sort[0]
  if (first?.field !== field) return undefined
  return first.direction === 'asc' ? ('arrow-up' as const) : ('arrow-down' as const)
}

/** The title column's header: sorting, and renaming the column for people who build collections. */
export function TitleHeader({
  actions,
  canBuild,
  config,
  fields,
  titleName,
}: {
  actions: ColumnActions
  canBuild: boolean
  config: ViewConfig
  fields: readonly FieldDefinition[]
  titleName: string
}) {
  const items: MenuEntry[] = [
    ...sortEntries('title', fields, titleName, actions),
    ...(canBuild ? [{ separator: true as const }, { label: 'Rename column…', icon: 'pencil' as const, onSelect: actions.renameTitle }] : []),
  ]
  return (
    <Menu
      items={items}
      label={`${titleName} column`}
      trigger={
        <Button className="aw-records__head" iconEnd={sortMark('title', config)} size="sm" variant="ghost">
          {titleName}
        </Button>
      }
    />
  )
}

/** A field's column header: sort, filter, group, hide, and editing or archiving the field for people who build collections. */
export function FieldHeader({
  actions,
  canBuild,
  config,
  field,
  fields,
  titleName,
}: {
  actions: ColumnActions
  canBuild: boolean
  config: ViewConfig
  field: FieldDefinition
  fields: readonly FieldDefinition[]
  titleName: string
}) {
  const grouped = config.groupBy === field.id
  const items: MenuEntry[] = [
    ...sortEntries(field.id, fields, titleName, actions),
    {
      label: 'Filter…',
      icon: 'search',
      // Focus is back on the header when this runs, so the filter opens beside it.
      onSelect: () => {
        if (document.activeElement instanceof HTMLElement) actions.filter(field, document.activeElement)
      },
    },
    ...(canGroup(field.type) ? [{ label: grouped ? 'Stop grouping' : `Group by ${field.name}`, icon: 'list' as const, onSelect: () => actions.group(grouped ? null : field.id) }] : []),
    { label: 'Hide column', icon: 'x', onSelect: () => actions.hide(field.id) },
    ...(canBuild
      ? [
          { separator: true as const },
          { label: 'Edit field…', icon: 'pencil' as const, onSelect: () => actions.editField(field) },
          { label: 'Archive field', icon: 'archive' as const, onSelect: () => actions.archiveField(field) },
        ]
      : []),
  ]
  return (
    <Menu
      items={items}
      label={`${field.name} column`}
      trigger={
        <Button className="aw-records__head" iconEnd={sortMark(field.id, config)} size="sm" title={field.description || undefined} variant="ghost">
          {field.name}
        </Button>
      }
    />
  )
}

/** The last header: adds a field (a column) for people who build collections. */
export function AddFieldHeader({ actions }: { actions: ColumnActions }) {
  return <IconButton icon="plus" label="Add a field" onClick={actions.addField} size="sm" />
}
