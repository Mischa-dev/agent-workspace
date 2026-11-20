import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { opKey, opQuery, useOp } from '../api/ops'
import { Banner, Button, EmptyState, ErrorState, ListSkeleton, Row, RowList, describeError, formatFullDate, formatRelative } from '../ui'

// The history panel (W1.21): what happened to a thing, newest first, read
// through the history seam (`history_list`, K3) whatever its kind. Each
// entry says who, through which connected app when it shows, when, and what
// changed. Where the kind can restore and the viewer may, an entry can be
// put back: that makes a new version, and the current one stays in history.

export type HistoryRestore = {
  /** The thing's current version, which the restore is held to. */
  expectedVersion: number
  /** What a restore leaves the viewer with to say, once it worked. */
  onRestored?: (version: number) => void
}

export function HistoryPanel({
  emptyText = 'Nothing has changed yet.',
  label = 'History',
  restore,
  thing,
}: {
  emptyText?: string
  label?: string
  /** Given when the viewer may restore; left out, the list only reads. */
  restore?: HistoryRestore
  /** The thing's id. */
  thing: string
}) {
  const queryClient = useQueryClient()
  const history = useQuery(opQuery('history_list', { thing }))
  const restoring = useOp('history_restore')
  const [restored, setRestored] = useState<number | null>(null)

  if (history.isPending) return <ListSkeleton label="Loading history" rows={3} />
  if (history.isError) return <ErrorState error={history.error} onRetry={() => void history.refetch()} title="The history didn’t load" />
  const { caps, entries } = history.data
  if (!entries.length) {
    return (
      <EmptyState icon="clock" title="No history yet">
        {emptyText}
      </EmptyState>
    )
  }
  const canRestore = Boolean(restore) && caps.restore
  const put = (version: number) => {
    if (!restore) return
    restoring.mutate(
      { thing, version, expectedVersion: restore.expectedVersion },
      {
        onSuccess: () => {
          setRestored(version)
          void queryClient.invalidateQueries({ queryKey: opKey('history_list', { thing }) })
          restore.onRestored?.(version)
        },
      },
    )
  }
  return (
    <div className="aw-history" data-piece="history">
      <RowList label={label}>
        {entries.map((entry, index) => (
          <Row
            actions={
              canRestore && index > 0 ? (
                <Button
                  busy={restoring.isPending && restoring.variables?.version === entry.version}
                  disabled={restoring.isPending}
                  onClick={() => put(entry.version)}
                  size="sm"
                >
                  Restore
                </Button>
              ) : null
            }
            key={entry.version}
            meta={
              <>
                <span>
                  {entry.actor || 'Someone'}
                  {entry.via ? ` through ${entry.via}` : ''}
                  {entry.acceptedBy ? `, accepted by ${entry.acceptedBy}` : ''}
                </span>
                <time dateTime={entry.at} title={formatFullDate(entry.at)}>
                  {formatRelative(entry.at)}
                </time>
              </>
            }
            title={entry.summary}
          />
        ))}
      </RowList>
      {restored !== null ? <Banner tone="success">Restored. It’s a new version now, and the one before stays here.</Banner> : null}
      {restoring.isError ? <Banner tone="danger">{describeError(restoring.error).message}</Banner> : null}
    </div>
  )
}
