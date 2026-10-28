import { useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { useProjectExperience } from '../experience/layout'
import { changeSaved, invalidateLater, removeSaved, saveForLater } from '../later/api'
import { TimeDialog } from '../later/TimeDialog'
import { atPhrase, describeWhen, whenChoices } from '../later/when'
import { IconButton, showToast, type MenuEntry } from '../ui'
import { invalidateConversation } from './liveMessages'
import { MessageDialog } from './MessageDialogs'
import type { ConversationMessage, SavedMark } from './types'

type Open =
  | { kind: 'remind'; message: ConversationMessage }
  | { kind: 'forward'; message: ConversationMessage }
  | { kind: 'task'; message: ConversationMessage }

/**
 * Save for later, Remind me, Forward and Create task for a message: the
 * toolbar's Save button and the More menu's entries. Your saved mark changes
 * here at once and is confirmed when the conversation refetches.
 */
export function useMessageLater({
  conversationId,
  projectId,
  taskProject,
}: {
  conversationId: string
  projectId: string
  /** The conversation is in a project with tasks, not a DM or group. */
  taskProject: boolean
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [marks, setMarks] = useState(new Map<string, SavedMark | null>())
  const [open, setOpen] = useState<Open | null>(null)
  const experience = useProjectExperience(taskProject ? projectId : null)
  const tasks = taskProject && experience.shows('area.tasks')

  const markOf = (message: ConversationMessage) => (marks.has(message.id) ? (marks.get(message.id) ?? null) : (message.saved ?? null))
  const mark = (messageId: string, value: SavedMark | null) => setMarks((current) => new Map(current).set(messageId, value))
  const refresh = () => {
    invalidateLater(queryClient)
    invalidateConversation(queryClient, projectId, conversationId)
  }
  const failed = (title: string) => (reason: unknown) => {
    toastError(title, reason)
  }

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
        title: item.remindAt ? `Reminder set for ${atPhrase(item.remindAt)}` : 'Saved for later',
        action: { label: 'View Later', onSelect: () => navigate('/later') },
      })
    })
  }

  const unsave = (message: ConversationMessage, saved: SavedMark) => {
    mark(message.id, null)
    void removeSaved(saved.id)
      .then(() => {
        refresh()
        showToast({
          tone: 'info',
          title: 'Removed from Later',
          onUndo: () => save(message, saved.remindAt ? new Date(saved.remindAt) : null),
        })
      })
      .catch((reason: unknown) => {
        mark(message.id, saved)
        failed('It wasn’t removed from Later')(reason)
      })
  }

  /** The toolbar's bookmark: saves, or takes it out of Later again. */
  const saveButton = (message: ConversationMessage): ReactNode => {
    const saved = markOf(message)
    return (
      <IconButton
        icon="bookmark"
        label={saved ? 'Remove from Later' : 'Save for later'}
        onClick={() => (saved ? unsave(message, saved) : void save(message).catch(failed('It wasn’t saved for later')))}
        pressed={Boolean(saved)}
        size="sm"
      />
    )
  }

  /** Forward and Create task, for the top of the More menu. */
  const shareItems = (message: ConversationMessage): MenuEntry[] => [
    { label: 'Forward…', icon: 'send', onSelect: () => setOpen({ kind: 'forward', message }) },
    ...(tasks ? [{ label: `Create ${experience.word('task')}…`, icon: 'work' as const, onSelect: () => setOpen({ kind: 'task', message }) }] : []),
  ]

  /** Remind me, under its own heading. */
  const remindItems = (message: ConversationMessage): MenuEntry[] => {
    const saved = markOf(message)
    return [
      { heading: saved?.remindAt ? `Reminder: ${describeWhen(saved.remindAt)}` : 'Remind me' },
      ...whenChoices().map((choice) => ({
        label: choice.label,
        icon: 'clock' as const,
        // Read the time again when it's picked, so a toolbar left open still means "from now".
        onSelect: () =>
          void save(message, whenChoices().find((fresh) => fresh.id === choice.id)?.at ?? choice.at).catch(failed('The reminder wasn’t set')),
      })),
      { label: 'Custom time…', icon: 'clock', onSelect: () => setOpen({ kind: 'remind', message }) },
      ...(saved?.remindAt
        ? [{ label: 'Stop reminding me', icon: 'x' as const, onSelect: () => void save(message, null).catch(failed('The reminder wasn’t turned off')) }]
        : []),
    ]
  }

  const dialogs = open ? (
    open.kind === 'remind' ? (
      <TimeDialog action="Remind me" onClose={() => setOpen(null)} onPick={(at) => save(open.message, at)} title="Remind me about this" />
    ) : (
      <MessageDialog conversationId={conversationId} kind={open.kind} message={open.message} onClose={() => setOpen(null)} projectId={projectId} />
    )
  ) : null

  return { saveButton, shareItems, remindItems, dialogs }
}
