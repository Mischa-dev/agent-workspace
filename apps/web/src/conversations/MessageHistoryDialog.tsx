import { useQuery } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { Banner, Button, Dialog, MessageSkeleton } from '../ui'
import { getMessageRevisions } from './api'
import { EDIT_HISTORY_HINT } from './composerModel'
import { MessageContent } from './MessageContent'
import { formatFullDate } from './timeFormat'
import type { ConversationMessage, MessageContentV1, MessageRevision } from './types'

function revisionLabel(revision: MessageRevision) {
  return revision.revision === 1 ? 'Original' : `Edit ${revision.revision - 1}`
}

/**
 * Whether every mention and reference in an old version can still be shown
 * with the message's current lists. If not, its plain text is shown instead.
 */
function resolvable(content: MessageContentV1, message: ConversationMessage) {
  const actors = new Set(message.mentionedActors.map((actor) => actor.id))
  const items = new Set(message.referencedWorkItems.map((item) => item.id))
  return content.nodes.every(
    (node) =>
      node.type === 'text' ||
      (node.type === 'mention' && actors.has(node.actorId)) ||
      (node.type === 'work_item_ref' && items.has(node.workItemId)),
  )
}

/** Every version of a message, oldest first, with the current one marked. */
export function MessageHistoryDialog({
  conversationId,
  message,
  onClose,
  projectId,
  viewerId,
}: {
  conversationId: string
  message: ConversationMessage
  onClose: () => void
  projectId: string
  viewerId?: string
}) {
  const history = useQuery({
    queryKey: queryKeys.messageRevisions(projectId, conversationId, message.id),
    queryFn: () => getMessageRevisions(conversationId, message.id),
    retry: false,
  })
  const data = history.data
  const revisions = data?.revisions ?? []
  return (
    <Dialog
      description={EDIT_HISTORY_HINT}
      footer={<Button onClick={onClose}>Close</Button>}
      onClose={onClose}
      open
      title="Edit history"
    >
      {history.isPending ? <MessageSkeleton label="Loading earlier versions" rows={2} /> : null}
      {history.isError ? <Banner tone="danger">The earlier versions didn’t load. Close this and try again.</Banner> : null}
      {data?.removed ? <Banner tone="offline">This message was removed, and the text of every version was deleted.</Banner> : null}
      {revisions.length > 0 ? (
        <ol className="aw-history">
          {revisions.map((revision) => (
            <li className="aw-history__item" key={revision.revision}>
              <p className="aw-history__meta">
                <strong>{revisionLabel(revision)}</strong>
                <span aria-hidden="true"> · </span>
                <time dateTime={revision.writtenAt}>{formatFullDate(revision.writtenAt)}</time>
                {revision.via ? <span> · via {revision.via}</span> : null}
                {!data?.removed && revision.revision === data?.revision ? (
                  <span className="aw-history__current">Current</span>
                ) : null}
              </p>
              {revision.redacted || !revision.content ? (
                <p className="aw-history__redacted">Text removed</p>
              ) : (
                <div className="aw-history__body">
                  <MessageContent
                    actors={message.mentionedActors}
                    content={resolvable(revision.content, message) ? revision.content : null}
                    fallbackPlainText={revision.plainText ?? ''}
                    viewerId={viewerId}
                    workItems={message.referencedWorkItems}
                  />
                </div>
              )}
            </li>
          ))}
        </ol>
      ) : null}
    </Dialog>
  )
}
