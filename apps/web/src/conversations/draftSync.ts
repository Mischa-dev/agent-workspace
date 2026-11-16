// Drafts across devices (C2.28). The composer keeps its draft in this
// browser first (app/localDrafts.ts); two seconds after typing stops it
// saves it to the server too, through `message_draft_set`, so it waits on
// the writer's other devices. An empty box with nothing kept here picks up
// the server's draft. Offline, the local draft is all there is, and a save
// that fails is simply tried again on the next change.

import { useEffect, useRef } from 'react'

import { callOp, newRequestId } from '../api/ops'
import type { ConversationActor, ConversationWorkItemChip } from './types'

export type SyncedDraft = { text: string; mentions: ConversationActor[]; workItems: ConversationWorkItemChip[] }

const SAVE_AFTER_MS = 2_000
const CONVERSATION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** Whether a composer's draft key is a conversation's, which the server keeps. */
export function syncsDraft(draftKey: string | undefined): draftKey is string {
  return Boolean(draftKey && CONVERSATION_ID.test(draftKey))
}

/** What the server keeps for a draft: nothing when there's no text. */
export function draftToSave(draft: SyncedDraft): SyncedDraft | undefined {
  return draft.text.trim() ? draft : undefined
}

function isDraft(value: unknown): value is SyncedDraft {
  const draft = value as Partial<SyncedDraft> | null
  return Boolean(draft && typeof draft.text === 'string' && Array.isArray(draft.mentions ?? []) && Array.isArray(draft.workItems ?? []))
}

/**
 * Keeps a composer's draft on the server. `restore` fills an empty box with
 * the server's draft when this browser had none.
 */
export function useDraftSync(draftKey: string | undefined, draft: SyncedDraft, startedEmpty: boolean, restore: (draft: SyncedDraft) => void) {
  const synced = useRef<string | null>(null)
  const typedAt = useRef(0)
  const restoreRef = useRef(restore)
  useEffect(() => {
    restoreRef.current = restore
  })

  useEffect(() => {
    if (!syncsDraft(draftKey) || !startedEmpty) return
    let cancelled = false
    void callOp('message_draft_list', { conversationId: draftKey })
      .then((output) => {
        const found = output.drafts[0]?.draft
        if (cancelled || !isDraft(found)) return
        synced.current = JSON.stringify(draftToSave({ text: found.text, mentions: found.mentions ?? [], workItems: found.workItems ?? [] }) ?? null)
        restoreRef.current({ text: found.text, mentions: found.mentions ?? [], workItems: found.workItems ?? [] })
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
    // Once per conversation: a new conversation is a new composer.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftKey])

  const { text, mentions, workItems } = draft
  useEffect(() => {
    if (!syncsDraft(draftKey)) return
    const toSave = draftToSave({ text, mentions, workItems })
    const serialized = JSON.stringify(toSave ?? null)
    // Nothing kept and nothing to keep: no call.
    if (serialized === synced.current || (synced.current === null && !toSave)) return
    typedAt.current = Date.now()
    const timer = window.setTimeout(() => {
      void callOp(
        'message_draft_set',
        { conversationId: draftKey, ...(toSave ? { draft: toSave } : {}), savedAt: new Date(typedAt.current).toISOString() },
        { requestId: newRequestId() },
      )
        .then(() => {
          synced.current = serialized
        })
        .catch(() => undefined)
    }, SAVE_AFTER_MS)
    return () => window.clearTimeout(timer)
  }, [draftKey, text, mentions, workItems])
}
