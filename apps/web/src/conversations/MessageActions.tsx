import { useRef, useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { IconButton } from '../ui'
import { setMessageReaction } from './api'
import { dropRemovedMessage, invalidateConversation, refreshMemoryCaches, setCachedReactions } from './liveMessages'
import { MessageEditor, RemoveMessageDialog } from './MessageEditing'
import { MessageHistoryDialog } from './MessageHistoryDialog'
import { MessageReactions, ReactionPicker } from './Reactions'
import { applyReaction, type ReactionChoice } from './reactions'
import type {
  ConversationActor,
  ConversationCapabilities,
  ConversationMessage,
  ConversationTimelineEntry,
  ConversationWorkItemReference,
  MessageReaction,
} from './types'

type MessageCapabilities = Pick<
  ConversationCapabilities,
  'editMessages' | 'removeOwnMessages' | 'removeAnyMessage' | 'sendMessage'
>

/**
 * The one place a message's toolbar is put together: the reaction picker, the
 * page's own actions (threads, pins, decisions) and edit and remove. Removed
 * messages get none of them; the row keeps only its copy-link button.
 *
 * Pages pass `toolbarFor` as renderToolbar, `reactionsFor` as renderReactions,
 * `editorFor` as renderEditor and `openHistory` as onOpenHistory to
 * TimelineEntries, and render `dialogs`.
 */
export function useMessageActions({
  actionsFor,
  capabilities,
  conversationId,
  onChanged,
  onReactionsChanged,
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
  /** Your reaction changed a message's reactions: show them on older pages too. */
  onReactionsChanged?: (messageId: string, reactions: MessageReaction[]) => void
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
  // The latest reaction request per message: an older answer arriving late
  // never replaces a newer one.
  const reactionRequests = useRef(new Map<string, number>())

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

  const canReact = (message: ConversationMessage) => Boolean(capabilities?.sendMessage) && !message.removed

  const showReactions = (messageId: string, reactions: MessageReaction[]) => {
    setCachedReactions(queryClient, { conversationId, messageId, reactions })
    onReactionsChanged?.(messageId, reactions)
  }

  // Show the change at once, then what the server says; on failure, put the
  // reactions back and refetch.
  const react = (message: ConversationMessage, choice: Pick<ReactionChoice, 'emoji' | 'name'>, reacted: boolean) => {
    const before = message.reactions ?? []
    const request = (reactionRequests.current.get(message.id) ?? 0) + 1
    reactionRequests.current.set(message.id, request)
    const latest = () => reactionRequests.current.get(message.id) === request
    showReactions(message.id, applyReaction(before, choice, viewerId, reacted))
    void setMessageReaction({ conversationId, messageId: message.id, name: choice.name, reacted })
      .then((result) => {
        if (latest()) showReactions(message.id, result.reactions)
      })
      .catch(() => {
        if (latest()) showReactions(message.id, before)
        invalidateConversation(queryClient, projectId, conversationId)
      })
  }

  const reactionsFor = (entry: ConversationTimelineEntry): ReactNode => {
    const message = entry.message
    if (!message || message.removed || !message.reactions?.length) return null
    return (
      <MessageReactions
        onToggle={
          canReact(message) ? (reaction) => react(message, reaction, !reaction.byViewer) : undefined
        }
        reactions={message.reactions}
        viewerId={viewerId}
      />
    )
  }

  const toolbarFor = (entry: ConversationTimelineEntry): ReactNode => {
    const message = entry.message
    if (!message || message.removed) return null
    return (
      <>
        {canReact(message) ? (
          <ReactionPicker
            onPick={(choice, reacted) => react(message, choice, reacted)}
            reactions={message.reactions ?? []}
          />
        ) : null}
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

  return { toolbarFor, reactionsFor, editorFor, openHistory: setHistory, dialogs }
}
