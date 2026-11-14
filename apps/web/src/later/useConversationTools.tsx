import { BellOff } from 'lucide-react'
import { useRef } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router-dom'
import slashClaims from 'virtual:modules/slots/slash'

import { createIdempotencyKey } from '../conversations/composerModel'
import type { ConversationKind, MessageContentV1 } from '../conversations/types'
import { toastError } from '../app/mutations'
import { useHomeExperience, useProjectExperience } from '../experience/layout'
import { ownerOf } from '../modules/ownership'
import { Banner, countLabel, showToast, type MenuEntry } from '../ui'
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
import { atPhrase, parseReminder, parseSchedule, whenPhrase } from './when'
import '../styles/conversation.css'

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
  // A project's layout can leave out slash commands, scheduling and agents in @ suggestions.
  const projectExperience = useProjectExperience(inProject ? projectId : null)
  // AWD-75: in DMs and groups, the workspace's modules decide.
  const homeExperience = useHomeExperience()
  const experience = inProject ? projectExperience : homeExperience
  const slashOn = (name: string) => {
    const module = ownerOf(slashClaims, name)
    return !module || experience.moduleOn(module)
  }
  const scheduling = experience.shows('talk.scheduling')
  const commands = experience.shows('talk.slash_commands')
    ? commandsFor({ inProject }).filter(
        (command) =>
          slashOn(command.name) &&
          (command.name !== 'schedule' || scheduling) &&
          (command.name !== 'tag' || experience.shows('talk.agent_mentions')) &&
          (command.name !== 'task' || experience.shows('area.tasks')) &&
          (command.name !== 'launch' || experience.shows('area.runs')) &&
          ((command.name !== 'later' && command.name !== 'remind') || experience.shows('workspace.later')),
      )
    : null
  const level = useConversationLevel(conversationId)
  const scheduled = useScheduled(conversationId, canPost)
  // One key per /task: a retry of the same one after a failure reuses it, anything else gets a new one.
  const taskKey = useRef<{ title: string; key: string } | null>(null)
  const waiting = (scheduled.data ?? []).filter((item) => item.status === 'scheduled')

  const viewLater = (view?: string) => ({ label: 'View', onSelect: () => navigate(view ? `/later?view=${view}` : '/later') })

  const changeLevel = async (next: NotificationLevel | null) => {
    const result = await setLevel(conversationId, next)
    queryClient.setQueryData(laterKeys.level(conversationId), result)
    showToast(
      result.level === 'nothing'
        ? { tone: 'success', title: 'Muted', description: 'You won’t be notified here.' }
        : { tone: 'success', title: result.level === 'all' ? 'Notifications: every message' : 'Notifications: mentions only' },
    )
  }

  const schedule = async (content: MessageContentV1, at: Date) => {
    await scheduleMessage(conversationId, content, at.toISOString())
    invalidateLater(queryClient)
    showToast({ tone: 'success', title: `Scheduled to send ${whenPhrase(at)}`, action: viewLater('scheduled') })
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
        showToast({ tone: 'success', title: `Reminder set for ${atPhrase(reminder.at)}`, description: reminder.note, action: viewLater() })
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
      case 'mute':
      case 'unmute':
      case 'notify': {
        const word = args.toLowerCase()
        // "/mute off" read as unmute before /unmute existed; keep it working.
        const next: NotificationLevel | null | undefined =
          command.name === 'unmute' || (command.name === 'mute' && word === 'off') ? null
            : command.name === 'mute' ? (word === '' ? 'nothing' : undefined)
              : word === 'all' ? 'all'
                : word === 'mentions' ? 'mentions'
                  : word === 'off' || word === 'nothing' ? 'nothing'
                    : undefined
        if (next === undefined) throw new Error('Try “/mute”, “/unmute”, or “/notify all”, “/notify mentions” or “/notify off”.')
        await changeLevel(next)
        return { kind: 'done' }
      }
      case 'task': {
        if (!args) {
          navigate(`/projects/${encodeURIComponent(projectId)}/work-items?create=task`)
          return { kind: 'done' }
        }
        const title = args.slice(0, 240)
        if (taskKey.current?.title !== title) taskKey.current = { title, key: createIdempotencyKey() }
        const item = await createWorkItem({ projectId, title, priority: 'no_priority', idempotencyKey: taskKey.current.key })
        taskKey.current = null
        showToast({
          tone: 'success',
          title: experience.shows('tech.task_keys') ? `Created ${item.key}` : `${experience.label('task')} created`,
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
          toastError('Your notification setting didn’t change', reason),
        ),
    })),
    ...(kind === 'thread' && level.data?.own
      ? [
          {
            label: 'Follow the channel',
            icon: 'undo' as const,
            onSelect: () => void changeLevel(null).catch((reason: unknown) => toastError('Your notification setting didn’t change', reason)),
          },
        ]
      : []),
  ]

  const banner = waiting.length ? (
    <Banner className="aw-conv__banner" icon="clock" tone="info">
      {waiting.length === 1
        ? `You have a message scheduled to send here ${whenPhrase(waiting[0].sendAt)}.`
        : `You have ${countLabel(waiting.length, 'message')} scheduled here.`}{' '}
      <Link to="/later?view=scheduled">See scheduled</Link>
    </Banner>
  ) : null

  const muted =
    current === 'nothing' ? (
      <BellOff aria-label="Muted" className="aw-icon aw-convhead__muted" role="img" size={14} />
    ) : null

  const composer: { commands?: SlashCommand[]; onCommand?: typeof onCommand; onSchedule?: typeof schedule; peopleOnly?: boolean } = canPost
    ? {
        commands: commands ?? undefined,
        onCommand: commands ? onCommand : undefined,
        onSchedule: scheduling ? schedule : undefined,
        peopleOnly: !experience.shows('talk.agent_mentions'),
      }
    : {}

  return {
    composer,
    banner,
    levelMenu,
    levelMark: muted,
  }
}
