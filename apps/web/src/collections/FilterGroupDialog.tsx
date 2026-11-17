import { useState } from 'react'

import { clauseText, clausesWithinLimits, type FieldClause, type FieldDefinition, type FieldReference } from '../fields/fieldModel'
import { Button, Dialog, IconButton, Menu, SegmentedControl, type MenuEntry } from '../ui'
import { FieldFilterEditor } from '../work-items/FieldFilters'
import { useNamedActors } from '../work-items/hooks'
import type { FilterGroup, FilterItem } from './api'
import { groupClauses, isClause } from './collectionModel'

/**
 * Builds or changes one group of a view's filter (T1.13): conditions joined
 * by "any" (or) or "all" (and), beside the view's other filters, which all
 * still hold. A group saved with groups inside it keeps them: they show as
 * one line each and can be removed, not changed, here.
 */
export function FilterGroupDialog({
  fields,
  group,
  onApply,
  onClose,
  others,
  projectId,
}: {
  fields: readonly FieldDefinition[]
  /** The group to change; null starts a new "any of" group. */
  group: FilterGroup | null
  onApply: (group: FilterGroup | null) => void
  onClose: () => void
  /** The view's other conditions, so the whole filter keeps within its limits. */
  others: readonly FieldClause[]
  projectId: string
}) {
  const [join, setJoin] = useState<'or' | 'and'>(group && 'and' in group ? 'and' : 'or')
  const [items, setItems] = useState<FilterItem[]>(group ? ('and' in group ? group.and : group.or) : [])
  const [editing, setEditing] = useState<{ index: number | null; field: FieldDefinition; anchor: HTMLElement } | null>(null)
  const active = fields.filter((field) => !field.archived)
  const fieldOf = (id: string) => fields.find((field) => field.id === id)
  // People named in the group ("me" reads as itself).
  const people = [...new Set(items.flatMap(groupClauses).flatMap((clause) => (fieldOf(clause.field)?.type === 'person' ? (clause.values ?? []) : [])))]
  const named = useNamedActors(
    projectId,
    people.filter((id) => id !== 'me'),
  )
  const references: FieldReference[] = [
    { id: 'me', kind: 'person', label: 'me', key: null },
    ...(named.data ?? []).map((person) => ({ id: person.id, kind: 'person' as const, label: person.displayName, key: null })),
  ]
  const withItem = (index: number | null, clause: FieldClause) =>
    index === null ? [...items, clause] : items.map((item, at) => (at === index ? clause : item))
  const fits = (next: readonly FilterItem[]) => clausesWithinLimits([...others, ...next.flatMap(groupClauses)])

  // Focus is back on the menu's button when an entry runs, so the condition opens beside it.
  const addOn = (field: FieldDefinition) => () => {
    if (document.activeElement instanceof HTMLElement) setEditing({ index: null, field, anchor: document.activeElement })
  }
  const addItems: MenuEntry[] = active.length
    ? [{ heading: 'Add a condition on' }, ...active.map((field) => ({ label: field.name, onSelect: addOn(field) }))]
    : [{ label: 'This collection has no fields to filter by', disabled: true, onSelect: () => undefined }]

  const text = (item: FilterItem) => {
    if (isClause(item)) return clauseText(item, fieldOf(item.field), references)
    const count = groupClauses(item).length
    return `${count} ${count === 1 ? 'condition' : 'conditions'}, ${'or' in item ? 'any' : 'all'} of them`
  }

  return (
    <Dialog
      description="Records show when they meet this group and the view’s other filters."
      footer={
        <>
          {group ? (
            <Button onClick={() => onApply(null)} variant="ghost">
              Remove group
            </Button>
          ) : null}
          <span className="aw-collection__spacer" />
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button disabled={!items.length} onClick={() => onApply(join === 'or' ? { or: items } : { and: items })} variant="primary">
            {group ? 'Save group' : 'Add group'}
          </Button>
        </>
      }
      onClose={onClose}
      open
      title={group ? 'Change filter group' : 'Add filter group'}
    >
      <div className="aw-filtergroup">
        <SegmentedControl
          label="Records meet"
          onChange={setJoin}
          options={[
            { value: 'or' as const, label: 'Any of these' },
            { value: 'and' as const, label: 'All of these' },
          ]}
          value={join}
        />
        {items.length ? (
          <ul aria-label="Conditions" className="aw-filtergroup__items">
            {items.map((item, index) => {
              const field = isClause(item) ? fieldOf(item.field) : undefined
              return (
                <li className="aw-filtergroup__item" key={index}>
                  {index > 0 ? <span className="aw-filtergroup__join">{join === 'or' ? 'or' : 'and'}</span> : null}
                  {field && !field.archived ? (
                    <button className="aw-filtergroup__text" onClick={(event) => setEditing({ index, field, anchor: event.currentTarget })} type="button">
                      {text(item)}
                    </button>
                  ) : (
                    <span className="aw-filtergroup__text">{text(item)}</span>
                  )}
                  <IconButton icon="x" label={`Remove ${text(item)}`} onClick={() => setItems(items.filter((_, at) => at !== index))} size="sm" />
                </li>
              )
            })}
          </ul>
        ) : (
          <p className="aw-filtergroup__empty">No conditions yet. Add one, then another: records that meet {join === 'or' ? 'any' : 'all'} of them show.</p>
        )}
        <Menu items={addItems} label="Add a condition" trigger={<Button icon="plus" size="sm">Add a condition</Button>} />
      </div>
      {editing ? (
        <FieldFilterEditor
          anchor={editing.anchor}
          clause={editing.index === null ? null : ((items[editing.index] as FieldClause | undefined) ?? null)}
          field={editing.field}
          fits={(clause) => fits(withItem(editing.index, clause))}
          onApply={(clause) => setItems(withItem(editing.index, clause))}
          onClose={() => setEditing(null)}
          projectId={projectId}
        />
      ) : null}
    </Dialog>
  )
}
