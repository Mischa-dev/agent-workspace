import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, invalidateOps, newRequestId, opQuery } from '../api/ops'
import type { HistoryEntry } from '../api/generated/operations'
import { Button, EmptyState, ErrorState, ListSkeleton, Row, RowList, describeError, formatRelative } from '../ui'
import { changedSummary, madeBy } from './historyModel'

export type HistoryPanelProps = {
  /** The thing whose versions these are. */
  thingId: string
  /** Its version now, which a restore must still find (read when restoring). */
  currentVersion: number | (() => Promise<number>)
  /** Whether the viewer may put an earlier version back. */
  canRestore: boolean
  /** A version was put back: the editor reloads. */
  onRestored?: (restored: number) => void
}

const PAGE = 30

/**
 * A thing's versions (C1.34), the same for pages, sheets and boards: who
 * made each, through which app and on whose request, what it changed, and
 * a way to put it back.
 */
export function HistoryPanel({ canRestore, currentVersion, onRestored, thingId }: HistoryPanelProps) {
  const queryClient = useQueryClient()
  const [before, setBefore] = useState<number | null>(null)
  const [older, setOlder] = useState<HistoryEntry[]>([])
  const listed = useQuery(opQuery('history_list', { thing: thingId, limit: PAGE }))
  const more = useQuery({ ...opQuery('history_list', { thing: thingId, limit: PAGE, before }), enabled: before !== null })
  const [busy, setBusy] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)

  if (listed.isPending) return <ListSkeleton label="Loading history" rows={4} />
  if (listed.isError) return <ErrorState error={listed.error} onRetry={() => void listed.refetch()} title="The history didn’t load" />
  const entries = [...listed.data.entries, ...older, ...(more.data?.entries ?? [])]
  const restorable = canRestore && listed.data.caps.restore
  const newest = listed.data.entries[0]?.version

  const restore = async (version: number) => {
    setBusy(version)
    setError(null)
    try {
      const expectedVersion = typeof currentVersion === 'number' ? currentVersion : await currentVersion()
      const done = await callOp('history_restore', { thing: thingId, version, expectedVersion }, { requestId: newRequestId() })
      await invalidateOps(queryClient, ['history_list', 'change_list'])
      onRestored?.(done.restored)
    } catch (reason) {
      setError(describeError(reason).message)
    } finally {
      setBusy(null)
    }
  }

  if (!entries.length) {
    return (
      <EmptyState icon="clock" title="No earlier versions yet">
        A version is kept as soon as this changes.
      </EmptyState>
    )
  }
  const last = entries[entries.length - 1]
  const page = more.data?.entries ?? listed.data.entries
  return (
    <div className="aw-history">
      {error ? <p className="aw-history__error" role="alert">{error}</p> : null}
      <RowList label="Versions">
        {entries.map((entry) => {
          const changed = changedSummary(entry)
          return (
            <Row
              actions={
                restorable && entry.version !== newest ? (
                  <Button busy={busy === entry.version} disabled={busy !== null} onClick={() => void restore(entry.version)} size="sm">
                    Restore
                  </Button>
                ) : null
              }
              key={entry.version}
              meta={
                <span>
                  {madeBy(entry)} · <time dateTime={entry.at}>{formatRelative(entry.at)}</time>
                  {changed ? ` · ${changed}` : ''}
                </span>
              }
              title={entry.summary}
            />
          )
        })}
      </RowList>
      {page.length === PAGE ? (
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
    </div>
  )
}
