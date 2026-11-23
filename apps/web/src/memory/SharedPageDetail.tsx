// A page shared with someone on its own (C1.27), outside a space they're
// in: the page alone, with its comments when the share lets them comment,
// and its live editor when it lets them edit. Nothing from around it: not
// the pages beside or inside it, its history or who changed it.
import { Suspense, lazy, useCallback, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'

import { thingLink } from '../modules/references/thingLinks'
import { usePageTitle } from '../shell/documentTitleModel'
import { useShellPage } from '../shell/shellStore'
import { Banner, PageHeader, formatFullDate, formatRelative } from '../ui'
import { Markdown } from '../ui/markdown'
import type { Page, ShareLevel } from './api'
import { discussionKey, getDiscussion, type CommentAnchor } from './discussion/api'
import type { LiveDiscussion } from './live/LivePage'
import './memory.css'
import '../styles/pages.css'

const LivePage = lazy(() => import('./live/LivePage'))
const PageDiscussion = lazy(() => import('./discussion/PageDiscussion'))

const ALLOWS: Record<ShareLevel, string> = {
  see: 'You can read it.',
  comment: 'You can read it and comment.',
  edit: 'You can read it, comment and edit it.',
}

export default function SharedPageDetail({ level, page }: { level: ShareLevel; page: Page }) {
  useShellPage({ mainClassName: 'aw-main--page' })
  usePageTitle(page.title)
  const comments = level !== 'see'
  const discussion = useQuery({
    queryKey: discussionKey(page.projectId, page.id),
    queryFn: () => getDiscussion(page.id),
    enabled: comments,
  })
  const [draft, setDraft] = useState<CommentAnchor | null>(null)
  const [activeThread, setActiveThread] = useState<string | null>(null)
  const startComment = useCallback((anchor: CommentAnchor) => {
    setDraft(anchor)
    window.requestAnimationFrame(() => document.getElementById('page-discussion')?.scrollIntoView({ behavior: 'smooth', block: 'start' }))
  }, [])
  const liveDiscussion = useMemo<LiveDiscussion | null>(() => {
    if (!discussion.data?.canComment) return null
    return {
      threads: discussion.data.threads.flatMap((thread) =>
        !thread.resolved && thread.blockId && thread.quote ? [{ id: thread.id, blockId: thread.blockId, quote: thread.quote }] : [],
      ),
      active: activeThread,
      suggested: [],
      onComment: startComment,
      onOpenThread: setActiveThread,
    }
  }, [discussion.data, activeThread, startComment])
  const read = <Markdown className="aw-doc" empty="Nothing here yet." renderLink={thingLink} source={page.body} />
  const live = level === 'edit' && !page.archived
  return (
    <div className="aw-page aw-page--reading">
      <PageHeader icon="list" title={page.title}>
        Shared with you · {ALLOWS[level]} · edited{' '}
        <time dateTime={page.updatedAt} title={formatFullDate(page.updatedAt)}>
          {formatRelative(page.updatedAt)}
        </time>
      </PageHeader>
      {page.archived ? <Banner tone="offline">This page is archived.</Banner> : null}
      {live ? (
        <Suspense fallback={<div className="aw-memory__live-wait">{read}</div>}>
          <LivePage discussion={liveDiscussion} fallback={read} pageId={page.id} projectId={page.projectId} title={page.title} />
        </Suspense>
      ) : (
        read
      )}
      {comments && discussion.data ? (
        <Suspense fallback={null}>
          <PageDiscussion
            active={activeThread}
            discussion={discussion.data}
            draft={draft}
            onActivate={setActiveThread}
            onDraftDone={() => setDraft(null)}
            pageId={page.id}
            projectId={page.projectId}
          />
        </Suspense>
      ) : null}
    </div>
  )
}
