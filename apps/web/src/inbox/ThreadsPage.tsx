import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { opQuery } from '../api/ops'
import { useAuthenticatedSession } from '../auth/session'
import { ThreadStatusChip } from '../conversations/threadBoard'
import { projectLocationHref } from '../projects/locations'
import { AppShell } from '../shell/AppShell'
import { EmptyState, ErrorState, ListSkeleton, PageHeader, Row, RowList, SegmentedControl, countLabel, formatFullDate, formatRelative } from '../ui'

type Show = 'open' | 'all'

const SHOW: { value: Show; label: string }[] = [
  { value: 'open', label: 'Open' },
  { value: 'all', label: 'All' },
]

/**
 * The threads you follow in every space (C2.36), newest reply first, with
 * each one's status and the outcome it settled on.
 */
export function ThreadsPage() {
  const session = useAuthenticatedSession()
  const [show, setShow] = useState<Show>('open')
  const threads = useQuery(opQuery('thread_followed_list', show === 'open' ? { openOnly: true } : {}))
  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page">
        <PageHeader icon="messages" title="Threads" />
        <SegmentedControl label="Show" onChange={setShow} options={SHOW} value={show} />
        {threads.isPending ? <ListSkeleton label="Loading threads" rows={5} /> : null}
        {threads.isError ? <ErrorState error={threads.error} onRetry={() => void threads.refetch()} title="Threads didn’t load" /> : null}
        {threads.data ? (
          threads.data.threads.length ? (
            <RowList label="Threads you follow">
              {threads.data.threads.map((thread) => {
                const where = thread.parentName ? `${thread.spaceName} · #${thread.parentName}` : thread.spaceName
                return (
                  <Row
                    actions={thread.status.toLowerCase() !== 'open' ? <ThreadStatusChip status={thread.status} /> : null}
                    key={thread.id}
                    meta={
                      <>
                        {where} · {countLabel(thread.messageCount, 'message')} ·{' '}
                        <span title={formatFullDate(thread.lastActivityAt)}>{formatRelative(thread.lastActivityAt)}</span>
                        {thread.unread ? <strong> · New</strong> : null}
                      </>
                    }
                    title={
                      <Link to={projectLocationHref(thread.spaceId, { kind: 'thread', conversationId: thread.id })}>{thread.name}</Link>
                    }
                  >
                    {thread.outcome ? <p className="aw-row__note">Outcome: {thread.outcome}</p> : null}
                  </Row>
                )
              })}
            </RowList>
          ) : (
            <EmptyState icon="messages" title={show === 'open' ? 'No open threads you follow' : 'No threads you follow'}>
              Threads you start, reply in or are mentioned in show up here, newest reply first.
            </EmptyState>
          )
        ) : null}
      </div>
    </AppShell>
  )
}
