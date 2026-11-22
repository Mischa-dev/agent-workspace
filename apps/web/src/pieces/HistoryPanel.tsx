import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import type { HistoryEntry } from '../api/generated/operations'
import { invalidateOps, opQuery, useOp } from '../api/ops'
import { Banner, Button, EmptyState, ErrorState, ListSkeleton, Row, RowList, describeError, formatFullDate, formatRelative } from '../ui'
import { changedSummary, madeBy } from './historyModel'

// The history panel (W1.21, C1.34): what happened to a thing, newest first,
// read through the history seam (`history_list`, K3) whatever its kind.
// Each entry says who, through which connected app and on whose request,
// when, and what changed. Where the kind can restore and the viewer may, an
// entry can be put back: that makes a new version, and the current one
// stays in history.

export type HistoryRestore = {
  /**
   * The thing's current version, which the restore is held to; a function
   * reads it when restoring, for editors whose version moves while open.
   */
  expectedVersion: number | (() => Promise<number>)
  /** What a restore leaves the viewer with to say, once it worked. */
  onRestored?: (version: number) => void
}

const PAGE = 30

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
  const history = useQuery(opQuery('history_list', { thing, limit: PAGE }))
  const [before, setBefore] = useState<number | null>(null)
  const [older, setOlder] = useState<HistoryEntry[]>([])
  const more = useQuery({ ...opQuery('history_list', { thing, limit: PAGE, before }), enabled: before !== null })
  const restoring = useOp('history_restore')
  const [restored, setRestored] = useState<number | null>(null)
  const [failed, setFailed] = useState<string | null>(null)

  if (history.isPending) return <ListSkeleton label="Loading history" rows={3} />
  if (history.isError) return <ErrorState error={history.error} onRetry={() => void history.refetch()} title="The history didn’t load" />
  const { caps } = history.data
  const entries = [...history.data.entries, ...older, ...(more.data?.entries ?? [])]
  if (!entries.length) {
    return (
      <EmptyState icon="clock" title="No history yet">
        {emptyText}
      </EmptyState>
    )
  }
  const canRestore = Boolean(restore) && caps.restore
  const newest = history.data.entries[0]?.version
  const put = async (version: number) => {
    if (!restore) return
    setFailed(null)
    try {
      const expectedVersion = typeof restore.expectedVersion === 'number' ? restore.expectedVersion : await restore.expectedVersion()
      await restoring.mutateAsync({ thing, version, expectedVersion })
      setRestored(version)
      await invalidateOps(queryClient, ['history_list', 'change_list'])
      restore.onRestored?.(version)
    } catch (reason) {
      setFailed(describeError(reason).message)
    }
  }
  const lastPage = more.data?.entries ?? history.data.entries
  const last = entries[entries.length - 1]
  return (
    <div className="aw-history" data-piece="history">
      <RowList label={label}>
        {entries.map((entry) => {
          const changed = changedSummary(entry)
          return (
            <Row
              actions={
                canRestore && entry.version !== newest ? (
                  <Button
                    busy={restoring.isPending && restoring.variables?.version === entry.version}
                    disabled={restoring.isPending}
                    onClick={() => void put(entry.version)}
                    size="sm"
                  >
                    Restore
                  </Button>
                ) : null
              }
              key={entry.version}
              meta={
                <>
                  <span>{madeBy(entry)}</span>
                  <time dateTime={entry.at} title={formatFullDate(entry.at)}>
                    {formatRelative(entry.at)}
                  </time>
                  {changed ? <span>{changed}</span> : null}
                </>
              }
              title={entry.summary}
            />
          )
        })}
      </RowList>
      {lastPage.length === PAGE ? (
        <Button
          busy={more.isFetching}
          onClick={() => {
            if (more.data) setOlder((kept) => [...kept, ...more.data.entries])
            setBefore(last.version)
          }}
          size="sm"
          variant="ghost"
        >
          Show older versions
        </Button>
      ) : null}
      {restored !== null ? <Banner tone="success">Restored. It’s a new version now, and the one before stays here.</Banner> : null}
      {failed ? <Banner tone="danger">{failed}</Banner> : null}
    </div>
  )
}
