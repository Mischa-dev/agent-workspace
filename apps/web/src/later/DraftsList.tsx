import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { callOp } from '../api/ops'
import { useHomeWorkspaceId } from '../experience/layout'
import { projectLocationHref } from '../projects/locations'
import { EmptyState, ErrorState, ListSkeleton, formatRelative, plainPreview } from '../ui'

/** Drafts (C2.28): what you started writing and haven't sent, on any device. */
export function DraftsList() {
  const workspaceId = useHomeWorkspaceId()
  const drafts = useQuery({
    queryKey: ['drafts', workspaceId],
    queryFn: () => callOp('message_draft_list', {}, { workspaceId: workspaceId ?? undefined }),
    enabled: Boolean(workspaceId),
  })
  if (drafts.isPending) return <ListSkeleton label="Loading drafts" rows={3} />
  if (drafts.isError) return <ErrorState error={drafts.error} onRetry={() => void drafts.refetch()} title="Drafts didn’t load" />
  const list = drafts.data.drafts
  if (!list.length) {
    return (
      <EmptyState icon="compose" title="No drafts">
        A message you start and don’t send waits here, and in its conversation on your other devices.
      </EmptyState>
    )
  }
  return (
    <ul aria-label="Drafts" className="aw-later__list">
      {list.map((draft) => {
        const text = typeof (draft.draft as { text?: unknown }).text === 'string' ? (draft.draft as { text: string }).text : ''
        const href =
          draft.conversationKind === 'dm' || draft.conversationKind === 'group'
            ? `/messages/${encodeURIComponent(draft.conversationId)}`
            : projectLocationHref(draft.spaceId, {
                kind: draft.conversationKind === 'thread' ? 'thread' : 'channel',
                conversationId: draft.conversationId,
              })
        return (
          <li className="aw-later__row" key={draft.conversationId}>
            <Link className="aw-later__draft" to={href}>
              <strong>{draft.conversationName ?? (draft.conversationKind === 'dm' ? 'Direct message' : 'Conversation')}</strong>
              <span>{plainPreview(text, 160)}</span>
              <small>{formatRelative(draft.updatedAt)}</small>
            </Link>
          </li>
        )
      })}
    </ul>
  )
}
