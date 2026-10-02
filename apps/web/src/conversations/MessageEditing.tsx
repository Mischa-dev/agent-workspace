import { useState } from 'react'

import { isApiError } from '../api/errors'
import { Banner, Button, Dialog } from '../ui'
import { editConversationMessage, getConversationMessage, removeConversationMessage } from './api'
import { draftFromContent, EDIT_HISTORY_HINT } from './composerModel'
import { StructuredComposer } from './StructuredComposer'
import type {
  ConversationActor,
  ConversationMessage,
  ConversationTimelineEntry,
  ConversationWorkItemReference,
} from './types'

/** Why an edit didn't save, in words a person can act on. */
async function editError(
  reason: unknown,
  conversationId: string,
  messageId: string,
): Promise<{ message: string; removedEntry?: ConversationTimelineEntry }> {
  if (!isApiError(reason)) {
    return { message: reason instanceof Error ? reason.message : 'Your edit wasn’t saved.' }
  }
  if (reason.status === 403) return { message: 'You can only edit your own messages.' }
  if (reason.status === 404) return { message: 'That message wasn’t found, or you can’t see it.' }
  if (reason.code === 'version_conflict') {
    return {
      message:
        'This message changed somewhere else, maybe through a connected app. Copy your text, then reopen the editor.',
    }
  }
  if (reason.status === 409) {
    // The server says "conflict" for a removed message, an archived
    // conversation and the edit limit alike; look at the message to tell.
    const entry = await getConversationMessage(conversationId, messageId).catch(() => null)
    if (entry?.message?.removed) return { message: 'This message was removed.', removedEntry: entry }
    return { message: 'This message can’t be edited any more.' }
  }
  if (reason.status === 422) return { message: 'That isn’t a message that can be saved. Check its length and mentions.' }
  return { message: reason.message }
}

/**
 * The inline editor that replaces a message's body. It saves against the
 * revision it opened at, so a change made elsewhere meanwhile is a conflict
 * rather than silently overwritten.
 */
export function MessageEditor({
  conversationId,
  expectedRevision,
  message,
  onCancel,
  onSaved,
  searchActors,
  searchWorkItems,
}: {
  conversationId: string
  expectedRevision: number
  message: ConversationMessage
  onCancel: () => void
  onSaved: (entry: ConversationTimelineEntry) => void
  searchActors: (query: string) => Promise<ConversationActor[]>
  searchWorkItems?: (query: string) => Promise<ConversationWorkItemReference[]>
}) {
  const [draft] = useState(() => draftFromContent(message))
  return (
    <StructuredComposer
      autoFocus
      className="aw-msg__editor"
      initialDraft={draft}
      keepDraftWhileSaving
      label="Edit message"
      note={EDIT_HISTORY_HINT}
      onCancel={onCancel}
      onSubmit={async (submission) => {
        try {
          const result = await editConversationMessage({
            conversationId,
            messageId: message.id,
            content: submission.content,
            expectedRevision,
            idempotencyKey: submission.idempotencyKey,
          })
          onSaved(result.entry)
        } catch (reason) {
          const failure = await editError(reason, conversationId, message.id)
          if (failure.removedEntry) onSaved(failure.removedEntry)
          throw new Error(failure.message, { cause: reason })
        }
      }}
      placeholder="Edit message"
      searchActors={searchActors}
      searchWorkItems={searchWorkItems}
      submitIcon="check"
      submitLabel="Save"
    />
  )
}

function removeError(reason: unknown, own: boolean) {
  if (!isApiError(reason)) return reason instanceof Error ? reason.message : 'The message wasn’t removed.'
  if (reason.status === 403) {
    return own
      ? 'You can’t remove messages here any more.'
      : 'Only project owners and admins can remove other people’s messages.'
  }
  if (reason.status === 404) return 'That message wasn’t found, or you can’t see it.'
  return reason.message
}

/** Confirms removing a message, as its author or as a project owner or admin. */
export function RemoveMessageDialog({
  conversationId,
  message,
  onClose,
  onRemoved,
  viewerId,
}: {
  conversationId: string
  message: ConversationMessage
  onClose: () => void
  onRemoved: (entry: ConversationTimelineEntry) => void
  viewerId: string
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const own = message.author.id === viewerId
  const confirm = () => {
    if (busy) return
    setBusy(true)
    setError(null)
    void removeConversationMessage({ conversationId, messageId: message.id })
      .then((result) => {
        onRemoved(result.entry)
        onClose()
      })
      .catch((reason: unknown) => {
        setError(removeError(reason, own))
        setBusy(false)
      })
  }
  const description = 'Everyone will see “This message was removed” instead. The text and its earlier versions are deleted and can’t be brought back.'
  return (
    <Dialog
      description={own ? description : `${description} They’ll see that you removed it.`}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} onClick={confirm} variant="danger">
            Remove message
          </Button>
        </>
      }
      onClose={onClose}
      open
      title={own ? 'Remove this message?' : `Remove ${message.author.displayName}’s message?`}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
