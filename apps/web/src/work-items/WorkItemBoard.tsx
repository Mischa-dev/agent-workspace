import { useState } from 'react'
import { Link } from 'react-router-dom'

import { Avatar, Banner, IconButton, actorKind } from '../ui'
import { useMoveWorkItem } from './hooks'
import { workItemHref } from './links'
import type { WorkItemCard, WorkItemStatus, WorkflowStatus } from './types'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'

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
  const [target, setTarget] = useState<string | null>(null)
  const submitMove = (item: WorkItemCard, targetStatusId: string, targetPosition: number) => {
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
    <div className="aw-board-wrap">
      {move.isError ? (
        <Banner className="aw-page__banner" tone="warning">
          The board changed before that move finished. It’s up to date now, so try the move again.
        </Banner>
      ) : null}
      <section aria-label="Work item board" className="aw-board">
        {columns.map((column) => (
          <section
            aria-label={`${column.status.name} column`}
            className={target === column.status.id ? 'aw-board__column is-target' : 'aw-board__column'}
            key={column.status.id}
            onDragLeave={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setTarget(null)
            }}
            onDragOver={(event) => {
              if (disabled || !dragged) return
              event.preventDefault()
              if (target !== column.status.id) setTarget(column.status.id)
            }}
            onDrop={(event) => {
              event.preventDefault()
              if (dragged && !disabled && dragged.status.id !== column.status.id) {
                submitMove(dragged, column.status.id, column.items.length + 1)
              }
              setDragged(null)
              setTarget(null)
            }}
          >
            <header className="aw-board__head">
              <StatusIcon category={column.status.semanticCategory} />
              <h2 className="aw-board__title">{column.status.name}</h2>
              <span className="aw-board__count">
                {column.items.length}
                {column.hasMore ? '+' : ''}
              </span>
            </header>
            <ol className="aw-board__cards">
              {column.items.map((item, index) => (
                <li
                  className={dragged?.id === item.id ? 'aw-board__card is-dragging' : 'aw-board__card'}
                  draggable={!disabled && !move.isPending}
                  key={item.id}
                  onDragEnd={() => {
                    setDragged(null)
                    setTarget(null)
                  }}
                  onDragStart={(event) => {
                    event.dataTransfer.effectAllowed = 'move'
                    setDragged(item)
                  }}
                >
                  <Link className="aw-board__link" to={workItemHref(projectId, item.id)}>
                    <span className="aw-board__key">{item.key}</span>
                    <span className="aw-board__name">{item.title}</span>
                  </Link>
                  <div className="aw-board__meta">
                    <PriorityIcon priority={item.priority} />
                    <span className="aw-board__type">{item.itemType.name}</span>
                    {item.assignee ? (
                      <Avatar
                        className="aw-board__assignee"
                        kind={actorKind(item.assignee.kind)}
                        label
                        name={item.assignee.displayName}
                        seed={item.assignee.actorId}
                        size={20}
                      />
                    ) : null}
                  </div>
                  {!disabled ? (
                    <div className="aw-board__actions">
                      <IconButton
                        disabled={move.isPending || index === 0}
                        icon="arrow-up"
                        label={`Move ${item.key} up`}
                        onClick={() => submitMove(item, column.status.id, index)}
                        size="sm"
                      />
                      <IconButton
                        disabled={move.isPending || index === column.items.length - 1}
                        icon="arrow-down"
                        label={`Move ${item.key} down`}
                        onClick={() => submitMove(item, column.status.id, index + 2)}
                        size="sm"
                      />
                      <select
                        aria-label={`Move ${item.key} to status`}
                        className="aw-board__status"
                        disabled={move.isPending}
                        onChange={(event) => {
                          if (event.target.value !== column.status.id) submitMove(item, event.target.value, 1)
                        }}
                        value={column.status.id}
                      >
                        {statuses.map((status) => (
                          <option key={status.id} value={status.id}>
                            {status.name}
                          </option>
                        ))}
                      </select>
                    </div>
                  ) : null}
                </li>
              ))}
            </ol>
            {column.items.length === 0 ? <p className="aw-board__empty">Nothing here yet.</p> : null}
            {column.hasMore ? <p className="aw-board__empty">More are in the list view.</p> : null}
          </section>
        ))}
      </section>
    </div>
  )
}
