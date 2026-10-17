import { useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { invalidateConversation } from '../conversations/liveMessages'
import type { ConversationMessage, SavedMark } from '../conversations/types'
import { IconButton, Menu, showToast, type MenuEntry } from '../ui'
import { changeSaved, invalidateLater, removeSaved, saveForLater } from './api'
import { ForwardDialog, MessageTaskDialog } from './dialogs'
import { TimeDialog } from './TimeDialog'
import { describeWhen, whenChoices } from './when'

type Open =
  | { kind: 'remind'; message: ConversationMessage }
  | { kind: 'forward'; message: ConversationMessage }
  | { kind: 'task'; message: ConversationMessage }

/**
 * Save for later, Remind me, Forward and Make a task, for each message's
 * toolbar. Your saved mark changes here at once and is confirmed when the
 * conversation refetches.
 */
export function useLaterActions({
  canForward,
  conversationId,
  projectId,
  taskProject,
}: {
  /** You can see this conversation's messages (always) and post somewhere. */
  canForward: boolean
  conversationId: string
  projectId: string
  /** The conversation is in a project with tasks, not a DM or group. */
  taskProject: boolean
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [marks, setMarks] = useState(new Map<string, SavedMark | null>())
  const [open, setOpen] = useState<Open | null>(null)

  const markOf = (message: ConversationMessage) => (marks.has(message.id) ? marks.get(message.id) ?? null : message.saved ?? null)
  const mark = (messageId: string, value: SavedMark | null) =>
    setMarks((current) => new Map(current).set(messageId, value))
  const refresh = () => {
    invalidateLater(queryClient)
    invalidateConversation(queryClient, projectId, conversationId)
  }
  const failed = (title: string) => (reason: unknown) =>
    showToast({ tone: 'danger', title, description: reason instanceof Error ? reason.message : 'Try again.' })

  const save = (message: ConversationMessage, remindAt: Date | null = null) => {
    const saved = markOf(message)
    const request = saved
      ? changeSaved(saved.id, { remindAt: remindAt ? remindAt.toISOString() : null, done: false })
      : saveForLater({ messageId: message.id, remindAt: remindAt?.toISOString() ?? null })
    return request.then((item) => {
      mark(message.id, { id: item.id, remindAt: item.remindAt })
      refresh()
      showToast({
        tone: 'success',
        title: item.remindAt ? `I’ll remind you ${describeWhen(item.remindAt).toLowerCase()}` : 'Saved for later',
        action: { label: 'View Later', onSelect: () => navigate('/later') },
      })
    })
  }

  const unsave = (message: ConversationMessage, saved: SavedMark) => {
    mark(message.id, null)
    void removeSaved(saved.id)
      .then(() => {
        refresh()
        showToast({ tone: 'info', title: 'Removed from Later', onUndo: () => save(message, saved.remindAt ? new Date(saved.remindAt) : null) })
      })
      .catch((reason: unknown) => {
        mark(message.id, saved)
        failed('Couldn’t remove it from Later.')(reason)
      })
  }

  const buttonsFor = (message: ConversationMessage): ReactNode => {
    if (message.removed) return null
    const saved = markOf(message)
    const items: MenuEntry[] = [
      { heading: saved?.remindAt ? `Reminder: ${describeWhen(saved.remindAt)}` : 'Remind me about this' },
      ...whenChoices().map((choice) => ({
        label: choice.label,
        icon: 'clock' as const,
        onSelect: () => void save(message, choice.at).catch(failed('The reminder wasn’t set.')),
      })),
      { label: 'Custom time…', icon: 'clock' as const, onSelect: () => setOpen({ kind: 'remind', message }) },
      ...(saved?.remindAt
        ? [{ label: 'Stop reminding me', icon: 'x' as const, onSelect: () => void save(message, null).catch(failed('That didn’t work.')) }]
        : []),
      { separator: true as const },
      ...(canForward ? [{ label: 'Forward…', icon: 'send' as const, onSelect: () => setOpen({ kind: 'forward', message }) }] : []),
      ...(taskProject ? [{ label: 'Make a task…', icon: 'work' as const, onSelect: () => setOpen({ kind: 'task', message }) }] : []),
      {
        label: 'Copy text',
        icon: 'copy' as const,
        onSelect: () =>
          void navigator.clipboard
            ?.writeText(message.plainText)
            .then(() => showToast({ tone: 'success', title: 'Text copied' }))
            .catch(() => undefined),
      },
    ]
    return (
      <>
        <IconButton
          icon="bookmark"
          label={saved ? 'Remove from Later' : 'Save for later'}
          onClick={() => (saved ? unsave(message, saved) : void save(message).catch(failed('It wasn’t saved.')))}
          pressed={Boolean(saved)}
          size="sm"
        />
        <Menu items={items} label="More message actions" placement="bottom-end" trigger={<IconButton icon="more" label="More" size="sm" />} />
      </>
    )
  }

  const dialogs = open ? (
    open.kind === 'remind' ? (
      <TimeDialog action="Remind me" onClose={() => setOpen(null)} onPick={(at) => save(open.message, at)} title="Remind me about this" />
    ) : open.kind === 'forward' ? (
      <ForwardDialog currentConversationId={conversationId} currentProjectId={projectId} message={open.message} onClose={() => setOpen(null)} />
    ) : (
      <MessageTaskDialog message={open.message} onClose={() => setOpen(null)} projectId={projectId} />
    )
  ) : null

  return { buttonsFor, dialogs }
}
