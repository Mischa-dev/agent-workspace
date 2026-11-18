import type { RefObject } from 'react'

import { Button, EmptyState } from '../ui'

/** No records to show: none yet, or none that match. */
export function RecordsEmpty({
  filtered,
  inputRef,
  onClearFilters,
}: {
  filtered: boolean
  inputRef: RefObject<HTMLInputElement | null> | null
  onClearFilters: () => void
}) {
  return (
    <EmptyState
      action={
        filtered ? (
          <Button onClick={onClearFilters}>Clear filters</Button>
        ) : inputRef ? (
          <Button icon="plus" onClick={() => inputRef.current?.focus()}>
            New record
          </Button>
        ) : null
      }
      icon="table"
      title={filtered ? 'No matching records' : 'No records yet'}
    >
      {filtered ? 'Try a different search, or clear the filters.' : 'Each record is a row: give it a name, then fill in its fields here or from an agent.'}
    </EmptyState>
  )
}
