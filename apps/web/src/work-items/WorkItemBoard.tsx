import { useState } from 'react'
import { Link } from 'react-router-dom'

import { useMoveWorkItem } from './hooks'
import type {
  WorkItemCard,
  WorkItemStatus,
  WorkflowStatus,
} from './types'

export function WorkItemBoard({
  columns,
  disabled,
  projectId,
  statuses,
}: {
  columns: Array<{
    status: WorkItemStatus
    items: WorkItemCard[]
    hasMore: boolean
  }>
  disabled: boolean
  projectId: string
  statuses: WorkflowStatus[]
}) {
  const move = useMoveWorkItem()
  const [dragged, setDragged] = useState<WorkItemCard | null>(null)
  const submitMove = (
    item: WorkItemCard,
    targetStatusId: string,
    targetPosition: number,
  ) => {
    move.mutate({
      projectId,
      workItemId: item.id,
      expectedVersion: item.version,
      targetStatusId,
      targetPosition,
      idempotencyKey: crypto.randomUUID(),
    })
  }

  return (
    <section aria-label="Work Item board" className="work-item-board">
      {columns.map((column) => (
        <section
          aria-label={`${column.status.name} column`}
          className="work-item-board-column"
          data-category={column.status.semanticCategory}
          key={column.status.id}
          onDragOver={(event) => { if (!disabled) event.preventDefault() }}
          onDrop={(event) => {
            event.preventDefault()
            if (dragged && !disabled) {
              submitMove(dragged, column.status.id, column.items.length + 1)
            }
            setDragged(null)
          }}
        >
          <header>
            <h2>{column.status.name}</h2>
            <span>{column.items.length}{column.hasMore ? '+' : ''}</span>
          </header>
          <ol>
            {column.items.map((item, index) => (
              <li
                draggable={!disabled && !move.isPending}
                key={item.id}
                onDragEnd={() => setDragged(null)}
                onDragStart={() => setDragged(item)}
              >
                <Link to={`/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(item.id)}`}>
                  <span className="work-item-key">{item.key}</span>
                  <strong>{item.title}</strong>
                </Link>
                <span className="board-card-meta">
                  {item.itemType.name} · {item.assignee?.displayName ?? 'Unassigned'}
                </span>
                {!disabled ? (
                  <div className="board-card-actions">
                    <button aria-label={`Move ${item.key} up`} className="text-button" disabled={move.isPending || index === 0} onClick={() => submitMove(item, column.status.id, index)} type="button">↑</button>
                    <button aria-label={`Move ${item.key} down`} className="text-button" disabled={move.isPending || index === column.items.length - 1} onClick={() => submitMove(item, column.status.id, index + 2)} type="button">↓</button>
                    <label>
                      <span className="visually-hidden">Move {item.key} to status</span>
                      <select aria-label={`Move ${item.key} to status`} disabled={move.isPending} onChange={(event) => { if (event.target.value !== column.status.id) submitMove(item, event.target.value, 1) }} value={column.status.id}>
                        {statuses.map((status) => <option key={status.id} value={status.id}>{status.name}</option>)}
                      </select>
                    </label>
                  </div>
                ) : null}
              </li>
            ))}
          </ol>
          {column.items.length === 0 ? <p>Move an item here with its status control.</p> : null}
          {column.hasMore ? <p>More items are available in the filtered list.</p> : null}
        </section>
      ))}
      {move.isError ? (
        <p className="form-error-summary board-error" role="alert">
          The board changed before the move completed. It has been refreshed; try the move again.
        </p>
      ) : null}
    </section>
  )
}

