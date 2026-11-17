import { useInfiniteQuery } from '@tanstack/react-query'

import { callOp, opKey } from '../../api/ops'
import { Button, ErrorState, Row, RowList, Section, formatDateTime, formatFullDate } from '../../ui'

const PAGE = 20

/**
 * Who changed a thing, when and how (K3.20): read from the event log with
 * the reader's access, so it names the person, the app they acted through
 * and who asked, never what the text said. Shown only when there's
 * something to list.
 */
export default function ThingActivity({ id }: { id: string }) {
  const query = useInfiniteQuery({
    queryKey: opKey('activity_list', { thing: id, limit: PAGE }),
    queryFn: ({ pageParam, signal }) =>
      callOp('activity_list', { thing: id, limit: PAGE, ...(pageParam === null ? {} : { before: pageParam }) }, { signal, retries: 0 }),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => (last.entries.length === PAGE ? (last.entries[last.entries.length - 1]?.position ?? null) : null),
  })
  if (query.isError) return <ErrorState error={query.error} onRetry={() => void query.refetch()} title="Activity didn’t load" />
  const entries = query.data?.pages.flatMap((page) => page.entries) ?? []
  if (entries.length === 0) return null
  return (
    <Section description="Who changed it, when, and through which app." id={`activity-${id}`} title="Activity">
      <RowList label="Activity">
        {entries.map((entry) => (
          <Row
            key={entry.position}
            meta={
              <span className="aw-row__metaline aw-wrap">
                <time dateTime={entry.at} title={formatFullDate(entry.at)}>
                  {formatDateTime(entry.at)}
                </time>
                {entry.changed.length > 0 && <span>Changed {entry.changed.join(', ')}</span>}
                {entry.version != null && <span>Version {entry.version}</span>}
              </span>
            }
            title={
              <span>
                {entry.summary}
                {entry.actor ? ` · ${entry.actor}` : ''}
                {entry.via ? ` via ${entry.via}` : ''}
                {entry.requestedBy ? `, asked by ${entry.requestedBy}` : ''}
              </span>
            }
          />
        ))}
      </RowList>
      {query.hasNextPage && (
        <Button busy={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()} size="sm" variant="ghost">
          Show older
        </Button>
      )}
    </Section>
  )
}
