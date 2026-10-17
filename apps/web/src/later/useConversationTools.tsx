import { BellOff } from 'lucide-react'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router-dom'

import { createIdempotencyKey } from '../conversations/composerModel'
import type { ConversationKind, MessageContentV1 } from '../conversations/types'
import { Banner, Icon, showToast, type MenuEntry } from '../ui'
import { createWorkItem } from '../work-items/api'
import {
  invalidateLater,
  laterKeys,
  saveForLater,
  scheduleMessage,
  setLevel,
  useConversationLevel,
  useScheduled,
  type NotificationLevel,
} from './api'
import { commandsFor, type SlashCommand, type SlashOutcome } from './slashCommands'
import { describeWhen, parseReminder, parseSchedule } from './when'

const LEVELS: { level: NotificationLevel; label: string }[] = [
  { level: 'all', label: 'Every new message' },
  { level: 'mentions', label: 'Only when mentioned' },
  { level: 'nothing', label: 'Nothing' },
]

/**
 * What a conversation adds to its page for W21: slash commands and Schedule
 * for its composer, a line about messages you've scheduled here, and the
 * notification level for its header menu.
 */
export function useConversationTools({
  canPost,
  conversationId,
  kind,
  latestMessageId,
  projectId,
}: {
  canPost: boolean
  conversationId: string
  kind: ConversationKind
  /** /later with nothing after it saves this one. */
  latestMessageId?: string | null
  projectId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const inProject = kind === 'channel' || kind === 'thread'
  const level = useConversationLevel(conversationId)
  const scheduled = useScheduled(conversationId, canPost)
  const [taskKey] = useState(createIdempotencyKey)
  const waiting = (scheduled.data ?? []).filter((item) => item.status === 'scheduled')

  const viewLater = (view?: string) => ({ label: 'View', onSelect: () => navigate(view ? `/later?view=${view}` : '/later') })

  const changeLevel = async (next: NotificationLevel | null) => {
    const result = await setLevel(conversationId, next)
    queryClient.setQueryData(laterKeys.level(conversationId), result)
    const label = LEVELS.find((item) => item.level === result.level)?.label.toLowerCase() ?? result.level
    showToast({ tone: 'success', title: result.level === 'nothing' ? 'Muted here' : `You’ll hear about ${label === 'only when mentioned' ? 'mentions only' : 'every message'} here` })
  }

  const schedule = async (content: MessageContentV1, at: Date) => {
    await scheduleMessage(conversationId, content, at.toISOString())
    invalidateLater(queryClient)
    showToast({ tone: 'success', title: `Scheduled for ${describeWhen(at).toLowerCase()}`, action: viewLater('scheduled') })
  }

  const onCommand = async (
    command: SlashCommand,
    args: string,
    build: (text: string) => MessageContentV1 | null,
  ): Promise<SlashOutcome> => {
    switch (command.name) {
      case 'remind': {
        const reminder = parseReminder(args)
        if (!reminder.note) throw new Error('Say what to remind you of, like “/remind me to call Sam at 3pm”.')
        if (!reminder.at) throw new Error('Add a time, like “at 3pm”, “tomorrow” or “in 20 minutes”.')
        if (reminder.at.getTime() <= Date.now()) throw new Error('That time has passed.')
        await saveForLater({ note: reminder.note, remindAt: reminder.at.toISOString() })
        invalidateLater(queryClient)
        showToast({ tone: 'success', title: `I’ll remind you ${describeWhen(reminder.at).toLowerCase()}`, description: reminder.note, action: viewLater() })
        return { kind: 'done' }
      }
      case 'later': {
        if (args) {
          await saveForLater({ note: args })
        } else if (latestMessageId) {
          await saveForLater({ messageId: latestMessageId })
        } else {
          navigate('/later')
          return { kind: 'done' }
        }
        invalidateLater(queryClient)
        showToast({ tone: 'success', title: 'Saved for later', action: viewLater() })
        return { kind: 'done' }
      }
      case 'schedule': {
        const parsed = parseSchedule(args)
        if (!parsed) throw new Error('Start with a time, like “/schedule tomorrow 9am Morning everyone”.')
        if (parsed.at.getTime() <= Date.now()) throw new Error('That time has passed.')
        const content = parsed.text ? build(parsed.text) : null
        if (!content) throw new Error('Add the message after the time.')
        await schedule(content, parsed.at)
        return { kind: 'done' }
      }
      case 'mute': {
        const word = args.toLowerCase()
        const next: NotificationLevel | null | undefined =
          word === '' || word === 'all off' || word === 'nothing' ? 'nothing'
            : word === 'mentions' ? 'mentions'
              : word === 'all' ? 'all'
                : word === 'off' || word === 'unmute' ? null
                  : undefined
        if (next === undefined) throw new Error('Try “/mute”, “/mute mentions”, “/mute all” or “/mute off”.')
        await changeLevel(next)
        return { kind: 'done' }
      }
      case 'task': {
        if (!args) {
          navigate(`/projects/${encodeURIComponent(projectId)}/work-items?create=task`)
          return { kind: 'done' }
        }
        const item = await createWorkItem({ projectId, title: args.slice(0, 240), priority: 'no_priority', idempotencyKey: taskKey })
        showToast({
          tone: 'success',
          title: `Created ${item.key}`,
          description: item.title,
          action: { label: 'Open', onSelect: () => navigate(`/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(item.id)}`) },
        })
        return { kind: 'done' }
      }
      case 'tag':
        return { kind: 'replace', text: '@' }
      case 'launch':
        navigate(`/projects/${encodeURIComponent(projectId)}/launches`)
        return { kind: 'done' }
    }
  }

  const current = level.data?.level ?? 'mentions'
  const levelMenu: MenuEntry[] = [
    { heading: 'Notify me about' },
    ...LEVELS.map((item) => ({
      label: item.label,
      icon: item.level === 'nothing' ? ('circle-minus' as const) : item.level === 'all' ? ('messages' as const) : ('at-sign' as const),
      checked: current === item.level,
      onSelect: () =>
        void changeLevel(item.level === 'mentions' && kind !== 'thread' ? null : item.level).catch((reason: unknown) =>
          showToast({ tone: 'danger', title: 'That didn’t change.', description: reason instanceof Error ? reason.message : 'Try again.' }),
        ),
    })),
    ...(kind === 'thread' && level.data?.own
      ? [{ label: 'Follow the channel', icon: 'undo' as const, onSelect: () => void changeLevel(null).catch(() => undefined) }]
      : []),
  ]

  const banner = waiting.length ? (
    <Banner className="aw-conv__banner" icon="clock" tone="info">
      {waiting.length === 1
        ? `You have a message scheduled here for ${describeWhen(waiting[0].sendAt).toLowerCase()}.`
        : `You have ${waiting.length} messages scheduled here.`}{' '}
      <Link to="/later?view=scheduled">See scheduled</Link>
    </Banner>
  ) : null

  const muted =
    current === 'nothing' ? (
      <BellOff aria-label="Muted" className="aw-icon aw-convhead__muted" role="img" size={14} />
    ) : current === 'all' ? (
      <Icon className="aw-convhead__muted" label="Every message notifies you" name="messages" size={14} />
    ) : null

  return {
    composer: canPost ? { commands: commandsFor({ inProject }), onCommand, onSchedule: schedule } : {},
    banner,
    levelMenu,
    levelMark: muted,
  }
}
