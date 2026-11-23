import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { FilePlus, Pin, PinOff } from 'lucide-react'

import { can } from '../auth/can'
import { toastError } from '../app/mutations'
import { memoryKeys, pin, unpin } from '../memory/api'
import { usePins } from '../memory/hooks'
import { useProjects } from '../projects/hooks'
import { useProjectExperience } from '../experience/layout'
import { showToast, type MenuEntry } from '../ui'
import { LazyDecisionDialog } from './LazyDecisionDialog'
import { LazyMakePageDialog } from '../memory/LazyMakePageDialog'
import { LazyRecordFromMessageDialog } from './LazyRecordFromMessageDialog'
import { LazyStickyFromMessageDialog } from './LazyStickyFromMessageDialog'
import type { ConversationMessage } from './types'

/**
 * Pin, Save as decision and Make a page, for a message's More menu. Pin is
 * a toggle that says what it did and can be undone from the toast; a
 * failure says why.
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
  const [making, setMaking] = useState<ConversationMessage | null>(null)
  const [recording, setRecording] = useState<ConversationMessage | null>(null)
  const [sticking, setSticking] = useState<ConversationMessage | null>(null)
  const experience = useProjectExperience(projectId)
  const records = experience.shows('area.collections')
  const boards = experience.shows('area.whiteboards')
  // A DM's decision goes in a project the person can write in; with none, there's nowhere to save it.
  const projects = useProjects()
  const decisionProjects = direct ? (projects.data?.projects ?? []).filter((project) => !project.archivedAt && can(project, 'pages.edit')) : []
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
        : [
            { label: 'Save as decision…', icon: 'checklist' as const, onSelect: () => setDeciding(message) },
            { label: 'Make a page…', icon: FilePlus, onSelect: () => setMaking(message) },
          ]),
      // C2.21: a record in one of this space's collections, linked back here.
      ...(direct || !records ? [] : [{ label: 'Make a record…', icon: 'table' as const, onSelect: () => setRecording(message) }]),
      // C2.22: a sticky on one of this space's whiteboards, linked back here.
      ...(direct || !boards ? [] : [{ label: 'Put on a whiteboard…', icon: 'whiteboard' as const, onSelect: () => setSticking(message) }]),
    ]
  }

  const dialogs = (
    <>
      {deciding ? (
        <LazyDecisionDialog
          conversationId={conversationId}
          message={deciding}
          onClose={() => setDeciding(null)}
          projectId={projectId}
          projects={direct ? decisionProjects : null}
        />
      ) : null}
      {making ? (
        <LazyMakePageDialog
          onClose={() => setMaking(null)}
          projectId={projectId}
          projects={direct ? decisionProjects : null}
          source={{ kind: 'messages', messageIds: [making.id] }}
        />
      ) : null}
      {recording ? (
        <LazyRecordFromMessageDialog message={recording} onClose={() => setRecording(null)} projectId={projectId} />
      ) : null}
      {sticking ? <LazyStickyFromMessageDialog message={sticking} onClose={() => setSticking(null)} projectId={projectId} /> : null}
    </>
  )

  return { itemsFor, dialogs, isPinned: (messageId: string) => pinned.has(messageId) }
}
