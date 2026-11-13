import { useEffect } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { sendConversationMessage } from '../conversations/api'
import type { OutboxTarget, QueuedSend } from '../conversations/composerModel'
import { showToast } from '../ui'
import { commentContent, createWorkItemComment } from '../work-items/api'
import { onReconnect } from './localDrafts'
import { flushOutbox } from './outbox'

function send(target: OutboxTarget, item: QueuedSend) {
  if (target.kind === 'message') {
    return sendConversationMessage({
      conversationId: target.conversationId,
      kind: target.conversationKind,
      content: item.content,
      idempotencyKey: item.idempotencyKey,
      attachments: item.attachments,
      replyTo: item.replyTo,
    })
  }
  return createWorkItemComment({
    projectId: target.projectId,
    workItemId: target.workItemId,
    content: commentContent(item),
    idempotencyKey: item.idempotencyKey,
    attachments: item.attachments,
    replyTo: item.replyTo,
  })
}

/**
 * Sends messages and comments written offline once the connection is back,
 * whichever page is open, and says so. Their own boxes, when open, send them
 * instead.
 */
export default function Outbox() {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  useEffect(() => {
    let running = false
    const run = () => {
      if (running) return
      running = true
      void flushOutbox(send)
        .then(({ refused, sent }) => {
          for (const item of [...sent, ...refused]) {
            const id = item.target.kind === 'message' ? item.target.conversationId : item.target.workItemId
            void queryClient.invalidateQueries({ predicate: (query) => query.queryKey.includes(id) })
          }
          const last = sent.at(-1)
          if (last) {
            showToast({
              tone: 'success',
              title: sent.length === 1 ? 'Sent what you wrote offline' : `Sent ${sent.length} messages you wrote offline`,
              action: { label: 'View', onSelect: () => navigate(last.target.href) },
            })
          }
          const first = refused[0]
          if (first) {
            showToast({
              tone: 'danger',
              title: refused.length === 1 ? 'A message you wrote offline wasn’t sent' : `${refused.length} messages you wrote offline weren’t sent`,
              description: 'Each is back in its box when you open it, to change or copy.',
              action: { label: 'Open', onSelect: () => navigate(first.target.href) },
            })
          }
        })
        .catch(() => undefined)
        .finally(() => {
          running = false
        })
    }
    run()
    return onReconnect(run)
  }, [navigate, queryClient])
  return null
}
