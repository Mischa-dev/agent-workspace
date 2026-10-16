import { useState, type ReactNode } from 'react'

import { ComboboxList, Popover } from '../ui'
import { useTaskSearch } from './planningHooks'
import { useDebouncedValue } from './pickerModel'
import { StatusIcon } from './WorkItemIcons'

const KEY = /^[A-Za-z][A-Za-z0-9]*-\d+$/

/**
 * Find a task by key or title, for parents, dependencies and relations. A
 * typed key such as AW-12 can be used even when the search hasn't found it,
 * since blockers may live in another project.
 */
export function TaskSearchPicker({
  allowTypedKey = true,
  anchor,
  exclude,
  extra,
  label,
  noneLabel,
  onClose,
  onPick,
  projectId,
}: {
  /** Offers "Use AW-12" for a typed key the search didn't find. Parents live in the same project, so they turn it off. */
  allowTypedKey?: boolean
  anchor: HTMLElement | null
  exclude: readonly string[]
  /** Something to show above the results, such as a "close as duplicate" choice. */
  extra?: ReactNode
  label: string
  /** Offers a "none" option (for clearing a parent). */
  noneLabel?: string
  onClose: () => void
  /** The task's id, or the typed key; null for the none option. */
  onPick: (target: { id: string | null; key: string }) => void
  projectId: string
}) {
  const [query, setQuery] = useState('')
  const debounced = useDebouncedValue(query, 200)
  const results = useTaskSearch(projectId, debounced, anchor !== null)
  const found = (results.data ?? []).filter((task) => !exclude.includes(task.id))
  const typed = query.trim()
  return (
    <Popover anchor={anchor} label={label} onClose={onClose} open={anchor !== null} width={340}>
      {extra}
      <ComboboxList
        createLabel={(text) => `Use ${text.toUpperCase()}`}
        emptyText={results.isError ? 'Search didn’t work. Try again.' : typed ? 'No work items match' : 'Type a key or a few words'}
        label={label}
        loading={results.isFetching}
        onClose={onClose}
        onCreate={
          allowTypedKey && KEY.test(typed) && !found.some((task) => task.key.toLowerCase() === typed.toLowerCase())
            ? (text) => {
                onPick({ id: null, key: text.toUpperCase() })
                onClose()
              }
            : undefined
        }
        onQueryChange={setQuery}
        onSelect={(value) => {
          if (value === '__none') return onPick({ id: null, key: '' })
          const task = found.find((candidate) => candidate.id === value)
          if (task) onPick({ id: task.id, key: task.key })
        }}
        options={[
          ...(noneLabel && !typed ? [{ value: '__none', label: noneLabel }] : []),
          ...found.map((task) => ({
            value: task.id,
            label: task.title,
            description: task.key,
            keywords: task.key,
            leading: <StatusIcon category={task.statusCategory} />,
          })),
        ]}
        placeholder="Search by key or title"
        selected={[]}
      />
    </Popover>
  )
}
