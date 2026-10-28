import { lazy, Suspense } from 'react'

import type { ConversationMessage } from './types'

// Forward and Create task load when one of them opens.
const ForwardDialog = lazy(() => import('../later/dialogs').then((module) => ({ default: module.ForwardDialog })))
const MessageTaskDialog = lazy(() => import('../later/dialogs').then((module) => ({ default: module.MessageTaskDialog })))

/** Forward a message, or make a task from it. */
export function MessageDialog({
  conversationId,
  kind,
  message,
  onClose,
  projectId,
}: {
  conversationId: string
  kind: 'forward' | 'task'
  message: ConversationMessage
  onClose: () => void
  projectId: string
}) {
  return (
    <Suspense fallback={null}>
      {kind === 'forward' ? (
        <ForwardDialog currentConversationId={conversationId} currentProjectId={projectId} message={message} onClose={onClose} />
      ) : (
        <MessageTaskDialog message={message} onClose={onClose} projectId={projectId} />
      )}
    </Suspense>
  )
}
