import { useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { IconButton } from '../ui'
import { dropRemovedMessage, refreshMemoryCaches } from './liveMessages'
import { MessageEditor, RemoveMessageDialog } from './MessageEditing'
import { MessageHistoryDialog } from './MessageHistoryDialog'
import type {
  ConversationActor,
  ConversationCapabilities,
  ConversationMessage,
  ConversationTimelineEntry,
  ConversationWorkItemReference,
} from './types'

type MessageCapabilities = Pick<
  ConversationCapabilities,
  'editMessages' | 'removeOwnMessages' | 'removeAnyMessage'
>

/**
 * The one place a message's toolbar is put together: the page's own actions
 * (threads, pins, decisions) and edit and remove. Removed messages get none of
 * them; the row keeps only its copy-link button.
 *
 * Pages pass `toolbarFor` as renderToolbar, `editorFor` as renderEditor and
 * `openHistory` as onOpenHistory to TimelineEntries, and render `dialogs`.
 */
export function useMessageActions({
  actionsFor,
  capabilities,
  conversationId,
  onChanged,
  projectId,
  searchActors,
  searchWorkItems,
  viewerId,
}: {
  /** The page's buttons for a message that isn't removed. */
  actionsFor?: (message: ConversationMessage, entry: ConversationTimelineEntry) => ReactNode
  capabilities: MessageCapabilities | null | undefined
  conversationId: string
  /** A message was edited or removed here: show the returned entry and refresh. */
  onChanged: (entry: ConversationTimelineEntry) => void
  projectId: string
  searchActors: (query: string) => Promise<ConversationActor[]>
  searchWorkItems?: (query: string) => Promise<ConversationWorkItemReference[]>
  viewerId: string
}) {
  const queryClient = useQueryClient()
  // The revision the editor opened at: saving over a newer one is a conflict.
  const [editing, setEditing] = useState<{ conversationId: string; messageId: string; revision: number } | null>(null)
  const [removing, setRemoving] = useState<ConversationMessage | null>(null)
  const [history, setHistory] = useState<ConversationMessage | null>(null)

  const canEdit = (message: ConversationMessage) =>
    Boolean(capabilities?.editMessages) && message.author.id === viewerId && !message.removed
  const canRemove = (message: ConversationMessage) =>
    !message.removed &&
    (Boolean(capabilities?.removeAnyMessage) ||
      (Boolean(capabilities?.removeOwnMessages) && message.author.id === viewerId))

  const removed = (entry: ConversationTimelineEntry) => {
    const message = entry.message
    if (message) dropRemovedMessage(queryClient, { conversationId, messageId: message.id })
    onChanged(entry)
    refreshMemoryCaches(queryClient)
    void queryClient.invalidateQueries({ queryKey: ['direct'] })
    void queryClient.invalidateQueries({
      predicate: (query) => query.queryKey[0] === 'project' && query.queryKey[1] === projectId,
    })
  }

  const toolbarFor = (entry: ConversationTimelineEntry): ReactNode => {
    const message = entry.message
    if (!message || message.removed) return null
    return (
      <>
        {actionsFor?.(message, entry)}
        {canEdit(message) ? (
          <IconButton
            icon="pencil"
            label="Edit message"
            onClick={() => setEditing({ conversationId, messageId: message.id, revision: message.revision })}
            size="sm"
          />
        ) : null}
        {canRemove(message) ? (
          <IconButton icon="trash" label="Remove message" onClick={() => setRemoving(message)} size="sm" />
        ) : null}
      </>
    )
  }

  const editorFor = (entry: ConversationTimelineEntry): ReactNode => {
    const message = entry.message
    if (!message || !editing || editing.conversationId !== conversationId || editing.messageId !== message.id) {
      return undefined
    }
    // A message removed while it's being edited closes the editor.
    if (!canEdit(message)) return undefined
    return (
      <MessageEditor
        conversationId={conversationId}
        expectedRevision={editing.revision}
        key={message.id}
        message={message}
        onCancel={() => setEditing(null)}
        onSaved={(saved) => {
          setEditing(null)
          if (saved.message?.removed) removed(saved)
          else onChanged(saved)
        }}
        searchActors={searchActors}
        searchWorkItems={searchWorkItems}
      />
    )
  }

  const dialogs = (
    <>
      {removing ? (
        <RemoveMessageDialog
          conversationId={conversationId}
          message={removing}
          onClose={() => setRemoving(null)}
          onRemoved={removed}
          viewerId={viewerId}
        />
      ) : null}
      {history ? (
        <MessageHistoryDialog
          conversationId={conversationId}
          message={history}
          onClose={() => setHistory(null)}
          projectId={projectId}
          viewerId={viewerId}
        />
      ) : null}
    </>
  )

  return { toolbarFor, editorFor, openHistory: setHistory, dialogs }
}
