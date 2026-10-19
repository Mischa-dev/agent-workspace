import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Pin, PinOff } from 'lucide-react'


import { toastError } from '../app/mutations'
import { memoryKeys, pin, unpin } from '../memory/api'
import { usePins } from '../memory/hooks'
import { showToast, type MenuEntry } from '../ui'
import { DecisionDialog } from './DecisionDialog'
import type { ConversationMessage } from './types'

/**
 * Pin and Save as decision, for a message's More menu. Pin is a toggle that
 * says what it did and can be undone from the toast; a failure says why.
 */
export function useMessageMemory({
  conversationId,
  direct = false,
  enabled,
  projectId,
}: {
  conversationId: string
  /** A DM or group: its decisions have no project page to show them on. */
  direct?: boolean
  /** You can post here, which is what pinning and deciding need. */
  enabled: boolean
  projectId: string
}) {
  const queryClient = useQueryClient()
  const pins = usePins(enabled ? projectId : '', conversationId)
  const [deciding, setDeciding] = useState<ConversationMessage | null>(null)
  const pinned = new Set((pins.data?.pins ?? []).filter((item) => item.kind === 'message').map((item) => item.id))
  const pinsKey = memoryKeys.pins(projectId, conversationId)

  const setPinned = (message: ConversationMessage, next: boolean, undoable = true): Promise<void> =>
    (next ? pin(conversationId, 'message', message.id) : unpin(conversationId, 'message', message.id))
      .then((result) => {
        queryClient.setQueryData(pinsKey, result)
        if (!undoable) return
        showToast({
          tone: 'success',
          title: next ? 'Pinned to this conversation' : 'Unpinned from this conversation',
          onUndo: () => setPinned(message, !next, false),
        })
      })
      .catch((reason: unknown) => {
        toastError(next ? 'The message wasn’t pinned' : 'The message wasn’t unpinned', reason)
        void queryClient.invalidateQueries({ queryKey: pinsKey })
      })

  const itemsFor = (message: ConversationMessage): MenuEntry[] => {
    if (!enabled || message.removed) return []
    const isPinned = pinned.has(message.id)
    return [
      {
        label: isPinned ? 'Unpin from this conversation' : 'Pin to this conversation',
        icon: isPinned ? PinOff : Pin,
        onSelect: () => void setPinned(message, !isPinned),
      },
      { label: 'Save as decision…', icon: 'checklist', onSelect: () => setDeciding(message) },
    ]
  }

  const dialogs = deciding ? (
    <DecisionDialog conversationId={conversationId} direct={direct} message={deciding} onClose={() => setDeciding(null)} projectId={projectId} />
  ) : null

  return { itemsFor, dialogs, isPinned: (messageId: string) => pinned.has(messageId) }
}
