import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Pin, PinOff } from 'lucide-react'


import { toastError } from '../app/mutations'
import { memoryKeys, pin, unpin } from '../memory/api'
import { usePins } from '../memory/hooks'
import { useProjects } from '../projects/hooks'
import { showToast, type MenuEntry } from '../ui'
import { LazyDecisionDialog } from './LazyDecisionDialog'
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
  // A DM's decision goes in a project the person can write in; with none, there's nowhere to save it.
  const projects = useProjects()
  const decisionProjects = direct ? (projects.data?.projects ?? []).filter((project) => !project.archivedAt && project.role !== 'viewer') : []
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
      ...(direct && decisionProjects.length === 0
        ? []
        : [{ label: 'Save as decision…', icon: 'checklist' as const, onSelect: () => setDeciding(message) }]),
    ]
  }

  const dialogs = deciding ? (
    <LazyDecisionDialog
      conversationId={conversationId}
      message={deciding}
      onClose={() => setDeciding(null)}
      projectId={projectId}
      projects={direct ? decisionProjects : null}
    />
  ) : null

  return { itemsFor, dialogs, isPinned: (messageId: string) => pinned.has(messageId) }
}
