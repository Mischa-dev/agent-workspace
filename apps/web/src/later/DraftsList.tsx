import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../api/ops'
import { here } from '../app/href'
import { useWorkspaceId } from '../app/workspace'
import { projectLocationHref } from '../projects/locations'
import { ListPiece, ListPieceTitle } from '../pieces/ListPiece'
import { EmptyState, ErrorState, Icon, ListSkeleton, formatRelative, plainPreview } from '../ui'

/** Drafts (C2.28): what you started writing and haven't sent, on any device. */
export function DraftsList() {
  const workspaceId = useWorkspaceId()
  const drafts = useQuery({
    ...opQuery('message_draft_list', {}, { workspaceId: workspaceId ?? undefined }),
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
  const hrefOf = (draft: (typeof list)[number]) =>
    draft.conversationKind === 'dm' || draft.conversationKind === 'group'
      ? here(`/messages/${encodeURIComponent(draft.conversationId)}`)
      : projectLocationHref(draft.spaceId, {
          kind: draft.conversationKind === 'thread' ? 'thread' : 'channel',
          conversationId: draft.conversationId,
        })
  const textOf = (draft: (typeof list)[number]) =>
    typeof (draft.draft as { text?: unknown }).text === 'string' ? (draft.draft as { text: string }).text : ''
  return (
    <ListPiece
      aside={(draft) => <time dateTime={draft.updatedAt}>{formatRelative(draft.updatedAt)}</time>}
      itemKey={(draft) => draft.conversationId}
      items={list}
      label="Drafts"
      leading={() => (
        <span className="aw-later__icon">
          <Icon name="compose" size={16} />
        </span>
      )}
      meta={(draft) => plainPreview(textOf(draft), 160)}
      title={(draft) => (
        <ListPieceTitle to={hrefOf(draft)}>
          {draft.conversationName ?? (draft.conversationKind === 'dm' ? 'Direct message' : 'Conversation')}
        </ListPieceTitle>
      )}
      wholeRow
    />
  )
}
