import { useRef, useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { toastError } from '../app/mutations'
import { IconButton, Menu, showToast, type MenuEntry } from '../ui'
import { markConversationUnread, setMessageReaction } from './api'
import { dropRemovedMessage, invalidateConversation, refreshMemoryCaches, setCachedReactions } from './liveMessages'
import { MessageEditor, RemoveMessageDialog } from './MessageEditing'
import { MessageHistoryDialog } from './MessageHistoryDialog'
import { useMessageMemory } from './MessageMemory'
import { MessageReactions, ReactionPicker } from './Reactions'
import { applyReaction, type ReactionChoice } from './reactions'
import { useMessageLater } from './useMessageLater'
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
  'editMessages' | 'removeOwnMessages' | 'removeAnyMessage' | 'sendMessage' | 'markSeen'
>

/** A message's address in this conversation, by default the page it's on. */
function pageLink(messageId: string) {
  return `${window.location.pathname}${window.location.search}#message-${messageId}`
}

/**
 * After an edit, back to the box you write in: the nearest conversation
 * composer around the message (the thread's own beside a channel).
 */
function focusComposerNear(element: HTMLElement | null) {
  let scope = element?.parentElement ?? null
  while (scope) {
    const box = scope.querySelector<HTMLTextAreaElement>('form[data-composer="conversation"] textarea')
    if (box) {
      box.focus()
      return
    }
    scope = scope.parentElement
  }
}

/**
 * The one place a message's toolbar is put together. Four buttons show: add a
 * reaction, the page's own main action (a thread), save for later, and More.
 * Everything else is in More: edit, forward, create task, pin, save as
 * decision, mark unread, copy, remind me, and remove last. A removed message
 * keeps only Copy link.
 *
 * Pages pass `toolbarFor` as renderToolbar, `reactionsFor` as renderReactions,
 * `editorFor` as renderEditor and `openHistory` as onOpenHistory to
 * TimelineEntries, `editLast` to the composer's onEditLast, and render
 * `dialogs`.
 */
export function useMessageActions({
  capabilities,
  conversationId,
  direct = false,
  linkFor = pageLink,
  onChanged,
  onMarkedUnread,
  onReactionsChanged,
  primaryFor,
  projectId,
  searchActors,
  searchWorkItems,
  taskProject = false,
  viewerId,
}: {
  capabilities: MessageCapabilities | null | undefined
  conversationId: string
  /** A DM or group. */
  direct?: boolean
  /** Where a link to a message goes, such as a thread's own page when it's open beside its channel. */
  linkFor?: (messageId: string) => string
  /** A message was edited or removed here: show the returned entry and refresh. */
  onChanged: (entry: ConversationTimelineEntry) => void
  /** Your read mark moved back to this message: move the New line to it. */
  onMarkedUnread?: (entry: ConversationTimelineEntry) => void
  /** Your reaction changed a message's reactions: show them on older pages too. */
  onReactionsChanged?: (messageId: string, reactions: MessageReaction[]) => void
  /** The page's main button for a message that isn't removed, such as its thread. */
  primaryFor?: (message: ConversationMessage, entry: ConversationTimelineEntry) => ReactNode
  projectId: string
  searchActors: (query: string) => Promise<ConversationActor[]>
  searchWorkItems?: (query: string) => Promise<ConversationWorkItemReference[]>
  /** In a project with tasks: messages can become tasks. */
  taskProject?: boolean
  viewerId: string
}) {
  const queryClient = useQueryClient()
  // The revision the editor opened at: saving over a newer one is a conflict.
  const [editing, setEditing] = useState<{ conversationId: string; messageId: string; revision: number } | null>(null)
  const [removing, setRemoving] = useState<ConversationMessage | null>(null)
  const [history, setHistory] = useState<ConversationMessage | null>(null)
  const editorHome = useRef<HTMLDivElement | null>(null)
  // The latest reaction request per message: an older answer arriving late
  // never replaces a newer one.
  const reactionRequests = useRef(new Map<string, number>())
  const later = useMessageLater({ conversationId, projectId, taskProject })
  const canPost = Boolean(capabilities?.sendMessage)
  const memory = useMessageMemory({ conversationId, direct, enabled: canPost, projectId })

  const canEdit = (message: ConversationMessage) =>
    Boolean(capabilities?.editMessages) && message.author.id === viewerId && !message.removed
  const canRemove = (message: ConversationMessage) =>
    !message.removed &&
    (Boolean(capabilities?.removeAnyMessage) ||
      (Boolean(capabilities?.removeOwnMessages) && message.author.id === viewerId))

  const refreshLists = () => {
    void queryClient.invalidateQueries({ queryKey: ['direct'] })
    void queryClient.invalidateQueries({
      predicate: (query) => query.queryKey[0] === 'project' && query.queryKey[1] === projectId,
    })
  }

  const removed = (entry: ConversationTimelineEntry) => {
    const message = entry.message
    if (message) dropRemovedMessage(queryClient, { conversationId, messageId: message.id })
    onChanged(entry)
    refreshMemoryCaches(queryClient)
    refreshLists()
  }

  const canReact = (message: ConversationMessage) => canPost && !message.removed

  // Your read mark moves back to just before this message, and the New line
  // with it. The page doesn't mark it read again until something new arrives
  // (useAutoMarkSeen).
  const markUnread = (entry: ConversationTimelineEntry, message: ConversationMessage) => {
    void markConversationUnread({ conversationId, messageId: message.id })
      .then(() => {
        onMarkedUnread?.(entry)
        showToast({ tone: 'success', title: 'Marked unread' })
      })
      .catch((reason: unknown) => toastError('It wasn’t marked unread', reason))
      .finally(refreshLists)
  }

  const copy = (text: string, done: string, failure: string) =>
    void (navigator.clipboard ? navigator.clipboard.writeText(text) : Promise.reject(new Error('This browser can’t copy from here.')))
      .then(() => showToast({ tone: 'success', title: done }))
      .catch((reason: unknown) => toastError(failure, reason))
  const copyLink = (message: ConversationMessage) =>
    copy(new URL(linkFor(message.id), window.location.origin).toString(), 'Link copied', 'The link wasn’t copied')

  const showReactions = (messageId: string, reactions: MessageReaction[]) => {
    setCachedReactions(queryClient, { conversationId, messageId, reactions })
    onReactionsChanged?.(messageId, reactions)
  }

  // Show the change at once, then what the server says; on failure, put the
  // reactions back, say so, and refetch.
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
      .catch((reason: unknown) => {
        if (!latest()) return
        showReactions(message.id, before)
        invalidateConversation(queryClient, projectId, conversationId)
        toastError(reacted ? 'Your reaction wasn’t added' : 'Your reaction wasn’t taken away', reason)
      })
  }

  const startEditing = (message: ConversationMessage) =>
    setEditing({ conversationId, messageId: message.id, revision: message.revision })

  /** Up arrow in an empty composer: edit your newest message here. */
  const editLast = (entries: readonly ConversationTimelineEntry[]) => {
    for (let index = entries.length - 1; index >= 0; index -= 1) {
      const message = entries[index].message
      if (message && message.author.id === viewerId && canEdit(message)) {
        startEditing(message)
        window.requestAnimationFrame(() =>
          document.getElementById(`message-${message.id}`)?.scrollIntoView({ block: 'nearest' }),
        )
        return true
      }
    }
    return false
  }

  const closeEditor = () => {
    const home = editorHome.current
    setEditing(null)
    window.requestAnimationFrame(() => focusComposerNear(home))
  }

  const reactionsFor = (entry: ConversationTimelineEntry): ReactNode => {
    const message = entry.message
    if (!message || message.removed || !message.reactions?.length) return null
    return (
      <MessageReactions
        onPick={canReact(message) ? (choice, reacted) => react(message, choice, reacted) : undefined}
        onToggle={canReact(message) ? (reaction) => react(message, reaction, !reaction.byViewer) : undefined}
        reactions={message.reactions}
        viewerId={viewerId}
      />
    )
  }

  const menuFor = (entry: ConversationTimelineEntry, message: ConversationMessage): MenuEntry[] => {
    const groups: MenuEntry[][] = [
      [
        ...(canEdit(message) ? [{ label: 'Edit', icon: 'pencil' as const, onSelect: () => startEditing(message) }] : []),
        ...later.shareItems(message),
      ],
      memory.itemsFor(message),
      [
        ...(capabilities?.markSeen ? [{ label: 'Mark unread from here', icon: 'inbox' as const, onSelect: () => markUnread(entry, message) }] : []),
        { label: 'Copy text', icon: 'copy', onSelect: () => copy(message.plainText, 'Text copied', 'The text wasn’t copied') },
        { label: 'Copy link', icon: 'link', onSelect: () => copyLink(message) },
      ],
      later.remindItems(message),
      canRemove(message) ? [{ label: 'Remove message…', icon: 'trash', danger: true, onSelect: () => setRemoving(message) }] : [],
    ]
    return groups
      .filter((group) => group.length > 0)
      .flatMap((group, index) => (index === 0 ? group : [{ separator: true as const }, ...group]))
  }

  const toolbarFor = (entry: ConversationTimelineEntry): ReactNode => {
    const message = entry.message
    if (!message) return null
    if (message.removed) {
      return <IconButton icon="link" label="Copy link to message" onClick={() => copyLink(message)} size="sm" />
    }
    return (
      <>
        {canReact(message) ? (
          <ReactionPicker onPick={(choice, reacted) => react(message, choice, reacted)} reactions={message.reactions ?? []} />
        ) : null}
        {primaryFor?.(message, entry)}
        {later.saveButton(message)}
        <Menu
          items={menuFor(entry, message)}
          label="More message actions"
          placement="bottom-end"
          trigger={<IconButton icon="more" label="More" size="sm" />}
        />
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
      <div className="aw-msg__editor-home" ref={editorHome}>
        <MessageEditor
          conversationId={conversationId}
          expectedRevision={editing.revision}
          key={message.id}
          message={message}
          onCancel={closeEditor}
          onSaved={(saved) => {
            closeEditor()
            if (saved.message?.removed) removed(saved)
            else onChanged(saved)
          }}
          searchActors={searchActors}
          searchWorkItems={searchWorkItems}
        />
      </div>
    )
  }

  const dialogs = (
    <>
      {later.dialogs}
      {memory.dialogs}
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

  return { toolbarFor, reactionsFor, editorFor, editLast, openHistory: setHistory, dialogs }
}
